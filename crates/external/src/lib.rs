// Copyright 2026 Ryan Duguid
// SPDX-License-Identifier: MIT OR Apache-2.0

//! pdfcraft-external — optional external programs (L4; never built for the web). See the README.
//!
//! PlutarchPages converts Office files through an installed LibreOffice and HTML files and web
//! pages through an installed Chrome, Edge or Chromium, and reads documents aloud through the
//! system's speech command. These programs are separate installations: nothing here links, copies
//! or bundles them. Conversion is off unless the user turns it on (the engine enforces that).
//!
//! Each run gets a private folder with a fresh browser or Office profile, a fixed input name and
//! an absolute path, a deadline and a cancel flag; when a run ends, its process group (Unix) or
//! process tree (Windows) is ended, best effort (see `supervise`). Local HTML is opened with
//! scripts off and the browser told to refuse network requests (unknown host names and a dead
//! proxy); that rests on the browser's flags, not on an observed block. None of this is an
//! operating-system sandbox: convert only files you trust.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

mod speech;
mod supervise;
mod web;

pub use speech::{MAX_SPEECH, Speech, SpeechBackend};
pub use web::checked_web_url;

use supervise::Ended;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ExternalError {
    /// The program isn't installed, or the one configured isn't usable.
    #[error("{0}")]
    Unavailable(String),
    #[error("{0}")]
    InvalidInput(String),
    #[error("{0} took longer than {1} seconds and was stopped")]
    TimedOut(String, u64),
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    ProcessFailed(String),
    /// The program's result is missing, too large or not a PDF.
    #[error("{0}")]
    InvalidOutput(String),
    #[error("{0}")]
    Io(String),
}

fn io(context: &str) -> impl Fn(std::io::Error) -> ExternalError + '_ {
    move |e| ExternalError::Io(format!("{context}: {e}"))
}

/// How long a conversion may take and how large its input and output may be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub timeout: Duration,
    pub max_input: u64,
    pub max_output: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { timeout: Duration::from_secs(120), max_input: 256 << 20, max_output: 512 << 20 }
    }
}

/// Office formats handed to LibreOffice. Macro-enabled formats (docm, xlsm, pptm) are not offered.
pub const OFFICE: &[&str] = &["doc", "docx", "odt", "rtf", "xls", "xlsx", "ods", "ppt", "pptx", "odp"];
/// HTML files handed to the browser.
pub const HTML: &[&str] = &["html", "htm", "xhtml"];
/// The oldest LibreOffice used: 26.2.5 fixed a series of document-parser memory-safety bugs
/// (CVE-2026-4430, CVE-2026-6040, CVE-2026-6045, CVE-2026-8357, CVE-2026-8358 and others).
pub const MIN_LIBREOFFICE: (u32, u32, u32) = (26, 2, 5);

fn extension_in(name: &str, list: &'static [&'static str]) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    let ext = lower.rsplit_once('.')?.1.to_string();
    list.iter().copied().find(|e| *e == ext)
}

/// The Office extension of `name`, if LibreOffice conversion takes it.
pub fn office_extension(name: &str) -> Option<&'static str> {
    extension_in(name, OFFICE)
}

/// The HTML extension of `name`, if browser conversion takes it.
pub fn html_extension(name: &str) -> Option<&'static str> {
    extension_in(name, HTML)
}

// ── finding the programs ──────────────────────────────────────────────────────────────────────

/// A configured program must be an absolute path to a native executable (on Windows an `.exe`
/// or `.com`, never a script or shortcut, which `cmd.exe` would interpret; on Unix a file with an
/// execute bit set).
fn usable(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(windows)]
    {
        path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("exe") || e.eq_ignore_ascii_case("com"))
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(any(unix, windows)))]
    {
        true
    }
}

fn on_path(names: &[&str]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path).filter(|d| d.is_absolute()) {
        for name in names {
            let candidate = dir.join(if cfg!(windows) { format!("{name}.exe") } else { (*name).to_string() });
            if usable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

fn find(explicit: Option<&Path>, what: &str, installed: Vec<PathBuf>, names: &[&str]) -> Result<PathBuf, ExternalError> {
    if let Some(p) = explicit {
        // A configured path that doesn't work is an error, never a reason to look elsewhere.
        return if usable(p) {
            Ok(p.to_path_buf())
        } else {
            Err(ExternalError::Unavailable(format!("the configured {what} ({}) is not a program", p.display())))
        };
    }
    installed.into_iter().find(|p| usable(p)).or_else(|| on_path(names)).ok_or_else(|| ExternalError::Unavailable(format!("{what} is not installed")))
}

fn under(vars: &[&str], rels: &[&str]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for v in vars {
        if let Some(base) = std::env::var_os(v).map(PathBuf::from).filter(|p| p.is_absolute()) {
            out.extend(rels.iter().map(|r| base.join(r)));
        }
    }
    out
}

/// Chrome, Edge or Chromium: `explicit` (Preferences or `PDFCRAFT_CHROME`), then the usual
/// install locations, then `PATH`.
pub fn find_browser(explicit: Option<&Path>) -> Result<PathBuf, ExternalError> {
    let mut installed = under(
        &["ProgramFiles", "ProgramFiles(x86)", "LocalAppData"],
        &[r"Google\Chrome\Application\chrome.exe", r"Microsoft\Edge\Application\msedge.exe", r"Chromium\Application\chrome.exe"],
    );
    installed.extend(
        [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
        ]
        .map(PathBuf::from),
    );
    find(
        explicit,
        "Chrome, Edge or Chromium",
        installed,
        &["google-chrome-stable", "google-chrome", "chromium", "chromium-browser", "microsoft-edge-stable", "microsoft-edge", "chrome", "msedge"],
    )
}

/// LibreOffice: `explicit` (Preferences or `PDFCRAFT_SOFFICE`), then the usual install locations,
/// then `PATH`. On Windows the console launcher `soffice.com` comes first: it waits for the
/// conversion and prints its version.
pub fn find_office(explicit: Option<&Path>) -> Result<PathBuf, ExternalError> {
    let mut installed = under(&["ProgramFiles", "ProgramFiles(x86)"], &[r"LibreOffice\program\soffice.com", r"LibreOffice\program\soffice.exe"]);
    installed.extend(
        ["/Applications/LibreOffice.app/Contents/MacOS/soffice", "/usr/lib/libreoffice/program/soffice", "/opt/libreoffice/program/soffice"]
            .map(PathBuf::from),
    );
    find(explicit, "LibreOffice", installed, &["soffice", "libreoffice"])
}

// ── a private conversion folder ───────────────────────────────────────────────────────────────

/// One conversion's private folder: the input under a fixed name, the program's profile, home and
/// scratch folders, and the output folder. Removed when dropped.
struct Stage {
    dir: tempfile::TempDir,
}

impl Stage {
    fn new() -> Result<Stage, ExternalError> {
        let mut builder = tempfile::Builder::new();
        builder.prefix("plutarchpages-convert-");
        // Owner-only on Unix, where the temporary folder is shared: tempfile otherwise leaves a
        // folder at the umask's mode, and the document staged in it is not for other users.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            builder.permissions(std::fs::Permissions::from_mode(0o700));
        }
        let dir = builder.tempdir().map_err(io("couldn't create a private folder"))?;
        for sub in ["in", "out", "profile", "home", "scratch"] {
            std::fs::create_dir(dir.path().join(sub)).map_err(io("couldn't prepare the private folder"))?;
        }
        Ok(Stage { dir })
    }

    fn path(&self, sub: &str) -> PathBuf {
        self.dir.path().join(sub)
    }

    /// Write `bytes` as `in/input.<ext>`, a name nothing else can have chosen.
    fn write_input(&self, ext: &str, bytes: &[u8]) -> Result<PathBuf, ExternalError> {
        use std::io::Write;
        let path = self.path("in").join(format!("input.{ext}"));
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).map_err(io("couldn't stage the file"))?;
        f.write_all(bytes).map_err(io("couldn't stage the file"))?;
        Ok(path)
    }

    /// Give the program an environment of its own: only the system, locale and font variables it
    /// needs from ours (no credentials, proxies, session addresses or other private values), and
    /// home, configuration, cache and temporary folders inside the stage.
    fn isolate(&self, cmd: &mut Command) {
        let scratch = self.path("scratch");
        let home = self.path("home");
        cmd.env_clear();
        for (key, value) in std::env::vars_os() {
            let Some(k) = key.to_str() else { continue };
            if KEPT_ENV.iter().any(|kept| kept.eq_ignore_ascii_case(k)) || k.starts_with("LC_") {
                cmd.env(&key, value);
            }
        }
        if cfg!(windows) {
            let (roaming, local) = (home.join("AppData").join("Roaming"), home.join("AppData").join("Local"));
            // Best effort: a program that needs them and finds them missing fails visibly.
            let _ = std::fs::create_dir_all(&roaming).and_then(|()| std::fs::create_dir_all(&local));
            cmd.env("TEMP", &scratch).env("TMP", &scratch).env("USERPROFILE", &home).env("APPDATA", roaming).env("LOCALAPPDATA", local);
        } else {
            cmd.env("HOME", &home)
                .env("TMPDIR", &scratch)
                .env("XDG_CONFIG_HOME", home.join(".config"))
                .env("XDG_CACHE_HOME", home.join(".cache"))
                .env("XDG_DATA_HOME", home.join(".local/share"))
                .env("XDG_RUNTIME_DIR", &scratch);
        }
        cmd.current_dir(scratch).stdin(Stdio::null());
    }
}

/// The variables a converter keeps from our environment (plus `LC_*`).
const KEPT_ENV: &[&str] = &[
    "PATH",
    "LANG",
    "LANGUAGE",
    "TZ",
    "FONTCONFIG_FILE",
    "FONTCONFIG_PATH",
    "SystemRoot",
    "SystemDrive",
    "windir",
    "PATHEXT",
    "ProgramFiles",
    "ProgramFiles(x86)",
    "ProgramW6432",
    "CommonProgramFiles",
    "CommonProgramFiles(x86)",
    "CommonProgramW6432",
    "ProgramData",
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE",
    "OS",
];

/// A `file:` URL for an absolute path (a folder when `dir`). The web build has no file paths.
#[cfg(not(target_arch = "wasm32"))]
fn file_url(path: &Path, dir: bool) -> Option<url::Url> {
    if dir { url::Url::from_directory_path(path).ok() } else { url::Url::from_file_path(path).ok() }
}

#[cfg(target_arch = "wasm32")]
fn file_url(_path: &Path, _dir: bool) -> Option<url::Url> {
    None
}

/// The last `n` bytes of `path`, without reading the whole file.
fn tail(path: &Path, n: u64) -> Option<Vec<u8>> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(n))).ok()?;
    let mut out = Vec::new();
    f.take(n).read_to_end(&mut out).ok()?;
    Some(out)
}

fn ends_with_eof(path: &Path) -> bool {
    tail(path, 1024).is_some_and(|t| t.windows(5).any(|w| w == b"%%EOF"))
}

/// Open the result without following a link (Unix), so a link planted at the name can't point
/// the read elsewhere.
fn open_result(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
    }
    options.open(path)
}

/// Read the program's result once: a regular file (not a link) of at most `max` bytes that starts
/// like a PDF and ends with `%%EOF`, all checked on the bytes read. The caller still parses it
/// before showing it.
fn read_output(path: &Path, max: u64) -> Result<Vec<u8>, ExternalError> {
    let file = open_result(path).map_err(|_| ExternalError::InvalidOutput("the program wrote no PDF".into()))?;
    let meta = file.metadata().map_err(io("couldn't read the result"))?;
    if !meta.file_type().is_file() {
        return Err(ExternalError::InvalidOutput("the program's result is not a regular file".into()));
    }
    if meta.len() > max {
        return Err(too_large(max));
    }
    let mut bytes = Vec::new();
    file.take(max.saturating_add(1)).read_to_end(&mut bytes).map_err(io("couldn't read the result"))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > max {
        return Err(too_large(max));
    }
    let head = bytes.get(..bytes.len().min(1024)).unwrap_or_default();
    let tail = bytes.get(bytes.len().saturating_sub(1024)..).unwrap_or_default();
    if !head.windows(5).any(|w| w == b"%PDF-") || !tail.windows(5).any(|w| w == b"%%EOF") {
        return Err(ExternalError::InvalidOutput("the program's result is not a complete PDF".into()));
    }
    Ok(bytes)
}

fn too_large(max: u64) -> ExternalError {
    ExternalError::InvalidOutput(format!("the result is larger than {} MB", max >> 20))
}

/// How many entries, and how deep, [`bytes_under`] measures before it gives up. A fresh
/// LibreOffice profile holds a few hundred files, a few folders deep.
const MAX_STAGE_ENTRIES: usize = 10_000;
const MAX_STAGE_DEPTH: usize = 32;

/// The bytes in the regular files under `dirs`, links not followed. Anything unreadable counts as
/// nothing and the sum saturates. Each size is read from the file itself: on Windows a directory
/// listing keeps the old size while a process still has the file open. More than
/// [`MAX_STAGE_ENTRIES`] entries in all, or folders deeper than [`MAX_STAGE_DEPTH`], count as
/// `u64::MAX`, past any limit, so the walk stays cheap and the program that made them is stopped.
/// `stop` is asked before each folder and every 256 entries; once it is true the walk returns what
/// it has counted, so a cancel or a deadline is not held up by it.
fn bytes_under(dirs: &[PathBuf], stop: &dyn Fn() -> bool) -> u64 {
    let (mut pending, mut sum, mut seen): (Vec<_>, _, _) = (dirs.iter().map(|d| (d.clone(), 0)).collect(), 0u64, 0usize);
    while let Some((dir, depth)) = pending.pop() {
        if stop() {
            return sum;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            seen += 1;
            if seen > MAX_STAGE_ENTRIES {
                return u64::MAX;
            }
            if seen % 256 == 0 && stop() {
                return sum;
            }
            let path = entry.path();
            match std::fs::symlink_metadata(&path) {
                Ok(meta) if meta.is_dir() && depth < MAX_STAGE_DEPTH => pending.push((path, depth + 1)),
                Ok(meta) if meta.is_dir() => return u64::MAX,
                Ok(meta) if meta.is_file() => sum = sum.saturating_add(meta.len()),
                _ => {}
            }
        }
    }
    sum
}

fn check_input(bytes: &[u8], limits: &Limits) -> Result<(), ExternalError> {
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limits.max_input {
        return Err(ExternalError::InvalidInput(format!("the file is larger than {} MB", limits.max_input >> 20)));
    }
    Ok(())
}

// ── the browser ───────────────────────────────────────────────────────────────────────────────

/// The browser's command line for printing `target` to `out`. `offline`: scripts off and every
/// network request refused, for local files.
fn browser_args(profile: &Path, out: &Path, target: &str, offline: bool) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "--headless=new",
        "--no-first-run",
        "--no-default-browser-check",
        "--disable-extensions",
        "--disable-sync",
        "--disable-background-networking",
        "--disable-component-update",
        "--disable-default-apps",
        "--deny-permission-prompts",
        "--mute-audio",
        "--no-pdf-header-footer",
    ]
    .map(OsString::from)
    .into();
    let mut user_data = OsString::from("--user-data-dir=");
    user_data.push(profile);
    args.push(user_data);
    if offline {
        args.extend(
            [
                "--blink-settings=scriptEnabled=false",
                "--host-resolver-rules=MAP * ~NOTFOUND",
                "--proxy-server=http://127.0.0.1:9",
                "--proxy-bypass-list=<-loopback>",
            ]
            .map(OsString::from),
        );
    }
    let mut print = OsString::from("--print-to-pdf=");
    print.push(out);
    args.push(print);
    // Nothing after `--` is read as a switch.
    args.push("--".into());
    args.push(target.into());
    args
}

fn run_browser(browser: &Path, target: &str, offline: bool, stage: &Stage, limits: &Limits, cancel: &AtomicBool) -> Result<Vec<u8>, ExternalError> {
    let out = stage.path("out").join("out.pdf");
    let mut cmd = Command::new(browser);
    cmd.args(browser_args(&stage.path("profile"), &out, target, offline));
    stage.isolate(&mut cmd);
    let mut child = supervise::spawn(cmd, "the browser")?;
    // Some browsers finish writing the PDF and then linger: done once the file is complete and
    // has stopped growing, or once it is too large (stopped at once; the read then refuses it).
    let mut last = None;
    let done = || {
        let len = std::fs::metadata(&out).map(|m| m.len()).ok();
        let stable = len.is_some() && len == last && ends_with_eof(&out);
        last = len;
        stable || len.is_some_and(|l| l > limits.max_output)
    };
    match supervise::wait(&mut child, Instant::now() + limits.timeout, cancel, done) {
        Ended::Exited(status) if !status.success() => {
            Err(ExternalError::ProcessFailed(format!("the browser stopped ({status}) without finishing the PDF")))
        }
        Ended::Exited(_) | Ended::Done => read_output(&out, limits.max_output),
        Ended::TimedOut => Err(ExternalError::TimedOut("the browser".into(), limits.timeout.as_secs())),
        Ended::Cancelled => Err(ExternalError::Cancelled),
    }
}

/// Print a local HTML file (named `name`, an [`HTML`] extension) to PDF with the browser at
/// `browser`, with scripts off and no network.
pub fn html_to_pdf(browser: &Path, name: &str, bytes: &[u8], limits: &Limits, cancel: &AtomicBool) -> Result<Vec<u8>, ExternalError> {
    let ext = html_extension(name).ok_or_else(|| ExternalError::InvalidInput(format!("{name} is not an HTML file")))?;
    check_input(bytes, limits)?;
    let stage = Stage::new()?;
    let input = stage.write_input(ext, bytes)?;
    let url = file_url(&input, false).ok_or_else(|| ExternalError::Io("couldn't address the staged file".into()))?;
    let result = run_browser(browser, url.as_str(), true, &stage, limits, cancel);
    let _ = stage.dir.close();
    result
}

/// Print the web page at `url` (http or https, see [`checked_web_url`]) to PDF with the browser at
/// `browser`. The page loads as in a fresh, private browser window: scripts and network on.
pub fn url_to_pdf(browser: &Path, url: &str, limits: &Limits, cancel: &AtomicBool) -> Result<Vec<u8>, ExternalError> {
    let url = checked_web_url(url)?;
    let stage = Stage::new()?;
    let result = run_browser(browser, url.as_str(), false, &stage, limits, cancel);
    let _ = stage.dir.close();
    result
}

// ── LibreOffice ───────────────────────────────────────────────────────────────────────────────

/// The fresh profile's settings: no macros at all, no links to untrusted locations, and
/// linked content in Writer and Calc documents (sections, files, external data) never
/// updated on load, so a document can't make LibreOffice fetch from the network or a share.
/// The two `Link` settings number "Never" differently (Writer.xcs: 0 Always, 1 On request,
/// 2 Never; Calc.xcs: 0 Always, 1 Never, 2 On request). Impress has no such setting.
/// From upstream's `crates/create/src/office.rs`.
const PROFILE_SETTINGS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<oor:items xmlns:oor="http://openoffice.org/2001/registry" xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
<item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="DisableMacrosExecution" oor:op="fuse"><value>true</value></prop></item>
<item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="MacroSecurityLevel" oor:op="fuse"><value>3</value></prop></item>
<item oor:path="/org.openoffice.Office.Common/Security/Scripting"><prop oor:name="BlockUntrustedRefererLinks" oor:op="fuse"><value>true</value></prop></item>
<item oor:path="/org.openoffice.Office.Writer/Content/Update"><prop oor:name="Link" oor:op="fuse"><value>2</value></prop></item>
<item oor:path="/org.openoffice.Office.Calc/Content/Update"><prop oor:name="Link" oor:op="fuse"><value>1</value></prop></item>
</oor:items>
"#;

/// A fresh LibreOffice profile in `stage`, holding [`PROFILE_SETTINGS`], as the
/// `-env:UserInstallation` argument.
fn profile_arg(stage: &Stage) -> Result<OsString, ExternalError> {
    let user = stage.path("profile").join("user");
    std::fs::create_dir(&user)
        .and_then(|()| std::fs::write(user.join("registrymodifications.xcu"), PROFILE_SETTINGS))
        .map_err(io("couldn't prepare the LibreOffice profile"))?;
    let url = file_url(&stage.path("profile"), true).ok_or_else(|| ExternalError::Io("couldn't address the profile folder".into()))?;
    Ok(OsString::from(format!("-env:UserInstallation={url}")))
}

/// A program version: (major, minor, patch).
type Version = (u32, u32, u32);

/// `LibreOffice 26.2.5.1 …` → (26, 2, 5).
fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    let word = text.split_whitespace().find(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()) && w.contains('.'))?;
    let mut parts = word.split('.').map(|p| p.parse::<u32>().ok());
    Some((parts.next()??, parts.next()??, parts.next().flatten().unwrap_or(0)))
}

/// The installed LibreOffice's version: Windows installs record it in `program/version.ini`;
/// elsewhere `soffice --version` prints it.
fn office_version(soffice: &Path) -> Result<(u32, u32, u32), ExternalError> {
    if let Some(ini) = soffice.parent().map(|p| p.join("version.ini"))
        && let Ok(text) = std::fs::read_to_string(&ini)
        && let Some(v) = text.lines().find_map(|l| l.strip_prefix("MsiProductVersion=")).and_then(parse_version)
    {
        return Ok(v);
    }
    let stage = Stage::new()?;
    let out_path = stage.path("scratch").join("version.txt");
    let out = std::fs::File::create(&out_path).map_err(io("couldn't ask LibreOffice for its version"))?;
    let mut cmd = Command::new(soffice);
    cmd.arg(profile_arg(&stage)?).arg("--version");
    stage.isolate(&mut cmd);
    cmd.stdout(out);
    let mut child = supervise::spawn_with_stdout(cmd, "LibreOffice").map_err(|e| ExternalError::Unavailable(e.to_string()))?;
    let ended = supervise::wait(&mut child, Instant::now() + Duration::from_secs(30), &AtomicBool::new(false), || false);
    let text =
        std::fs::read(&out_path).map(|b| String::from_utf8_lossy(b.get(..b.len().min(4096)).unwrap_or_default()).into_owned()).unwrap_or_default();
    let _ = stage.dir.close();
    match ended {
        Ended::Exited(_) => {
            parse_version(&text).ok_or_else(|| ExternalError::Unavailable("couldn't tell which LibreOffice version is installed".into()))
        }
        _ => Err(ExternalError::Unavailable("LibreOffice didn't report its version".into())),
    }
}

/// Refuse a LibreOffice older than [`MIN_LIBREOFFICE`]. The answer is remembered per program.
fn check_office_version(soffice: &Path) -> Result<(), ExternalError> {
    static SEEN: OnceLock<Mutex<HashMap<PathBuf, Version>>> = OnceLock::new();
    let seen = SEEN.get_or_init(Mutex::default);
    let known = seen.lock().ok().and_then(|m| m.get(soffice).copied());
    let version = match known {
        Some(v) => v,
        None => {
            let v = office_version(soffice)?;
            if let Ok(mut m) = seen.lock() {
                m.insert(soffice.to_path_buf(), v);
            }
            v
        }
    };
    if version < MIN_LIBREOFFICE {
        let (a, b, c) = version;
        let (x, y, z) = MIN_LIBREOFFICE;
        return Err(ExternalError::Unavailable(format!("LibreOffice {a}.{b}.{c} has known security problems; update to {x}.{y}.{z} or later")));
    }
    Ok(())
}

/// Convert an Office file (named `name`, an [`OFFICE`] extension) to PDF with the LibreOffice at
/// `soffice`, in a fresh profile so it never touches the user's own LibreOffice settings.
pub fn office_to_pdf(soffice: &Path, name: &str, bytes: &[u8], limits: &Limits, cancel: &AtomicBool) -> Result<Vec<u8>, ExternalError> {
    let ext = office_extension(name).ok_or_else(|| ExternalError::InvalidInput(format!("{name} is not an Office file LibreOffice converts here")))?;
    check_input(bytes, limits)?;
    check_office_version(soffice)?;
    let stage = Stage::new()?;
    let input = stage.write_input(ext, bytes)?;
    let mut cmd = Command::new(soffice);
    cmd.arg(profile_arg(&stage)?)
        .args(["--headless", "--norestore", "--nologo", "--nodefault", "--convert-to", "pdf", "--outdir"])
        .arg(stage.path("out"))
        .arg(&input);
    stage.isolate(&mut cmd);
    let out = stage.path("out").join("input.pdf");
    let result = (|| {
        let mut child = supervise::spawn(cmd, "LibreOffice")?;
        // Stopped at once when what it writes anywhere in its stage grows too large: the result,
        // or a file under another name (a result may be written as one first), its profile and
        // home included. Only the staged input is left out. The walk gives way to a cancel or the
        // deadline, which `wait` then acts on.
        let deadline = Instant::now() + limits.timeout;
        let watched: Vec<PathBuf> = ["out", "scratch", "profile", "home"].iter().map(|sub| stage.path(sub)).collect();
        let stop = || cancel.load(std::sync::atomic::Ordering::Relaxed) || Instant::now() >= deadline;
        match supervise::wait(&mut child, deadline, cancel, || bytes_under(&watched, &stop) > limits.max_output) {
            Ended::Exited(status) if !status.success() => {
                Err(ExternalError::ProcessFailed(format!("LibreOffice stopped ({status}) without finishing the PDF")))
            }
            Ended::Exited(_) => read_output(&out, limits.max_output),
            Ended::Done => Err(ExternalError::InvalidOutput(format!(
                "what LibreOffice wrote is larger than {} MB, or holds more files or folders than allowed",
                limits.max_output >> 20
            ))),
            Ended::TimedOut => Err(ExternalError::TimedOut("LibreOffice".into(), limits.timeout.as_secs())),
            Ended::Cancelled => Err(ExternalError::Cancelled),
        }
    })();
    let _ = stage.dir.close();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    mod fork_tests;

    #[test]
    fn extensions_are_allowlisted() {
        assert_eq!(office_extension("Report.DOCX"), Some("docx"));
        assert_eq!(office_extension("macro.docm"), None);
        assert_eq!(office_extension("no-extension"), None);
        assert_eq!(html_extension("page.HTM"), Some("htm"));
        assert_eq!(html_extension("page.html.exe"), None);
    }

    #[test]
    fn the_profile_turns_off_macros_and_never_updates_links() {
        let stage = Stage::new().unwrap();
        profile_arg(&stage).unwrap();
        let written = std::fs::read_to_string(stage.path("profile").join("user").join("registrymodifications.xcu")).unwrap();
        let between = |s: &str, start: &str, end: &str| s.split_once(start).and_then(|(_, rest)| rest.split_once(end)).map(|(v, _)| v.to_string());
        // Each setting, as (path, property, value).
        let settings: Vec<(String, String, String)> = written
            .split("<item ")
            .skip(1)
            .filter_map(|item| {
                Some((between(item, "oor:path=\"", "\"")?, between(item, "oor:name=\"", "\"")?, between(item, "<value>", "</value>")?))
            })
            .collect();
        let has = |path: &str, name: &str, value: &str| settings.iter().any(|(p, n, v)| p == path && n == name && v == value);
        let scripting = "/org.openoffice.Office.Common/Security/Scripting";
        assert!(has(scripting, "DisableMacrosExecution", "true"), "{settings:?}");
        assert!(has(scripting, "MacroSecurityLevel", "3"), "{settings:?}");
        assert!(has(scripting, "BlockUntrustedRefererLinks", "true"), "{settings:?}");
        // "Never" is 2 for Writer but 1 for Calc (Writer.xcs and Calc.xcs); Writer's 1 is
        // its default, "On request".
        assert!(has("/org.openoffice.Office.Writer/Content/Update", "Link", "2"), "{settings:?}");
        assert!(has("/org.openoffice.Office.Calc/Content/Update", "Link", "1"), "{settings:?}");
        assert_eq!(settings.len(), 5, "{settings:?}");
        let _ = stage.dir.close();
    }

    #[test]
    fn versions_parse() {
        assert_eq!(parse_version("LibreOffice 26.2.5.1 420(Build:1)"), Some((26, 2, 5)));
        assert_eq!(parse_version("26.8.0.2"), Some((26, 8, 0)));
        assert_eq!(parse_version("LibreOffice 7.6"), Some((7, 6, 0)));
        assert_eq!(parse_version("no version here"), None);
        assert!((25, 8, 7) < MIN_LIBREOFFICE && (26, 2, 5) >= MIN_LIBREOFFICE && (26, 8, 0) > MIN_LIBREOFFICE);
    }

    #[test]
    fn a_configured_program_must_be_an_absolute_executable() {
        assert!(matches!(find_browser(Some(Path::new("chrome"))), Err(ExternalError::Unavailable(_))), "relative paths are refused");
        let dir = tempfile::tempdir().unwrap();
        assert!(find_office(Some(dir.path())).is_err(), "a folder is not a program");
        let script = dir.path().join(if cfg!(windows) { "soffice.cmd" } else { "missing" });
        assert!(find_office(Some(&script)).is_err(), "a script (Windows) or a missing file is refused");
    }

    /// The browser's arguments: the target is the last argument, after `--`, so it can't be read
    /// as a switch, and local files get scripts off and no network.
    #[test]
    fn browser_arguments_keep_the_target_inert() {
        let args = browser_args(Path::new("/p"), Path::new("/o/out.pdf"), "--enable-automation", true);
        let s: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(s.iter().rev().take(2).cloned().collect::<Vec<_>>(), vec!["--enable-automation".to_string(), "--".into()]);
        for flag in [
            "--blink-settings=scriptEnabled=false",
            "--host-resolver-rules=MAP * ~NOTFOUND",
            "--proxy-server=http://127.0.0.1:9",
            "--user-data-dir=/p",
        ] {
            assert!(s.iter().any(|a| a == flag), "{flag} missing: {s:?}");
        }
        assert!(s.iter().all(|a| a != "--no-sandbox" && a != "--disable-web-security"));
        let online = browser_args(Path::new("/p"), Path::new("/o/out.pdf"), "https://example.org/", false);
        assert!(!online.iter().any(|a| a.to_string_lossy().contains("scriptEnabled")));
    }

    /// A fake converter that writes a fake end marker and hangs, and one that writes nothing: both
    /// end in a controlled error, never a hang.
    #[cfg(unix)]
    #[test]
    fn fake_converters_fail_cleanly() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let fake = |name: &str, body: &str| {
            let p = dir.path().join(name);
            std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p
        };
        let limits = Limits { timeout: Duration::from_secs(3), ..Limits::default() };
        let cancel = AtomicBool::new(false);
        // Writes "%%EOF" into the --print-to-pdf target without a PDF header, then hangs.
        let liar = fake(
            "liar",
            r#"for a in "$@"; do case "$a" in --print-to-pdf=*) printf 'not a pdf %%%%EOF' > "${a#--print-to-pdf=}";; esac; done; sleep 30"#,
        );
        let r = html_to_pdf(&liar, "p.html", b"<p>x</p>", &limits, &cancel);
        assert!(matches!(r, Err(ExternalError::InvalidOutput(_))), "{r:?}");
        let silent = fake("silent", "exit 0");
        let r = html_to_pdf(&silent, "p.html", b"<p>x</p>", &limits, &cancel);
        assert!(matches!(r, Err(ExternalError::InvalidOutput(_))), "{r:?}");
        let hang = fake("hang", "sleep 30");
        let started = Instant::now();
        let r = url_to_pdf(&hang, "https://example.org/", &Limits { timeout: Duration::from_secs(1), ..limits }, &cancel);
        assert!(matches!(r, Err(ExternalError::TimedOut(..))) && started.elapsed() < Duration::from_secs(10), "{r:?}");
        assert!(matches!(url_to_pdf(&hang, "file:///etc/passwd", &limits, &cancel), Err(ExternalError::InvalidInput(_))));
    }

    /// Final-review finding F9: a failed exit, a result that keeps growing and a result planted as
    /// a link are all refused, and a growing result stops the program at once.
    #[cfg(unix)]
    #[test]
    fn failed_growing_and_linked_results_are_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let fake = |name: &str, body: &str| {
            let p = dir.path().join(name);
            let script =
                format!("#!/bin/sh\nfor a in \"$@\"; do case \"$a\" in --print-to-pdf=*) out=\"${{a#--print-to-pdf=}}\";; esac; done\n{body}\n");
            std::fs::write(&p, script).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p
        };
        let cancel = AtomicBool::new(false);
        let limits = Limits { timeout: Duration::from_secs(20), max_input: 1 << 20, max_output: 4096 };
        let pdf = dir.path().join("good.pdf");
        std::fs::write(&pdf, "%PDF-1.7\n1 0 obj<<>>endobj\ntrailer<<>>\n%%EOF\n").unwrap();
        let failed = fake("failed", &format!("cat '{}' > \"$out\"; exit 3", pdf.display()));
        let r = html_to_pdf(&failed, "p.html", b"<p>x</p>", &limits, &cancel);
        assert!(matches!(r, Err(ExternalError::ProcessFailed(_))), "{r:?}");
        let growing = fake("growing", "head -c 100000 /dev/zero > \"$out\"; sleep 30");
        let started = Instant::now();
        let r = html_to_pdf(&growing, "p.html", b"<p>x</p>", &limits, &cancel);
        assert!(matches!(r, Err(ExternalError::InvalidOutput(_))) && started.elapsed() < Duration::from_secs(10), "{r:?}");
        let linked = fake("linked", &format!("ln -s '{}' \"$out\"; exit 0", pdf.display()));
        let r = html_to_pdf(&linked, "p.html", b"<p>x</p>", &limits, &cancel);
        assert!(matches!(r, Err(ExternalError::InvalidOutput(_))), "{r:?}");
        let fine = fake("fine", &format!("cat '{}' > \"$out\"; exit 0", pdf.display()));
        assert!(html_to_pdf(&fine, "p.html", b"<p>x</p>", &limits, &cancel).is_ok());
    }

    /// Final-review finding F10: a converter sees only the allowlisted variables of our
    /// environment, with home and temporary folders inside its stage.
    #[cfg(unix)]
    #[test]
    fn converters_get_an_allowlisted_environment() {
        use std::os::unix::fs::PermissionsExt;
        // Cargo sets this for test runs; it stands in for any private variable of ours.
        if std::env::var_os("CARGO_MANIFEST_DIR").is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let seen = dir.path().join("env.txt");
        let p = dir.path().join("printenv");
        std::fs::write(&p, format!("#!/bin/sh\nenv > '{}'\nexit 1\n", seen.display())).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _ = html_to_pdf(&p, "p.html", b"<p>x</p>", &Limits::default(), &AtomicBool::new(false));
        let env = std::fs::read_to_string(&seen).unwrap();
        assert!(!env.contains("CARGO_MANIFEST_DIR="), "{env}");
        assert!(env.lines().any(|l| l.starts_with("PATH=")), "{env}");
        let home = env.lines().find_map(|l| l.strip_prefix("HOME="));
        assert!(home.is_some_and(|h| h.ends_with("/home")) && home != std::env::var("HOME").ok().as_deref(), "{home:?}");
    }

    /// The size walk counts every folder it is given against one entry budget, and gives way at
    /// once to a cancel or the deadline (`stop`).
    #[test]
    fn the_size_walk_has_one_budget_and_gives_way() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        let half = MAX_STAGE_ENTRIES / 2 + 1;
        for d in [&a, &b] {
            std::fs::create_dir(d).unwrap();
            for i in 0..half {
                std::fs::write(d.join(i.to_string()), b"x").unwrap();
            }
        }
        assert_eq!(bytes_under(std::slice::from_ref(&a), &|| false), half as u64);
        assert_eq!(bytes_under(&[a.clone(), b.clone()], &|| false), u64::MAX, "one budget for both");
        let asked = std::cell::Cell::new(0);
        let stop_on_second_ask = || {
            asked.set(asked.get() + 1);
            asked.get() == 2
        };
        assert_eq!(bytes_under(std::slice::from_ref(&a), &stop_on_second_ask), 255, "asked again at entry 256");
        assert_eq!(bytes_under(&[a, b], &|| true), 0, "stopped before the first folder");
    }

    /// The temporary folder is shared on Unix: a conversion's folder, and the document staged in
    /// it, are for its owner alone.
    #[cfg(unix)]
    #[test]
    fn the_conversion_folder_is_the_owners_alone() {
        use std::os::unix::fs::PermissionsExt;
        let stage = Stage::new().unwrap();
        let mode = std::fs::metadata(stage.dir.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "{mode:o}");
    }

    /// LibreOffice may write its result under another name first, in any folder of its stage:
    /// growth there stops it too, with a size error, as growth of the result does, and so do
    /// more folders or files than the watcher measures cheaply.
    #[cfg(unix)]
    #[test]
    fn an_office_conversion_growing_under_another_name_is_stopped() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        // Read as the version, so no `--version` run is needed.
        std::fs::write(dir.path().join("version.ini"), "MsiProductVersion=26.2.5.2\n").unwrap();
        let limits = Limits { timeout: Duration::from_secs(20), max_input: 1 << 20, max_output: 4096 };
        let cancel = AtomicBool::new(false);
        let writes = [
            "head -c 100000 /dev/zero > \"$outdir/part.tmp\"",
            "head -c 100000 /dev/zero > \"$TMPDIR/lu1.tmp\"",
            "head -c 100000 /dev/zero > \"$HOME/lu2.tmp\"",
            // Too many folders or files to measure cheaply: stopped as well.
            "d=\"$TMPDIR\"; i=0; while [ $i -lt 80 ]; do d=\"$d/d\"; mkdir \"$d\"; i=$((i+1)); done",
            "i=0; while [ $i -lt 12000 ]; do : > \"$outdir/f$i\"; i=$((i+1)); done",
            // The count is for the whole stage, not each folder.
            "i=0; while [ $i -lt 4000 ]; do : > \"$outdir/f$i\"; : > \"$TMPDIR/f$i\"; : > \"$HOME/f$i\"; i=$((i+1)); done",
        ];
        for write in writes {
            let soffice = dir.path().join("soffice");
            let args = "prev=; for a in \"$@\"; do [ \"$prev\" = --outdir ] && outdir=\"$a\"; prev=\"$a\"; done";
            std::fs::write(&soffice, format!("#!/bin/sh\n{args}\n{write}\nsleep 30\n")).unwrap();
            std::fs::set_permissions(&soffice, std::fs::Permissions::from_mode(0o755)).unwrap();
            let started = Instant::now();
            let r = office_to_pdf(&soffice, "letter.docx", b"PK", &limits, &cancel);
            assert!(
                matches!(&r, Err(ExternalError::InvalidOutput(m)) if m.contains("larger")) && started.elapsed() < Duration::from_secs(15),
                "{write}: {r:?}"
            );
        }
    }
}
