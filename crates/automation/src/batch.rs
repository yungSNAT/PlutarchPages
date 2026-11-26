//! One-shot folder jobs shared by CLI and MCP. Discovery finishes before any output is made.

use std::collections::{BTreeMap, HashSet};
#[cfg(windows)]
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pdfcraft_engine::{Session, ocr, optimize};
use serde_json::{Value, json};

use crate::{Args, Automation, Result, failed};

const MAX_ENTRIES: usize = 10_000;
const MAX_DEPTH: usize = 64;
const MAX_INPUT: u64 = 512 * 1024 * 1024;

struct Recipe<'a> {
    steps: Vec<&'a str>,
    images: optimize::ImageSettings,
    mono: optimize::MonoSettings,
    ocr: ocr::OcrSettings,
    /// The calling session's JavaScript preference, carried into each file's own session.
    javascript: bool,
}

impl<'a> Recipe<'a> {
    /// Read and validate `doc_batch`'s steps and settings, taking the JavaScript preference from
    /// `auto`'s session.
    fn parse(auto: &Automation, args: &'a Args) -> Result<Self> {
        let steps = args.strs("steps")?;
        if steps.is_empty() || steps.len() > 3 {
            return Err(Args::wrong("steps", "one to three unique steps: convert, ocr, optimize"));
        }
        let mut seen = HashSet::new();
        for (i, step) in steps.iter().enumerate() {
            if !matches!(*step, "convert" | "ocr" | "optimize") || !seen.insert(*step) || (*step == "convert" && i != 0) {
                return Err(Args::wrong("steps", "unique convert/ocr/optimize steps, with convert first when present"));
            }
        }
        for (keys, step) in [(&["ppi", "quality"][..], "optimize"), (&["dpi", "language", "skip_text_pages"][..], "ocr")] {
            if !steps.contains(&step) && keys.iter().any(|k| args.get(k).is_some()) {
                return Err(failed(format!("{} settings need the {step} step", keys.join("/"))));
            }
        }
        let mut images = optimize::ImageSettings::REDUCE;
        let mut mono = optimize::MonoSettings::REDUCE;
        if let Some(ppi) = args.opt_num("ppi")? {
            if !(9.0..=1600.0).contains(&ppi) {
                return Err(Args::wrong("ppi", "9 to 1600"));
            }
            images.target_ppi = ppi;
            images.above_ppi = ppi * 1.5;
            // Black-and-white images keep Reduce File Size's higher resolution, at which text
            // stays legible, and are never taken below a higher `ppi` asked for.
            mono.target_ppi = mono.target_ppi.max(ppi);
            mono.above_ppi = mono.target_ppi * 1.5;
        }
        if let Some(q) = args.opt_int("quality")? {
            if !(1..=100).contains(&q) {
                return Err(Args::wrong("quality", "1 to 100"));
            }
            images.compression = optimize::Compression::Jpeg(q as u8);
        }
        if args.opt_num("dpi")?.is_some_and(|dpi| !(72.0..=600.0).contains(&dpi)) {
            return Err(Args::wrong("dpi", "72 to 600"));
        }
        Ok(Self { steps, images, mono, ocr: auto.ocr_settings(args)?, javascript: auto.session.javascript() })
    }

    fn accepts(&self, path: &Path) -> bool {
        let extension = path.extension().and_then(std::ffi::OsStr::to_str).unwrap_or("").to_ascii_lowercase();
        extension == "pdf" || (self.steps.contains(&"convert") && pdfcraft_engine::CONVERTIBLE.contains(&extension.as_str()))
    }

    /// Run the recipe's steps on the file at `path` in a fresh session; returns the output bytes
    /// and a report entry per step.
    fn process(&self, path: &Path) -> Result<(Arc<Vec<u8>>, Vec<Value>)> {
        // A fresh session per file bounds retained document state and never touches caller tabs.
        // It keeps the caller's JavaScript preference, set before any open runs open-time scripts.
        let mut session = Session::new();
        session.set_javascript(self.javascript);
        let mut input = Vec::new();
        std::fs::File::open(path).map_err(failed)?.take(MAX_INPUT + 1).read_to_end(&mut input).map_err(failed)?;
        if input.len() as u64 > MAX_INPUT {
            return Err(failed("input exceeds 512 MiB"));
        }
        let name = path.file_name().and_then(std::ffi::OsStr::to_str).ok_or_else(|| failed("input name is not UTF-8"))?;
        let mut bytes = Arc::new(input);
        let mut log = Vec::new();
        for step in &self.steps {
            let before = bytes.len();
            match *step {
                "convert" => bytes = session.convert_to_pdf(name, &bytes)?.1,
                "ocr" => {
                    let recognizers = ocr::recognizers(&self.ocr).map_err(failed)?;
                    let result =
                        ocr::recognize_file(name, bytes, None, self.ocr.clone(), &recognizers, self.javascript, |_, _| true).map_err(failed)?;
                    log.push(
                        json!({"step":"ocr","words":result.words(),"skipped_pages":result.pages.iter().filter(|p| p.skipped.is_some()).count()}),
                    );
                    bytes = result.bytes;
                    continue;
                }
                "optimize" => {
                    let id = session.open(name, None, bytes.clone(), None).map_err(failed)?;
                    let settings = optimize::Settings { color: self.images, gray: self.images, mono: self.mono, ..Default::default() };
                    let (candidate, _) = session.optimized_bytes(id, &settings, &[])?;
                    session.close(id);
                    if candidate.len() < bytes.len() {
                        bytes = candidate;
                    }
                }
                _ => return Err(failed("unknown batch step")),
            }
            log.push(json!({"step":step,"bytes_before":before,"bytes_after":bytes.len()}));
        }
        Ok((bytes, log))
    }
}

// Windows junctions are reparse points too; do not descend into them as ordinary folders.
fn linked(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

/// Canonicalize the existing ancestor, including when the requested output does not exist yet.
pub(crate) fn absolute(path: &Path) -> Result<PathBuf> {
    let mut ancestor = path;
    let mut tail = Vec::new();
    while !ancestor.exists() {
        if ancestor.symlink_metadata().is_ok() {
            return Err(failed("broken link in batch path"));
        }
        tail.push(ancestor.file_name().ok_or_else(|| failed("invalid batch path"))?);
        ancestor = ancestor.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    }
    let mut path = ancestor.canonicalize().map_err(failed)?;
    path.extend(tail.iter().rev());
    crate::lexical(&path).ok_or_else(|| failed("invalid batch path"))
}

struct Discovery {
    files: BTreeMap<PathBuf, PathBuf>,
    skipped: Vec<Value>,
    seen: HashSet<PathBuf>,
    visited: usize,
}

impl Discovery {
    fn walk(&mut self, path: &Path, relative: &Path, out: &Path, recursive: bool, depth: usize) -> Result<()> {
        self.visited += 1;
        if self.visited > MAX_ENTRIES || depth > MAX_DEPTH {
            return Err(failed("batch discovery exceeds 10000 entries or 64 folder levels"));
        }
        if depth > 0 && path.starts_with(out) {
            return Ok(());
        }
        let metadata = path.symlink_metadata().map_err(|e| failed(format!("{}: {e}", path.display())))?;
        if linked(&metadata) || (!metadata.is_file() && !metadata.is_dir()) {
            self.skipped.push(json!({"path":path.to_string_lossy(),"status":"skipped","reason":"link or special file"}));
        } else if (metadata.is_file() || depth == 0 || recursive) && self.seen.insert(path.to_path_buf()) {
            if metadata.is_file() {
                self.files.insert(path.to_path_buf(), relative.to_path_buf());
            } else if depth == 0 || recursive {
                // Bound the list before sorting it; never collect an unbounded folder listing.
                let mut entries = Vec::new();
                for entry in std::fs::read_dir(path).map_err(failed)? {
                    if entries.len() >= MAX_ENTRIES.saturating_sub(self.visited) {
                        return Err(failed("batch discovery exceeds 10000 entries"));
                    }
                    entries.push(entry.map_err(failed)?.path());
                }
                entries.sort();
                for child in entries {
                    let name = child.file_name().ok_or_else(|| failed("entry has no name"))?;
                    self.walk(&child, &relative.join(name), out, recursive, depth + 1)?;
                }
            }
        }
        Ok(())
    }
}

impl Automation {
    pub(crate) fn doc_batch(&self, args: &Args) -> Result<Value> {
        let recipe = Recipe::parse(self, args)?;
        let recursive = args.opt_bool("recursive")?.unwrap_or(false);
        let stop = args.opt_bool("stop_on_error")?.unwrap_or(false);
        let dry_run = args.opt_bool("dry_run")?.unwrap_or(false);
        let paths = args.strs("paths")?;
        if paths.is_empty() || paths.len() > 1000 {
            return Err(Args::wrong("paths", "1 to 1000 files or folders"));
        }
        let out = absolute(&self.resolve(args.str("out_dir")?, true)?)?;
        let mut roots = Vec::new();
        for path in paths {
            let resolved = self.resolve(path, false)?;
            let original = self.root.as_ref().map_or_else(|| PathBuf::from(path), |root| root.join(path));
            if linked(&original.symlink_metadata().map_err(failed)?) {
                return Err(failed("input roots must be regular files or folders, not links"));
            }
            let real = absolute(&resolved)?;
            if real.is_dir() && real.starts_with(&out) {
                return Err(failed("out_dir must not contain an input folder or equal it"));
            }
            roots.push(real);
        }
        let mut discovery = Discovery { files: BTreeMap::new(), skipped: Vec::new(), seen: HashSet::new(), visited: 0 };
        for root in roots {
            let relative = if root.is_dir() { PathBuf::new() } else { PathBuf::from(root.file_name().ok_or_else(|| failed("input has no name"))?) };
            discovery.walk(&root, &relative, &out, recursive, 0)?;
        }
        let mut files = discovery.skipped;
        let (mut completed, mut failed_count) = (0, 0);
        let mut stopped = false;
        let mut planned_outputs: HashSet<PathBuf> = HashSet::new();
        for (path, relative) in discovery.files {
            if stopped || !recipe.accepts(&path) {
                files.push(json!({"path":path.to_string_lossy(),"status":"skipped","reason":if stopped {"stopped after error"} else {"unsupported input extension"}}));
                continue;
            }
            if dry_run {
                let planned: Result<PathBuf> = (|| {
                    let target = out.join(&relative).with_extension("pdf");
                    let target = self.resolve(target.to_str().ok_or_else(|| failed("output path is not UTF-8"))?, true)?;
                    if !target.starts_with(&out) {
                        return Err(failed("output path leaves out_dir"));
                    }
                    let dir = target.parent().ok_or_else(|| failed("output has no parent"))?;
                    let stem = target.file_stem().ok_or_else(|| failed("output has no stem"))?.to_string_lossy().into_owned();
                    for n in 1..=10_000 {
                        let candidate = if n == 1 { target.clone() } else { dir.join(format!("{stem} ({n}).pdf")) };
                        if candidate.symlink_metadata().is_ok() || planned_outputs.contains(&candidate) {
                            continue;
                        }
                        planned_outputs.insert(candidate.clone());
                        return Ok(candidate);
                    }
                    Err(failed("all 10000 output names are occupied"))
                })();
                match planned {
                    Ok(target) => files.push(json!({"path":path.to_string_lossy(),"status":"planned","output":target.to_string_lossy().replace('\\', "/"),"steps":recipe.steps})),
                    Err(e) => {
                        failed_count += 1;
                        files.push(json!({"path":path.to_string_lossy(),"status":"error","error":e.to_string()}));
                        stopped = stop;
                    }
                }
                continue;
            }
            // Recheck confinement and links before reading a discovered file.
            let result: Result<_> = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let current = self.resolve(path.to_str().ok_or_else(|| failed("input path is not UTF-8"))?, false)?;
                if linked(&path.symlink_metadata().map_err(failed)?) || absolute(&current)? != path {
                    return Err(failed("input path changed after discovery"));
                }
                let (bytes, log) = recipe.process(&current)?;
                let target = out.join(relative).with_extension("pdf");
                let target = self.resolve(target.to_str().ok_or_else(|| failed("output path is not UTF-8"))?, true)?;
                if !target.starts_with(&out) {
                    return Err(failed("output path leaves out_dir"));
                }
                let output = publish(&target, &bytes)?;
                Ok((output, bytes.len(), log))
            }))
            .unwrap_or_else(|_| Err(failed("batch conversion panicked; no output was published")));
            match result {
                Ok((output, bytes, steps)) => {
                    completed += 1;
                    files.push(json!({"path":path.to_string_lossy(),"status":"ok","output":output.to_string_lossy(),"bytes":bytes,"steps":steps}));
                }
                Err(e) => {
                    failed_count += 1;
                    files.push(json!({"path":path.to_string_lossy(),"status":"error","error":e.to_string()}));
                    stopped = stop;
                }
            }
        }
        let skipped = files.iter().filter(|file| file["status"] == "skipped").count();
        Ok(json!({"dry_run":dry_run,"completed":completed,"failed":failed_count,"skipped":skipped,"stopped":stopped,"files":files}))
    }
}

/// Publish atomically without replacing any existing name, even if another writer wins a race.
///
/// `target` is the absolute path the caller checked. Its folder is opened from the file system
/// root one name at a time, refusing a link at every step and making missing folders on the way,
/// and the file is written through that open folder, so a folder swapped for a link after the
/// check cannot redirect the write.
pub(crate) fn publish(target: &Path, bytes: &[u8]) -> Result<PathBuf> {
    let dir = target.parent().ok_or_else(|| failed("output has no parent"))?;
    let name = target.file_name().ok_or_else(|| failed("output has no name"))?;
    let stem = target.file_stem().ok_or_else(|| failed("output has no stem"))?.to_string_lossy();
    let names = std::iter::once(name.to_os_string()).chain((2..=10_000).map(|n| format!("{stem} ({n}).pdf").into()));
    Folder::open_checked(dir)?.publish(bytes, names).map(|name| dir.join(name))
}

#[cfg(any(unix, windows))]
const LINKED_FOLDER: &str = "the output folder is a link, passes through one or is not a folder; nothing was written";
#[cfg(all(unix, not(any(target_os = "linux", target_os = "android", target_os = "freebsd"))))]
const UNREADABLE_FOLDER: &str = "a folder on the way to the output folder can't be read or written; on macOS and other systems without O_PATH the output folder and every folder above it must be readable, and a folder that has to be made needs a writable parent: fix the permissions or choose another output folder; nothing was written";

/// The output folder, held open from the file system root down.
///
/// Unix: a descriptor reached with `openat` and `O_NOFOLLOW` one name at a time, and the file is
/// made and named relative to it, so later moves of any folder on the path cannot redirect it.
#[cfg(unix)]
struct Folder(std::os::fd::OwnedFd);

/// A temporary name in a [`Folder`], removed on drop unless a rename already took it.
#[cfg(unix)]
struct Temp<'a>(&'a std::os::fd::OwnedFd, String, bool);

#[cfg(unix)]
impl Drop for Temp<'_> {
    fn drop(&mut self) {
        if !self.2 {
            let _ = rustix::fs::unlinkat(self.0, self.1.as_str(), rustix::fs::AtFlags::empty());
        }
    }
}

#[cfg(unix)]
impl Folder {
    fn open_checked(dir: &Path) -> Result<Self> {
        use rustix::fs::{Mode, OFlags, mkdirat, openat};
        use rustix::io::Errno;
        use std::path::Component;
        // `O_PATH` (Linux, Android, FreeBSD 14) needs only search permission on the folders above,
        // as writing by path did. macOS has no `O_PATH`, and rustix offers it no `O_SEARCH`, so
        // there the output folder and each folder above it must also be readable: a known limit,
        // refused with a message saying what to do rather than written by path. A link met with `O_NOFOLLOW` fails with
        // ELOOP (EMLINK on FreeBSD, ENOTDIR with `O_PATH`).
        #[cfg(any(target_os = "linux", target_os = "android", target_os = "freebsd"))]
        let flags = OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        #[cfg(not(any(target_os = "linux", target_os = "android", target_os = "freebsd")))]
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let refused = |e: Errno| match e {
            Errno::LOOP | Errno::NOTDIR | Errno::MLINK => failed(LINKED_FOLDER),
            #[cfg(not(any(target_os = "linux", target_os = "android", target_os = "freebsd")))]
            Errno::ACCESS => failed(UNREADABLE_FOLDER),
            e => failed(std::io::Error::from(e)),
        };
        let mut components = dir.components();
        if components.next() != Some(Component::RootDir) {
            return Err(failed("output folder is not absolute"));
        }
        let mut fd = rustix::fs::open("/", flags, Mode::empty()).map_err(refused)?;
        for component in components {
            let Component::Normal(name) = component else { return Err(failed(LINKED_FOLDER)) };
            fd = match openat(&fd, name, flags, Mode::empty()) {
                Err(Errno::NOENT) => {
                    // A name made since is opened the same way, so it cannot be a link either.
                    mkdirat(&fd, name, Mode::from_raw_mode(0o777)).or_else(|e| if e == Errno::EXIST { Ok(()) } else { Err(e) }).map_err(refused)?;
                    openat(&fd, name, flags, Mode::empty())
                }
                opened => opened,
            }
            .map_err(refused)?;
        }
        Ok(Self(fd))
    }

    /// Write `bytes` to a temporary file in the folder, then give it the first free name.
    fn publish(&self, bytes: &[u8], names: impl Iterator<Item = std::ffi::OsString>) -> Result<std::ffi::OsString> {
        use rustix::fs::{AtFlags, Mode, OFlags, linkat, openat};
        use rustix::io::Errno;
        use std::hash::BuildHasher;
        let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let mut made = None;
        for _ in 0..64 {
            let name = format!(".tmp{:016x}", std::collections::hash_map::RandomState::new().hash_one(0u8));
            match openat(&self.0, name.as_str(), flags, Mode::from_raw_mode(0o600)) {
                Ok(fd) => {
                    made = Some((Temp(&self.0, name, false), std::fs::File::from(fd)));
                    break;
                }
                Err(Errno::EXIST) => {}
                Err(e) => return Err(failed(std::io::Error::from(e))),
            }
        }
        let (mut temp, mut file) = made.ok_or_else(|| failed("no free temporary name in the output folder"))?;
        file.write_all(bytes).map_err(failed)?;
        file.sync_all().map_err(failed)?;
        drop(file);
        for name in names {
            // As `tempfile` does: rename without replacing where the system can, else hard-link
            // (which never replaces) and drop the temporary name.
            #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
            match rustix::fs::renameat_with(&self.0, temp.1.as_str(), &self.0, name.as_os_str(), rustix::fs::RenameFlags::NOREPLACE) {
                Ok(()) => {
                    temp.2 = true;
                    return Ok(name);
                }
                Err(Errno::EXIST) => continue,
                Err(Errno::NOSYS | Errno::INVAL) => {}
                Err(e) => return Err(failed(std::io::Error::from(e))),
            }
            match linkat(&self.0, temp.1.as_str(), &self.0, name.as_os_str(), AtFlags::empty()) {
                Ok(()) => return Ok(name),
                Err(Errno::EXIST) => {}
                Err(e) => return Err(failed(std::io::Error::from(e))),
            }
        }
        Err(failed("all 10000 output names are occupied"))
    }
}

/// Windows: every folder from the drive root down is held open without `FILE_SHARE_DELETE`, so
/// none can be renamed, moved or deleted (and so none swapped for a junction) until the file is
/// named. Each is opened as itself (`FILE_FLAG_OPEN_REPARSE_POINT`) and refused if it is a link,
/// including a volume mounted at a folder. Known limit: holding does not stop another process
/// turning a held, empty folder into a junction in place (`FSCTL_SET_REPARSE_POINT`), and the file
/// is still made and named by path, so such a conversion after opening is not caught; closing it
/// needs a create and rename relative to the held handle. Elsewhere (wasm) the folder is only made.
#[cfg(not(unix))]
struct Folder {
    path: PathBuf,
    #[cfg(windows)]
    _held: Vec<File>,
}

#[cfg(not(unix))]
impl Folder {
    #[cfg(windows)]
    fn open_checked(dir: &Path) -> Result<Self> {
        use std::os::windows::fs::OpenOptionsExt;
        use std::path::Component;
        const FILE_SHARE_READ_WRITE: u32 = 0x1 | 0x2;
        const BACKUP_SEMANTICS_AND_OPEN_REPARSE_POINT: u32 = 0x0200_0000 | 0x0020_0000;
        let hold = |path: &Path| {
            std::fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ_WRITE).custom_flags(BACKUP_SEMANTICS_AND_OPEN_REPARSE_POINT).open(path)
        };
        let (mut path, mut held) = (PathBuf::new(), Vec::new());
        for component in dir.components() {
            match component {
                Component::Prefix(_) => {
                    path.push(component);
                    continue;
                }
                Component::RootDir => path.push(component),
                Component::Normal(name) => path.push(name),
                Component::CurDir | Component::ParentDir => return Err(failed(LINKED_FOLDER)),
            }
            let file = match hold(&path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Its parent is held, so the name is made in the folder already checked.
                    std::fs::create_dir(&path)
                        .or_else(|e| if e.kind() == std::io::ErrorKind::AlreadyExists { Ok(()) } else { Err(e) })
                        .map_err(failed)?;
                    hold(&path)
                }
                opened => opened,
            }
            .map_err(failed)?;
            // Not a directory, or a name surrogate (symbolic link, junction or mount point).
            if !file.metadata().map_err(failed)?.is_dir() {
                return Err(failed(LINKED_FOLDER));
            }
            held.push(file);
        }
        if held.is_empty() {
            return Err(failed("output folder is not absolute"));
        }
        Ok(Self { path, _held: held })
    }

    #[cfg(not(windows))]
    fn open_checked(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir).map_err(failed)?;
        Ok(Self { path: dir.to_path_buf() })
    }

    fn publish(&self, bytes: &[u8], names: impl Iterator<Item = std::ffi::OsString>) -> Result<std::ffi::OsString> {
        let mut temp = tempfile::NamedTempFile::new_in(&self.path).map_err(failed)?;
        temp.write_all(bytes).map_err(failed)?;
        temp.as_file().sync_all().map_err(failed)?;
        for name in names {
            let candidate = self.path.join(&name);
            match temp.persist_noclobber(&candidate) {
                Ok(_) => return Ok(name),
                Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists || candidate.symlink_metadata().is_ok() => temp = e.file,
                Err(e) => return Err(failed(e.error)),
            }
        }
        Err(failed("all 10000 output names are occupied"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_limits_fail_before_reading_another_entry() {
        let mut discovery = Discovery { files: BTreeMap::new(), skipped: Vec::new(), seen: HashSet::new(), visited: MAX_ENTRIES };
        assert!(discovery.walk(Path::new("absent"), Path::new(""), Path::new("output"), true, 0).unwrap_err().to_string().contains("10000"));
        discovery.visited = 0;
        assert!(discovery.walk(Path::new("absent"), Path::new(""), Path::new("output"), true, MAX_DEPTH + 1).unwrap_err().to_string().contains("64"));
    }

    /// A link `at` to the folder `target`: a symlink on Unix, a junction on Windows (which needs
    /// no privilege).
    fn link_dir(at: &Path, target: &Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, at).unwrap();
        #[cfg(windows)]
        {
            // Rebuilt from components so every separator is `\` (cmd reads `/x` as a switch).
            let (link, target): (PathBuf, PathBuf) = (at.components().collect(), target.components().collect());
            let status = std::process::Command::new("cmd").arg("/C").arg("mklink").arg("/J").arg(link).arg(target).output().unwrap();
            assert!(status.status.success(), "mklink /J failed: {}", String::from_utf8_lossy(&status.stderr));
        }
    }

    /// The canonical temporary folder, as callers pass it, and a folder `outside` in it, named
    /// as created (mklink takes it in that form).
    fn sandbox() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        let base = dir.path().canonicalize().unwrap();
        (dir, base, outside)
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn publish_refuses_an_output_folder_swapped_for_a_link() {
        // The caller checked `base/out`; it was then replaced by a link out of the root.
        let (_dir, base, outside) = sandbox();
        link_dir(&base.join("out"), &outside);
        let error = publish(&base.join("out/a.pdf"), b"%PDF-1.7").unwrap_err().to_string();
        assert!(error.contains("link"), "{error}");
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0, "nothing is written through the link");
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn publish_refuses_a_link_above_the_output_folder() {
        let (_dir, base, outside) = sandbox();
        link_dir(&base.join("up"), &outside);
        let error = publish(&base.join("up/new/sub/a.pdf"), b"%PDF-1.7").unwrap_err().to_string();
        assert!(error.contains("link"), "{error}");
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0, "no folder is made through the link");
    }

    #[test]
    fn publish_makes_missing_folders_and_never_replaces_a_file() {
        let (_dir, base, _) = sandbox();
        let target = base.join("out/x/a.pdf");
        assert_eq!(publish(&target, b"first").unwrap(), target);
        assert_eq!(publish(&target, b"second").unwrap(), base.join("out/x/a (2).pdf"));
        assert_eq!(std::fs::read(&target).unwrap(), b"first");
        assert_eq!(std::fs::read(base.join("out/x/a (2).pdf")).unwrap(), b"second");
        assert_eq!(std::fs::read_dir(base.join("out/x")).unwrap().count(), 2, "no temporary file is left");
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_swapped_for_a_link_after_it_was_opened_still_gets_the_file() {
        let (_dir, base, outside) = sandbox();
        let out = base.join("out");
        let folder = Folder::open_checked(&out).unwrap();
        std::fs::rename(&out, base.join("moved")).unwrap();
        link_dir(&out, &outside);
        assert_eq!(folder.publish(b"%PDF-1.7", [std::ffi::OsString::from("a.pdf")].into_iter()).unwrap(), "a.pdf");
        assert_eq!(std::fs::read(base.join("moved/a.pdf")).unwrap(), b"%PDF-1.7");
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
    }

    #[cfg(any(target_os = "linux", target_os = "android", target_os = "freebsd"))]
    #[test]
    fn publish_needs_only_search_permission_above_the_output_folder() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, base, _) = sandbox();
        let (locked, out) = (base.join("locked"), base.join("locked/out"));
        std::fs::create_dir_all(&out).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o111)).unwrap();
        let result = if std::fs::read_dir(&locked).is_ok() {
            eprintln!("skipping: this user reads the folder anyway (root)");
            None
        } else {
            Some(publish(&out.join("a.pdf"), b"%PDF-1.7"))
        };
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        if let Some(result) = result {
            assert_eq!(result.unwrap(), out.join("a.pdf"));
        }
    }

    #[cfg(windows)]
    #[test]
    fn an_open_output_folder_and_its_parents_cannot_be_moved() {
        let (_dir, base, _) = sandbox();
        let out = base.join("parent/out");
        let folder = Folder::open_checked(&out).unwrap();
        assert!(std::fs::rename(&out, base.join("moved")).is_err(), "the folder is held");
        assert!(std::fs::rename(base.join("parent"), base.join("moved")).is_err(), "its parent is held");
        drop(folder);
        std::fs::rename(&out, base.join("moved")).unwrap();
    }

    /// Turns the existing, empty folder `dir` into a junction to `target` in place, as another
    /// process could whilst publish holds it: no rename and no delete, only a reparse point set
    /// through a handle opened for `FILE_WRITE_ATTRIBUTES`, which no share mode can refuse.
    #[cfg(windows)]
    fn convert_in_place(dir: &Path, target: &Path) {
        const SCRIPT: &str = r#"param([string]$Dir, [string]$Target)
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
using Microsoft.Win32.SafeHandles;
public static class InPlaceJunction {
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern SafeFileHandle CreateFileW(string name, uint access, uint share, IntPtr sa, uint disposition, uint flags, IntPtr template);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool DeviceIoControl(SafeFileHandle h, uint code, byte[] inBuf, int inLen, IntPtr outBuf, int outLen, out int returned, IntPtr overlapped);
    public static string Make(string dir, string target) {
        SafeFileHandle h = CreateFileW(dir, 0x100, 7, IntPtr.Zero, 3, 0x02000000 | 0x00200000, IntPtr.Zero);
        if (h.IsInvalid) return "open failed " + Marshal.GetLastWin32Error();
        byte[] sub = Encoding.Unicode.GetBytes("\\??\\" + target);
        int pathLen = sub.Length + 4;
        byte[] buf = new byte[16 + pathLen];
        BitConverter.GetBytes(0xA0000003u).CopyTo(buf, 0);
        BitConverter.GetBytes((ushort)(8 + pathLen)).CopyTo(buf, 4);
        BitConverter.GetBytes((ushort)sub.Length).CopyTo(buf, 10);
        BitConverter.GetBytes((ushort)(sub.Length + 2)).CopyTo(buf, 12);
        sub.CopyTo(buf, 16);
        int r;
        bool ok = DeviceIoControl(h, 0x000900A4, buf, buf.Length, IntPtr.Zero, 0, out r, IntPtr.Zero);
        int err = Marshal.GetLastWin32Error();
        h.Close();
        return ok ? "ok" : "fsctl failed " + err;
    }
}
'@
[InPlaceJunction]::Make($Dir, $Target)
"#;
        let script = target.with_file_name("in-place-junction.ps1");
        std::fs::write(&script, SCRIPT).unwrap();
        let (dir, target): (PathBuf, PathBuf) = (dir.components().collect(), target.components().collect());
        let out =
            std::process::Command::new("pwsh").args(["-NoProfile", "-NonInteractive", "-File"]).arg(&script).arg(dir).arg(target).output().unwrap();
        let said = String::from_utf8_lossy(&out.stdout);
        assert_eq!(said.trim(), "ok", "{}", String::from_utf8_lossy(&out.stderr));
    }

    #[cfg(windows)]
    #[test]
    fn a_held_empty_folder_turned_into_a_junction_in_place_gets_nothing_written_through_it() {
        let (_dir, base, outside) = sandbox();
        let out = base.join("out");
        let folder = Folder::open_checked(&out).unwrap();
        convert_in_place(&out, &outside);
        let _ = folder.publish(b"%PDF-1.7", [std::ffi::OsString::from("a.pdf")].into_iter());
        let leaked: Vec<_> = std::fs::read_dir(&outside).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert!(leaked.is_empty(), "written outside the checked folder: {leaked:?}");
    }

    #[test]
    fn doc_batch_dry_run_lists_outputs_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("input")).unwrap();
        let pdf = Session::new().create_from_text("Doc", "Hello").unwrap();
        std::fs::write(dir.path().join("input/a.pdf"), &*pdf).unwrap();
        let auto = Automation::new().with_root(dir.path()).unwrap();
        let report = auto.doc_batch(&Args(&json!({"paths":["input"],"steps":["optimize"],"out_dir":"out","dry_run":true}))).unwrap();
        assert_eq!(report["dry_run"], true, "{report}");
        assert_eq!(report["completed"], 0, "{report}");
        assert_eq!(report["failed"], 0, "{report}");
        assert_eq!(report["files"][0]["status"], "planned", "{report}");
        assert!(report["files"][0]["output"].as_str().unwrap().ends_with("out/a.pdf"), "{report}");
        assert!(!dir.path().join("out").exists());
    }
}
