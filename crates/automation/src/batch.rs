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
/// none can be renamed, moved or deleted until the file is named, and each is opened as itself
/// (`FILE_FLAG_OPEN_REPARSE_POINT`) and refused if it is a link, including a volume mounted at a
/// folder. A held folder can still be turned into a link in place whilst it is empty, and a
/// relative symbolic link set that way is followed by a create relative to the folder, so:
///
/// - nothing is taken on trust from where it was asked for: each folder held, and each file or
///   folder made relative to its held parent, must be an entry of that parent by the final path
///   of its own handle (`GetFinalPathNameByHandleW`); one made elsewhere is deleted again by its
///   own handle and the publish fails;
/// - a held folder whose entry is held in turn can no longer be emptied, so never converted (NTFS
///   and ReFS refuse a reparse point on a folder with an entry); an anchor file held without
///   delete sharing does the same for the output folder whilst the file is named by path;
/// - the bytes are written through a handle held without delete sharing, so the file cannot be
///   moved whilst they are written, and the name taken must then hold that file;
/// - clean-up deletes only what publish made, each by its own handle, never by name.
///
/// Elsewhere (wasm) the folder is only made.
#[cfg(windows)]
struct Folder {
    path: PathBuf,
    /// From the drive root down, each with its final path; the last is the output folder.
    held: Vec<(File, PathBuf)>,
}

/// A file or folder publish made, deleted by its own handle on drop unless kept.
#[cfg(windows)]
struct Made(Option<File>);

#[cfg(windows)]
impl Made {
    fn get(&self) -> Result<&File> {
        self.0.as_ref().ok_or_else(|| failed("the file made was already closed"))
    }

    /// Close the handle and keep what it refers to.
    fn keep(mut self) {
        self.0 = None;
    }

    /// A second handle to the same file (no name is looked up).
    fn reopen(file: &File, access: winx::file::AccessMode, share: winx::file::ShareMode) -> std::io::Result<File> {
        use std::os::windows::io::AsHandle;
        winx::file::reopen_file(file.as_handle(), access, share, winx::file::Flags::FILE_FLAG_OPEN_REPARSE_POINT)
    }
}

#[cfg(windows)]
impl Drop for Made {
    fn drop(&mut self) {
        use fs_at::os::windows::FileExt;
        use winx::file::{AccessMode, ShareMode};
        if let Some(file) = self.0.take() {
            let all = ShareMode::FILE_SHARE_READ | ShareMode::FILE_SHARE_WRITE | ShareMode::FILE_SHARE_DELETE;
            // Directly when the handle has DELETE access (a folder made), else through a second
            // handle to the same file that has it.
            if let Err((file, _)) = file.delete_by_handle()
                && let Ok(deleting) = Self::reopen(&file, AccessMode::DELETE, all)
            {
                let _ = deleting.delete_by_handle();
            }
        }
    }
}

#[cfg(windows)]
impl Folder {
    const SHARE_READ_WRITE: u32 = 0x1 | 0x2;
    const BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const READ_ATTRIBUTES: u32 = 0x80;

    /// `path` opened as itself, by path, without `FILE_SHARE_DELETE`.
    fn hold_path(path: &Path) -> std::io::Result<File> {
        use std::os::windows::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(Self::SHARE_READ_WRITE)
            .custom_flags(Self::BACKUP_SEMANTICS | Self::OPEN_REPARSE_POINT)
            .open(path)
    }

    fn refused(e: std::io::Error) -> crate::ToolError {
        // `ERROR_DIRECTORY`, or `ERROR_CANT_RESOLVE_FILENAME`: a link met relative to a held
        // folder, or a held folder turned into one.
        const ERROR_DIRECTORY: i32 = 267;
        const ERROR_CANT_RESOLVE_FILENAME: i32 = 1921;
        match e.raw_os_error() {
            Some(ERROR_DIRECTORY | ERROR_CANT_RESOLVE_FILENAME) => failed(LINKED_FOLDER),
            _ => failed(e),
        }
    }

    /// Where `file` is now, by its own handle, which must be an entry of the held folder whose
    /// final path is `parent`.
    fn placed_in(file: &File, parent: &Path) -> Result<PathBuf> {
        let at = winx::file::get_file_path(file).map_err(failed)?;
        if at.file_name().is_some() && at.parent() == Some(parent) { Ok(at) } else { Err(failed(LINKED_FOLDER)) }
    }

    fn leaf(&self) -> Result<&(File, PathBuf)> {
        self.held.last().ok_or_else(|| failed("output folder is not absolute"))
    }

    fn open_checked(dir: &Path) -> Result<Self> {
        use std::path::Component;
        let mut folder = Self { path: PathBuf::new(), held: Vec::new() };
        for component in dir.components() {
            match component {
                Component::Prefix(_) => folder.path.push(component),
                Component::RootDir => {
                    folder.path.push(component);
                    let root = Self::hold_path(&folder.path).map_err(failed)?;
                    if !root.metadata().map_err(failed)?.is_dir() {
                        return Err(failed(LINKED_FOLDER));
                    }
                    let at = winx::file::get_file_path(&root).map_err(failed)?;
                    folder.held.push((root, at));
                }
                Component::Normal(name) => folder.enter_checked(name)?,
                Component::CurDir | Component::ParentDir => return Err(failed(LINKED_FOLDER)),
            }
        }
        if folder.held.len() < 2 {
            return Err(failed("output folder is not absolute"));
        }
        Ok(folder)
    }

    /// Hold the folder `name` in the current one, making it there if it is missing.
    fn enter_checked(&mut self, name: &std::ffi::OsStr) -> Result<()> {
        let (parent, parent_at) = self.leaf()?;
        let path = self.path.join(name);
        let pinned = match Self::hold_path(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                match fs_at::OpenOptions::default().mkdir_at(parent, name) {
                    Ok(made) => {
                        // Made through a parent turned into a relative symbolic link, it is
                        // elsewhere: `made` deletes it again.
                        let made = Made(Some(made));
                        Self::placed_in(made.get()?, parent_at)?;
                        made.keep();
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(Self::refused(e)),
                }
                Self::hold_path(&path)
            }
            held => held,
        }
        .map_err(failed)?;
        // Not a directory, or a name surrogate (symbolic link, junction or mount point); or the
        // path no longer leads to an entry of the held parent.
        if !pinned.metadata().map_err(failed)?.is_dir() {
            return Err(failed(LINKED_FOLDER));
        }
        let at = Self::placed_in(&pinned, parent_at)?;
        self.held.push((pinned, at));
        self.path = path;
        Ok(())
    }

    /// A new, empty file with a free temporary name, made relative to the output folder; the
    /// handle reads attributes only and shares everything, so it pins nothing.
    fn make_temporary(&self) -> Result<Made> {
        use fs_at::os::windows::OpenOptionsExt;
        use std::hash::BuildHasher;
        let (leaf, _) = self.leaf()?;
        for _ in 0..64 {
            let name = format!(".tmp{:016x}", std::collections::hash_map::RandomState::new().hash_one(0u8));
            let mut options = fs_at::OpenOptions::default();
            options.create_new(true).desired_access(Self::READ_ATTRIBUTES);
            match options.open_at(leaf, &name) {
                Ok(file) => return Ok(Made(Some(file))),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(Self::refused(e)),
            }
        }
        Err(failed("no free temporary name in the output folder"))
    }

    /// `made` held without delete sharing, once it is shown to be in the output folder.
    fn pin(&self, made: &Made, access: winx::file::AccessMode) -> Result<File> {
        use winx::file::ShareMode;
        let (_, leaf_at) = self.leaf()?;
        let pinned = Made::reopen(made.get()?, access, ShareMode::FILE_SHARE_READ | ShareMode::FILE_SHARE_WRITE).map_err(failed)?;
        Self::placed_in(&pinned, leaf_at)?;
        Ok(pinned)
    }

    /// Write `bytes` to a temporary file in the folder, then give it the first free name.
    fn publish(&self, bytes: &[u8], names: impl Iterator<Item = std::ffi::OsString>) -> Result<std::ffi::OsString> {
        use winx::file::AccessMode;
        // The anchor: whilst it is held the output folder has an entry nobody else can remove.
        let anchor = self.make_temporary()?;
        let anchor_pin = self.pin(&anchor, AccessMode::GENERIC_READ)?;
        let temp = self.make_temporary()?;
        let named = self.pin(&temp, AccessMode::GENERIC_READ | AccessMode::GENERIC_WRITE).and_then(|mut file| {
            file.write_all(bytes).map_err(failed)?;
            file.sync_all().map_err(failed)
        });
        let named = named.and_then(|()| self.take_name(temp, names));
        drop(anchor_pin);
        named
    }

    /// Give `temp` the first free name, then check by its handle that the name holds it.
    fn take_name(&self, temp: Made, names: impl Iterator<Item = std::ffi::OsString>) -> Result<std::ffi::OsString> {
        let (_, leaf_at) = self.leaf()?;
        let at = Self::placed_in(temp.get()?, leaf_at)?;
        let from = self.path.join(at.file_name().ok_or_else(|| failed(LINKED_FOLDER))?);
        for name in names {
            let candidate = self.path.join(&name);
            // `MoveFileExW` without replacing, by path: every folder on it is held and has an
            // entry, so none can be a link. The name is never removed by `tempfile`.
            let path = tempfile::TempPath::try_from_path(&from).map_err(failed)?;
            match path.persist_noclobber(&candidate) {
                Ok(()) => {
                    if winx::file::get_file_path(temp.get()?).is_ok_and(|now| now == leaf_at.join(&name)) {
                        temp.keep();
                        return Ok(name);
                    }
                    // Another file was put at the temporary name and took the name; it is left
                    // alone, and `temp` deletes ours wherever it went.
                    return Err(failed("the file was replaced whilst it was being named; nothing was kept"));
                }
                Err(e) => {
                    let _ = e.path.keep();
                    if e.error.kind() != std::io::ErrorKind::AlreadyExists && candidate.symlink_metadata().is_err() {
                        return Err(failed(e.error));
                    }
                }
            }
        }
        Err(failed("all 10000 output names are occupied"))
    }
}

#[cfg(not(any(unix, windows)))]
struct Folder {
    path: PathBuf,
}

#[cfg(not(any(unix, windows)))]
impl Folder {
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

    #[cfg(any(unix, windows))]
    #[test]
    fn a_link_planted_at_the_output_name_is_not_followed() {
        let (_dir, base, outside) = sandbox();
        std::fs::create_dir(base.join("out")).unwrap();
        link_dir(&base.join("out/a.pdf"), &outside);
        assert_eq!(publish(&base.join("out/a.pdf"), b"%PDF-1.7").unwrap(), base.join("out/a (2).pdf"));
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0, "nothing is written through the link");
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

    /// `path` with every separator `\`, as Windows tools take it.
    #[cfg(windows)]
    fn win(path: &Path) -> String {
        path.components().collect::<PathBuf>().to_string_lossy().into_owned()
    }

    /// Sets a mount-point reparse point on the existing folder `dir`, pointing at `target` (a
    /// folder, which makes a junction, or `Volume{...}\`, which mounts a volume there), as another
    /// process could whilst publish holds it: no rename and no delete, only `FSCTL_SET_REPARSE_POINT`
    /// through a handle opened for `access` (`FILE_WRITE_ATTRIBUTES` is not subject to sharing).
    /// Returns what the script said: `ok`, or the Windows error.
    #[cfg(windows)]
    fn convert_in_place(dir: &Path, target: &str, access: u32) -> String {
        reparse_in_place(dir, target, access, false)
    }

    /// Sets a relative directory symbolic link (`SYMLINK_FLAG_RELATIVE`, `target` such as
    /// `..\outside`) on the existing folder `dir` in place, as [`convert_in_place`] does a junction.
    /// `None`, with a message, when this user may not make symbolic links.
    #[cfg(windows)]
    fn relative_symlink_in_place(dir: &Path, target: &str) -> Option<()> {
        match reparse_in_place(dir, target, FILE_WRITE_ATTRIBUTES, true).as_str() {
            "ok" => Some(()),
            // ERROR_PRIVILEGE_NOT_HELD: no SeCreateSymbolicLinkPrivilege and no Developer Mode.
            "fsctl failed 1314" => {
                eprintln!("skipping: this user cannot make symbolic links");
                None
            }
            said => panic!("{said}"),
        }
    }

    #[cfg(windows)]
    fn reparse_in_place(dir: &Path, target: &str, access: u32, relative_symlink: bool) -> String {
        const SCRIPT: &str = r#"param([string]$Dir, [string]$Target, [uint32]$Access, [string]$Kind)
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
    public static string Make(string dir, string target, uint access, string kind) {
        SafeFileHandle h = CreateFileW(dir, access, 7, IntPtr.Zero, 3, 0x02000000 | 0x00200000, IntPtr.Zero);
        if (h.IsInvalid) return "open failed " + Marshal.GetLastWin32Error();
        byte[] buf;
        if (kind == "symrel") {
            // IO_REPARSE_TAG_SYMLINK: a 20-byte header (with Flags), then the names.
            byte[] name = Encoding.Unicode.GetBytes(target);
            buf = new byte[20 + 2 * name.Length];
            BitConverter.GetBytes(0xA000000Cu).CopyTo(buf, 0);
            BitConverter.GetBytes((ushort)(12 + 2 * name.Length)).CopyTo(buf, 4);
            BitConverter.GetBytes((ushort)name.Length).CopyTo(buf, 10);
            BitConverter.GetBytes((ushort)name.Length).CopyTo(buf, 12);
            BitConverter.GetBytes((ushort)name.Length).CopyTo(buf, 14);
            BitConverter.GetBytes(1u).CopyTo(buf, 16);
            name.CopyTo(buf, 20);
            name.CopyTo(buf, 20 + name.Length);
        } else {
            byte[] sub = Encoding.Unicode.GetBytes("\\??\\" + target);
            int pathLen = sub.Length + 4;
            buf = new byte[16 + pathLen];
            BitConverter.GetBytes(0xA0000003u).CopyTo(buf, 0);
            BitConverter.GetBytes((ushort)(8 + pathLen)).CopyTo(buf, 4);
            BitConverter.GetBytes((ushort)sub.Length).CopyTo(buf, 10);
            BitConverter.GetBytes((ushort)(sub.Length + 2)).CopyTo(buf, 12);
            sub.CopyTo(buf, 16);
        }
        int r;
        bool ok = DeviceIoControl(h, 0x000900A4, buf, buf.Length, IntPtr.Zero, 0, out r, IntPtr.Zero);
        int err = Marshal.GetLastWin32Error();
        h.Close();
        return ok ? "ok" : "fsctl failed " + err;
    }
}
'@
[InPlaceJunction]::Make($Dir, $Target, $Access, $Kind)
"#;
        // pwsh refuses a `\\?\` script path, which the canonical sandbox has.
        let script = PathBuf::from(win(&dir.with_file_name("in-place-junction.ps1")).trim_start_matches(r"\\?\"));
        std::fs::write(&script, SCRIPT).unwrap();
        let out = std::process::Command::new("pwsh")
            .args(["-NoProfile", "-NonInteractive", "-File"])
            .arg(&script)
            .arg(win(dir))
            .arg(target)
            .arg(access.to_string())
            .arg(if relative_symlink { "symrel" } else { "junction" })
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if said.is_empty() { String::from_utf8_lossy(&out.stderr).trim().to_string() } else { said }
    }

    #[cfg(windows)]
    const FILE_WRITE_ATTRIBUTES: u32 = 0x100;
    #[cfg(windows)]
    const GENERIC_WRITE: u32 = 0x4000_0000;

    #[cfg(windows)]
    fn names(dir: &Path) -> Vec<std::ffi::OsString> {
        std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name()).collect()
    }

    #[cfg(windows)]
    fn a_held_empty_folder_turned_into_a_junction_gets_nothing(access: u32) {
        let (_dir, base, outside) = sandbox();
        let out = base.join("out");
        let folder = Folder::open_checked(&out).unwrap();
        assert_eq!(convert_in_place(&out, &win(&outside), access), "ok");
        let error = folder.publish(b"%PDF-1.7", [std::ffi::OsString::from("a.pdf")].into_iter()).unwrap_err().to_string();
        assert!(error.contains("link"), "{error}");
        assert!(names(&outside).is_empty(), "written outside the checked folder: {:?}", names(&outside));
    }

    #[cfg(windows)]
    #[test]
    fn a_held_empty_folder_turned_into_a_junction_in_place_gets_nothing_written_through_it() {
        a_held_empty_folder_turned_into_a_junction_gets_nothing(FILE_WRITE_ATTRIBUTES);
    }

    #[cfg(windows)]
    #[test]
    fn a_held_empty_folder_turned_into_a_junction_by_a_writer_gets_nothing_written_through_it() {
        a_held_empty_folder_turned_into_a_junction_gets_nothing(GENERIC_WRITE);
    }

    /// The design rests on this: NTFS refuses a reparse point on a folder that has an entry.
    #[cfg(windows)]
    #[test]
    fn a_folder_with_an_entry_cannot_be_turned_into_a_junction() {
        let (_dir, base, outside) = sandbox();
        let out = base.join("out");
        std::fs::create_dir(&out).unwrap();
        std::fs::write(out.join("entry"), b"").unwrap();
        assert_eq!(convert_in_place(&out, &win(&outside), FILE_WRITE_ATTRIBUTES), "fsctl failed 145");
    }

    #[cfg(windows)]
    #[test]
    fn a_folder_just_made_and_turned_into_a_junction_gets_no_folder_made_through_it() {
        let (_dir, base, outside) = sandbox();
        let made = base.join("new");
        let mut folder = Folder::open_checked(&made).unwrap();
        assert_eq!(convert_in_place(&made, &win(&outside), FILE_WRITE_ATTRIBUTES), "ok");
        let error = folder.enter_checked("sub".as_ref()).unwrap_err().to_string();
        assert!(error.contains("link"), "{error}");
        assert!(names(&outside).is_empty(), "made outside the checked folder: {:?}", names(&outside));
    }

    /// The only entry of a held folder is moved out, the emptied folder is turned into a junction
    /// to where the entry went, and the walk goes on by path: the path now leads outside.
    #[cfg(windows)]
    #[test]
    fn an_entry_moved_out_of_a_held_folder_turned_into_a_junction_is_refused() {
        let (_dir, base, outside) = sandbox();
        let out = base.join("out");
        std::fs::create_dir_all(out.join("inner")).unwrap();
        let mut folder = Folder::open_checked(&out).unwrap();
        std::fs::rename(out.join("inner"), outside.join("inner")).unwrap();
        assert_eq!(convert_in_place(&out, &win(&outside), FILE_WRITE_ATTRIBUTES), "ok");
        let error = folder.enter_checked("inner".as_ref()).unwrap_err().to_string();
        assert!(error.contains("link"), "{error}");
        assert!(names(&outside.join("inner")).is_empty());
    }

    /// Moves each temporary file of `out` that another process can move into `to`, and puts a
    /// file of its own at that name.
    #[cfg(windows)]
    fn swap_temporaries(out: &Path, to: &Path) {
        for name in names(out) {
            if std::fs::rename(out.join(&name), to.join(&name)).is_ok() {
                std::fs::write(out.join(&name), b"theirs").unwrap();
            }
        }
    }

    /// Another process moves the temporary file away and puts its own file at the temporary name
    /// just before it is named: the rename then gives the other file the output name. The publish
    /// fails, leaves that file alone and removes only the file it made.
    #[cfg(windows)]
    #[test]
    fn a_temporary_file_swapped_before_it_is_named_is_not_published() {
        let (_dir, base, outside) = sandbox();
        let out = base.join("out");
        let folder = Folder::open_checked(&out).unwrap();
        let taken = std::iter::once(()).map(|()| {
            swap_temporaries(&out, &outside);
            std::ffi::OsString::from("a.pdf")
        });
        let error = folder.publish(b"ours", taken).unwrap_err().to_string();
        assert!(error.contains("replaced"), "{error}");
        assert_eq!(std::fs::read(out.join("a.pdf")).unwrap(), b"theirs", "the other file is not removed");
        assert!(names(&outside).is_empty(), "the file made is not kept: {:?}", names(&outside));
    }

    /// A failed publish removes only what it made: a file another process put at a temporary name
    /// survives, and the folder is left holding only that file.
    #[cfg(windows)]
    #[test]
    fn a_file_put_at_a_temporary_name_survives_a_failed_publish() {
        let (_dir, base, outside) = sandbox();
        let out = base.join("out");
        let folder = Folder::open_checked(&out).unwrap();
        let taken = std::iter::once(()).filter_map(|()| {
            swap_temporaries(&out, &outside);
            None::<std::ffi::OsString>
        });
        let error = folder.publish(b"ours", taken).unwrap_err().to_string();
        assert!(error.contains("occupied"), "{error}");
        let left: Vec<Vec<u8>> = names(&out).iter().map(|name| std::fs::read(out.join(name)).unwrap()).collect();
        assert_eq!(left, [b"theirs".to_vec()]);
        assert!(names(&outside).is_empty(), "the file made is not kept: {:?}", names(&outside));
    }

    /// Whilst the file is named, another process moves out every entry of the output folder it
    /// can: the anchor stays, so the folder cannot be turned into a link.
    #[cfg(windows)]
    #[test]
    fn the_output_folder_keeps_an_entry_whilst_the_file_is_named() {
        let (_dir, base, outside) = sandbox();
        let out = base.join("out");
        let folder = Folder::open_checked(&out).unwrap();
        let said = std::cell::RefCell::new(String::new());
        let taken = std::iter::once(()).map(|()| {
            for name in names(&out) {
                let _ = std::fs::rename(out.join(&name), outside.join(&name));
            }
            *said.borrow_mut() = convert_in_place(&out, &win(&outside), FILE_WRITE_ATTRIBUTES);
            std::ffi::OsString::from("a.pdf")
        });
        let _ = folder.publish(b"%PDF-1.7", taken);
        assert_eq!(said.into_inner(), "fsctl failed 145");
    }

    /// A file made in the output folder is moved out before it is pinned: the pin reports where
    /// the file now is, so it is refused, and the file is then deleted by its handle.
    #[cfg(windows)]
    #[test]
    fn a_file_moved_out_before_it_is_pinned_is_refused_and_deleted() {
        let (_dir, base, outside) = sandbox();
        let out = base.join("out");
        let folder = Folder::open_checked(&out).unwrap();
        let made = folder.make_temporary().unwrap();
        let [name] = names(&out).try_into().unwrap();
        std::fs::rename(out.join(&name), outside.join(&name)).unwrap();
        let error = folder.pin(&made, winx::file::AccessMode::GENERIC_READ).unwrap_err().to_string();
        assert!(error.contains("link"), "{error}");
        drop(made);
        assert!(names(&outside).is_empty(), "{:?}", names(&outside));
    }

    /// A held folder just made is turned in place into a relative symbolic link to `..\outside`;
    /// a relative create resolves that against the folder itself, so it lands in `outside`.
    #[cfg(windows)]
    #[test]
    fn a_folder_just_made_and_turned_into_a_relative_symlink_gets_no_folder_made_through_it() {
        let (_dir, base, outside) = sandbox();
        let made = base.join("new");
        let mut folder = Folder::open_checked(&made).unwrap();
        if relative_symlink_in_place(&made, r"..\outside").is_none() {
            return;
        }
        let error = folder.enter_checked("sub".as_ref()).unwrap_err().to_string();
        assert!(error.contains("link"), "{error}");
        assert!(names(&outside).is_empty(), "made outside the checked folder: {:?}", names(&outside));
    }

    /// The held output folder `b\out` is turned into a relative symbolic link to `..\outside`, and
    /// a junction `outside` beside `b` leads to `b\outside`, so the folder reached by path
    /// (`b\..\outside`) and the one reached relative to the held folder (`out\..\outside`) are the
    /// same: every check that compares the two agrees, and the file must still not land there.
    #[cfg(windows)]
    #[test]
    fn a_held_output_folder_turned_into_a_relative_symlink_gets_nothing_published_through_it() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join(r"b\outside");
        std::fs::create_dir_all(&outside).unwrap();
        link_dir(&dir.path().join("outside"), &outside);
        let out = dir.path().canonicalize().unwrap().join(r"b\out");
        let folder = Folder::open_checked(&out).unwrap();
        if relative_symlink_in_place(&out, r"..\outside").is_none() {
            return;
        }
        let result = folder.publish(b"%PDF-1.7 ours", [std::ffi::OsString::from("a.pdf")].into_iter());
        assert!(names(&outside).is_empty(), "written outside the checked folder: {:?} ({result:?})", names(&outside));
        let error = result.unwrap_err().to_string();
        assert!(error.contains("link"), "{error}");
    }

    #[cfg(windows)]
    #[test]
    fn publish_refuses_an_output_folder_that_is_a_directory_symlink() {
        let (_dir, base, outside) = sandbox();
        match std::os::windows::fs::symlink_dir(&outside, base.join("out")) {
            Ok(()) => {}
            // ERROR_PRIVILEGE_NOT_HELD: no SeCreateSymbolicLinkPrivilege and no Developer Mode.
            Err(e) if e.raw_os_error() == Some(1314) => return eprintln!("skipping: this user cannot make symbolic links"),
            Err(e) => panic!("{e}"),
        }
        let error = publish(&base.join("out/a.pdf"), b"%PDF-1.7").unwrap_err().to_string();
        assert!(error.contains("link"), "{error}");
        assert!(names(&outside).is_empty());
    }

    /// The volume holding the sandbox, as `Volume{...}\`.
    #[cfg(windows)]
    fn volume_of(path: &Path) -> String {
        let Some(std::path::Component::Prefix(prefix)) = path.components().next() else { panic!("no drive in {path:?}") };
        let drive = format!("{}\\", prefix.as_os_str().to_string_lossy().trim_start_matches(r"\\?\"));
        let out = std::process::Command::new("mountvol").arg(&drive).arg("/L").output().unwrap();
        let said = String::from_utf8_lossy(&out.stdout);
        said.trim().trim_start_matches(r"\\?\").to_string()
    }

    #[cfg(windows)]
    #[test]
    fn a_volume_mounted_at_the_output_folder_gets_nothing_written_to_it() {
        let (_dir, base, _) = sandbox();
        let volume = volume_of(&base);
        assert!(volume.starts_with("Volume{"), "{volume}");
        let name = format!("plutarch-mount-test-{}.pdf", std::process::id());
        // Mounted before the walk, and mounted at a held, empty folder after it.
        let early = base.join("early");
        std::fs::create_dir(&early).unwrap();
        assert_eq!(convert_in_place(&early, &volume, FILE_WRITE_ATTRIBUTES), "ok");
        let error = publish(&early.join(&name), b"%PDF-1.7").unwrap_err().to_string();
        assert!(error.contains("link"), "{error}");
        let late = base.join("late");
        let folder = Folder::open_checked(&late).unwrap();
        assert_eq!(convert_in_place(&late, &volume, FILE_WRITE_ATTRIBUTES), "ok");
        let error = folder.publish(b"%PDF-1.7", [std::ffi::OsString::from(&name)].into_iter()).unwrap_err().to_string();
        assert!(error.contains("link"), "{error}");
        assert!(!early.join(&name).exists() && !late.join(&name).exists(), "nothing reached the volume's root");
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
