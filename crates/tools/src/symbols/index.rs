//! The index the mention picker searches: the files of each approved folder and the
//! definitions in them.
//!
//! - One index per approved folder, shared by every chat that has the folder. A chat only
//!   searches the indexes of its own approved folders (a folder inside another approved
//!   one is covered by the outer one).
//! - Built on a background thread, never on the caller's: names first (breadth first, so
//!   shallow files come first), definitions second. What is built so far is published in
//!   batches, so a search answers at once from what is there.
//! - The walk never follows a link, never enters a folder the sandbox refuses (the app's
//!   own folders) or a place mentions refuse (`.git`, `.ssh`, skill folders), and skips the
//!   folders the file search skips plus build output. Files over 1 MB, binary files and
//!   files in protected places are left out, so a name the index shows is one a mention
//!   can read.
//! - A rebuild (at most every few seconds, when searched) reuses what it knew of a file
//!   whose size and modified time did not change, and keeps serving the old list until the
//!   new one is ready.
//! - Memory is bounded: 20,000 files and 200,000 definitions per folder, 2,000 per file,
//!   and at most 4 folders kept (the least recently searched is dropped).

use std::collections::VecDeque;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime};

use policy::path::{Flavor, PathSandbox, RealFs, volume_case};
use policy::{WriteKind, always_ask_reason};

use super::find::{Lang, Symbol, SymbolKind, find_symbols};
use super::resolve::off_limits_for;

/// Files indexed per folder at most.
pub const MAX_FILES: usize = 20_000;
/// Definitions kept per folder at most.
pub const MAX_SYMBOLS: usize = 200_000;
/// Files larger than this are left out.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// Folders kept at most.
pub const MAX_FOLDERS: usize = 4;
/// A folder is walked again when searched at least this long after its last build.
const REFRESH_AFTER: Duration = Duration::from_secs(3);
/// Files read between two publications of the definitions found so far.
const PUBLISH_EVERY: usize = 400;

/// Folders the walk never enters: those the file search skips, build output, caches and
/// dependency folders.
const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".turbo",
    ".parcel-cache",
    ".cache",
    "coverage",
    "__pycache__",
    ".venv",
    "venv",
    ".tox",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".gradle",
    "bin",
    "obj",
    ".idea",
    ".vs",
    "vendor",
    "Pods",
    ".svn",
    ".hg",
    ".ssh",
    ".gnupg",
];

/// Extensions of files that are never text: left out without reading them.
const BINARY_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "ico", "icns", "webp", "tif", "tiff", "psd", "pdf", "zip",
    "gz", "tgz", "bz2", "xz", "7z", "rar", "zst", "jar", "war", "exe", "dll", "so", "dylib", "a",
    "lib", "o", "obj", "pdb", "class", "pyc", "wasm", "woff", "woff2", "ttf", "otf", "eot", "mp3",
    "mp4", "m4a", "wav", "ogg", "flac", "mov", "avi", "mkv", "webm", "gguf", "bin", "db", "sqlite",
    "sqlite3", "rlib", "rmeta", "node",
];

/// One file in an index.
#[derive(Debug, Clone)]
pub struct FileEntry {
    /// Inside the folder, with `/`.
    pub rel: Box<str>,
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// Definitions read, `None` until the file was read.
    symbols: Option<Arc<[Symbol]>>,
    /// The file turned out to be binary or unreadable: not shown.
    hidden: bool,
}

impl FileEntry {
    pub fn symbols(&self) -> &[Symbol] {
        self.symbols.as_deref().unwrap_or(&[])
    }
}

/// What an index holds at one moment.
#[derive(Debug, Default)]
pub struct Snapshot {
    pub files: Vec<FileEntry>,
    /// Names and definitions are complete.
    pub complete: bool,
    /// The folder has more files than [`MAX_FILES`].
    pub truncated: bool,
}

/// The index of one approved folder.
#[derive(Debug)]
pub struct FolderIndex {
    root: PathBuf,
    sandbox: PathSandbox,
    skill_roots: Vec<String>,
    current: RwLock<Arc<Snapshot>>,
    building: AtomicBool,
    cancelled: AtomicBool,
    built_at: Mutex<Option<Instant>>,
    max_files: usize,
}

impl FolderIndex {
    /// An index of `root` (canonical), not built yet. `forbidden` and `skill_roots` are the
    /// app's, as the chat context holds them.
    pub fn new(root: &str, forbidden: &[String], skill_roots: &[String]) -> Arc<Self> {
        let (sandbox, _) = PathSandbox::new(
            Flavor::native(),
            &[root.to_string()],
            forbidden,
            &RealFs,
            &|p| volume_case(p),
        );
        Arc::new(Self {
            root: PathBuf::from(root),
            sandbox,
            skill_roots: skill_roots.to_vec(),
            current: RwLock::new(Arc::new(Snapshot::default())),
            building: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            built_at: Mutex::new(None),
            max_files: MAX_FILES,
        })
    }

    /// The same with a smaller file cap (tests).
    #[cfg(test)]
    fn with_max_files(root: &str, max_files: usize) -> Arc<Self> {
        let idx = Self::new(root, &[], &[]);
        let mut idx = Arc::try_unwrap(idx).expect("new index");
        idx.max_files = max_files;
        Arc::new(idx)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.current
            .read()
            .map(|s| Arc::clone(&s))
            .unwrap_or_default()
    }

    pub fn is_building(&self) -> bool {
        self.building.load(Ordering::Acquire)
    }

    /// Starts a build on a background thread unless one runs or the last one is recent.
    pub fn refresh(self: &Arc<Self>) {
        let due = self
            .built_at
            .lock()
            .map(|at| at.is_none_or(|t| t.elapsed() >= REFRESH_AFTER))
            .unwrap_or(false);
        if !due || self.building.swap(true, Ordering::AcqRel) {
            return;
        }
        let me = Arc::clone(self);
        let started = std::thread::Builder::new()
            .name("mention-index".into())
            .spawn(move || {
                me.build();
                if let Ok(mut at) = me.built_at.lock() {
                    *at = Some(Instant::now());
                }
                me.building.store(false, Ordering::Release);
            });
        if started.is_err() {
            self.building.store(false, Ordering::Release);
        }
    }

    /// Builds on this thread (tests and the background thread).
    pub fn build_now(&self) {
        self.build();
        if let Ok(mut at) = self.built_at.lock() {
            *at = Some(Instant::now());
        }
    }

    /// Waits up to `limit` for a running build to finish.
    pub fn wait(&self, limit: Duration) {
        let until = Instant::now() + limit;
        while self.is_building() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    fn publish(&self, snap: Snapshot) {
        if let Ok(mut cur) = self.current.write() {
            *cur = Arc::new(snap);
        }
    }

    fn build(&self) {
        let old = self.snapshot();
        let (listed, truncated) = self.list();
        if self.cancelled.load(Ordering::Acquire) {
            return;
        }
        // Reuse what was read of a file that did not change.
        let mut by_rel: std::collections::HashMap<&str, &FileEntry> =
            old.files.iter().map(|f| (&*f.rel, f)).collect();
        let mut files: Vec<FileEntry> = listed
            .into_iter()
            .map(|mut f| {
                if let Some(prev) = by_rel.remove(&*f.rel)
                    && prev.size == f.size
                    && prev.modified == f.modified
                {
                    f.symbols.clone_from(&prev.symbols);
                    f.hidden = prev.hidden;
                }
                f
            })
            .collect();
        drop(by_rel);
        let publish = |files: &[FileEntry], complete: bool| {
            self.publish(Snapshot {
                files: files.to_vec(),
                complete,
                truncated,
            });
        };
        // Names first, as long as the old list is not more complete.
        if !old.complete || old.files.is_empty() {
            publish(&files, false);
        }
        let mut total_symbols: usize = files.iter().map(|f| f.symbols().len()).sum();
        let mut since = 0usize;
        for k in 0..files.len() {
            if self.cancelled.load(Ordering::Acquire) {
                return;
            }
            if files[k].symbols.is_some() {
                continue;
            }
            let path = self.root.join(&*files[k].rel);
            let (symbols, hidden) = read_symbols(&path, &files[k].rel, MAX_SYMBOLS - total_symbols);
            total_symbols += symbols.len();
            files[k].symbols = Some(symbols.into());
            files[k].hidden = hidden;
            since += 1;
            if since >= PUBLISH_EVERY && (!old.complete || old.files.is_empty()) {
                publish(&files, false);
                since = 0;
            }
        }
        publish(&files, true);
    }

    /// The files of the folder, breadth first, without reading them.
    fn list(&self) -> (Vec<FileEntry>, bool) {
        let mut out = Vec::new();
        if !self.allowed(&self.root) {
            return (out, false);
        }
        let mut queue: VecDeque<(PathBuf, String)> = VecDeque::new();
        queue.push_back((self.root.clone(), String::new()));
        while let Some((dir, rel)) = queue.pop_front() {
            if self.cancelled.load(Ordering::Acquire) {
                break;
            }
            let Ok(rd) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut entries: Vec<_> = rd.filter_map(Result::ok).collect();
            entries.sort_by_key(|e| e.file_name());
            for e in entries {
                let Ok(ft) = e.file_type() else { continue };
                // Links are never followed (13.4): a link may lead out of the folder.
                if ft.is_symlink() {
                    continue;
                }
                let name = e.file_name().to_string_lossy().into_owned();
                let child_rel = if rel.is_empty() {
                    name.clone()
                } else {
                    format!("{rel}/{name}")
                };
                let path = e.path();
                if ft.is_dir() {
                    if SKIP_DIRS.contains(&name.as_str()) || !self.allowed(&path) {
                        continue;
                    }
                    queue.push_back((path, child_rel));
                    continue;
                }
                if !ft.is_file() || skipped_by_name(&name) {
                    continue;
                }
                let Ok(meta) = e.metadata() else { continue };
                if meta.len() > MAX_FILE_BYTES {
                    continue;
                }
                // Skill folders were refused with their folder; this is the per-file part
                // (`.bashrc`, `.git-credentials`, ...).
                if always_ask_reason(
                    &path.display().to_string(),
                    WriteKind::Edit,
                    Flavor::native(),
                )
                .is_some()
                {
                    continue;
                }
                if out.len() >= self.max_files {
                    return (out, true);
                }
                out.push(FileEntry {
                    rel: child_rel.into(),
                    size: meta.len(),
                    modified: meta.modified().ok(),
                    symbols: None,
                    hidden: false,
                });
            }
        }
        (out, false)
    }

    /// Whether the walk may enter the folder `path`: the sandbox lets it through (not one
    /// of the app's own folders) and mentions may read inside it.
    fn allowed(&self, path: &Path) -> bool {
        let shown = path.display().to_string();
        self.sandbox.check(&shown, None, &RealFs).is_ok()
            && off_limits_for(&shown, &self.skill_roots).is_none()
    }
}

fn skipped_by_name(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty() && BINARY_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())
    })
}

/// The definitions of one file, and whether the file is to be hidden (binary, unreadable).
/// Files of a language the finder does not know are only checked for being binary.
fn read_symbols(path: &Path, rel: &str, room: usize) -> (Vec<Symbol>, bool) {
    let lang = Lang::from_path(rel);
    let Ok(f) = std::fs::File::open(path) else {
        return (Vec::new(), true);
    };
    let limit = if lang.is_some() { MAX_FILE_BYTES } else { 8192 };
    let mut bytes = Vec::new();
    if f.take(limit).read_to_end(&mut bytes).is_err() {
        return (Vec::new(), true);
    }
    if bytes.iter().take(8192).any(|&b| b == 0) {
        return (Vec::new(), true);
    }
    let Some(lang) = lang else {
        return (Vec::new(), false);
    };
    let mut symbols = find_symbols(&String::from_utf8_lossy(&bytes), lang);
    symbols.truncate(room);
    (symbols, false)
}

/// The indexes of the process: one per approved folder, at most [`MAX_FOLDERS`].
#[derive(Debug, Default)]
pub struct Indexes {
    folders: Mutex<Vec<Arc<FolderIndex>>>,
}

impl Indexes {
    pub fn new() -> Self {
        Self::default()
    }

    /// The indexes for these approved folders (canonical), created and refreshed as
    /// needed. A folder inside another one of the list is covered by the outer one.
    pub fn for_roots(
        &self,
        roots: &[String],
        forbidden: &[String],
        skill_roots: &[String],
    ) -> Vec<Arc<FolderIndex>> {
        let outer: Vec<&String> = roots
            .iter()
            .filter(|r| {
                !roots
                    .iter()
                    .any(|o| o != *r && Path::new(r.as_str()).starts_with(o.as_str()))
            })
            .collect();
        let Ok(mut folders) = self.folders.lock() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for root in outer.into_iter().take(MAX_FOLDERS) {
            let pos = folders
                .iter()
                .position(|f| f.root == Path::new(root.as_str()) && f.skill_roots == skill_roots);
            let idx = match pos {
                // Most recently used last.
                Some(p) => folders.remove(p),
                None => FolderIndex::new(root, forbidden, skill_roots),
            };
            folders.push(Arc::clone(&idx));
            out.push(idx);
        }
        while folders.len() > MAX_FOLDERS {
            folders.remove(0).cancel();
        }
        drop(folders);
        for idx in &out {
            idx.refresh();
        }
        out
    }
}

/// One search result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// The file, absolute.
    pub path: PathBuf,
    /// The file inside its folder, with `/`.
    pub rel: String,
    /// `None` for the file itself.
    pub symbol: Option<HitSymbol>,
    pub score: i32,
    pub modified: Option<SystemTime>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HitSymbol {
    pub name: String,
    pub qualified: String,
    pub kind: SymbolKind,
    pub container: Option<String>,
    pub line: u32,
    pub end_line: u32,
}

/// What a search found.
#[derive(Debug, Clone, Default)]
pub struct Found {
    pub hits: Vec<Hit>,
    /// Some index is still being built: asking again may find more.
    pub indexing: bool,
    /// Files searched.
    pub files: usize,
}

/// Searches `indexes` for `query`: files by a fuzzy match on their path and definitions by
/// their name. `path#name` looks for definitions called `name` in files matching `path`.
/// An empty query lists the most recently changed files.
pub fn search(indexes: &[Arc<FolderIndex>], query: &str, limit: usize) -> Found {
    let query = query.trim();
    let limit = limit.clamp(1, 100);
    let (path_q, sym_q) = match query.split_once('#') {
        Some((p, s)) => (p.trim(), Some(s.trim())),
        None => (query, None),
    };
    let mut found = Found::default();
    let mut hits: Vec<Hit> = Vec::new();
    for idx in indexes {
        let snap = idx.snapshot();
        found.indexing |= !snap.complete || idx.is_building();
        found.files += snap.files.len();
        for f in snap.files.iter().filter(|f| !f.hidden) {
            let file_score = if path_q.is_empty() {
                Some(0)
            } else {
                score_path(&f.rel, path_q.as_bytes())
            };
            let lang = Lang::from_path(&f.rel);
            match sym_q {
                None => {
                    if query.is_empty() {
                        hits.push(file_hit(idx, f, 0));
                        continue;
                    }
                    if let Some(sc) = file_score {
                        hits.push(file_hit(idx, f, sc));
                    }
                    for s in f.symbols() {
                        if let Some(sc) = score_name(&s.name, path_q.as_bytes(), s.kind) {
                            hits.push(symbol_hit(idx, f, s, lang, sc));
                        }
                    }
                }
                Some(sq) => {
                    let Some(fs) = file_score else { continue };
                    for s in f.symbols() {
                        let sc = if sq.is_empty() {
                            Some(0)
                        } else {
                            score_name(&s.name, sq.as_bytes(), s.kind)
                        };
                        if let Some(sc) = sc {
                            hits.push(symbol_hit(idx, f, s, lang, sc + fs / 10));
                        }
                    }
                }
            }
        }
    }
    if query.is_empty() {
        // Most recently changed first.
        hits.sort_by_key(|h| std::cmp::Reverse(h.modified));
    } else {
        let k = limit.min(hits.len());
        if k > 0 && k < hits.len() {
            hits.select_nth_unstable_by(k - 1, cmp_hits);
            hits.truncate(k);
        }
        hits.sort_by(cmp_hits);
    }
    hits.truncate(limit);
    found.hits = hits;
    found
}

fn cmp_hits(a: &Hit, b: &Hit) -> std::cmp::Ordering {
    b.score
        .cmp(&a.score)
        .then_with(|| a.rel.len().cmp(&b.rel.len()))
        .then_with(|| a.rel.cmp(&b.rel))
        .then_with(|| {
            let line = |h: &Hit| h.symbol.as_ref().map_or(0, |s| s.line);
            line(a).cmp(&line(b))
        })
}

fn file_hit(idx: &FolderIndex, f: &FileEntry, score: i32) -> Hit {
    Hit {
        path: idx.root.join(&*f.rel),
        rel: f.rel.to_string(),
        symbol: None,
        score,
        modified: f.modified,
    }
}

fn symbol_hit(idx: &FolderIndex, f: &FileEntry, s: &Symbol, lang: Option<Lang>, score: i32) -> Hit {
    Hit {
        path: idx.root.join(&*f.rel),
        rel: f.rel.to_string(),
        symbol: Some(HitSymbol {
            name: s.name.to_string(),
            qualified: lang.map_or_else(|| s.name.to_string(), |l| s.qualified(l)),
            kind: s.kind,
            container: s.container.as_deref().map(str::to_string),
            line: s.line,
            end_line: s.end_line,
        }),
        score,
        modified: f.modified,
    }
}

/// `needle` occurs in `hay`, ignoring ASCII case: where.
fn find_ci(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    if needle.len() > hay.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&i| hay[i..i + needle.len()].eq_ignore_ascii_case(needle))
}

/// The letters of `needle` appear in `hay` in order, ignoring ASCII case: how spread out.
fn subsequence_ci(hay: &[u8], needle: &[u8]) -> Option<usize> {
    let mut it = needle.iter().peekable();
    let (mut first, mut last) = (None, 0usize);
    for (i, &b) in hay.iter().enumerate() {
        let Some(&&n) = it.peek() else { break };
        if b.eq_ignore_ascii_case(&n) {
            first.get_or_insert(i);
            last = i;
            it.next();
        }
    }
    if it.peek().is_some() {
        return None;
    }
    Some(last + 1 - first.unwrap_or(0) - needle.len())
}

/// How well a file's path matches: its name first, then the whole path, then the letters
/// in order.
fn score_path(rel: &str, q: &[u8]) -> Option<i32> {
    let bytes = rel.as_bytes();
    let base = rel.rfind('/').map_or(0, |p| p + 1);
    let name = &bytes[base..];
    let len_penalty = (bytes.len() / 8) as i32;
    if name.eq_ignore_ascii_case(q) {
        return Some(1000 - len_penalty);
    }
    if name.len() >= q.len() && name[..q.len()].eq_ignore_ascii_case(q) {
        return Some(900 - (name.len() - q.len()) as i32 - len_penalty);
    }
    if let Some(p) = find_ci(name, q) {
        return Some(750 - p as i32 - len_penalty);
    }
    if let Some(p) = find_ci(bytes, q) {
        return Some(600 - (p / 4) as i32 - len_penalty);
    }
    let spread = subsequence_ci(bytes, q)?;
    Some((300 - spread as i32 - len_penalty).max(1))
}

/// How well a definition's name matches.
fn score_name(name: &str, q: &[u8], kind: SymbolKind) -> Option<i32> {
    let bytes = name.as_bytes();
    // An impl block repeats its type's name: below the type.
    let kind_penalty = if kind == SymbolKind::Impl { 60 } else { 0 };
    let score = if bytes.eq_ignore_ascii_case(q) {
        1100
    } else if bytes.len() >= q.len() && bytes[..q.len()].eq_ignore_ascii_case(q) {
        950 - (bytes.len() - q.len()) as i32
    } else if let Some(p) = find_ci(bytes, q) {
        700 - p as i32
    } else {
        let spread = subsequence_ci(bytes, q)?;
        (250 - spread as i32 * 4 - bytes.len() as i32).max(1)
    };
    Some(score - kind_penalty)
}

#[cfg(test)]
mod tests;
