//! The gallery folder (0.1.0 Studio): where results, their records, thumbnails and the
//! pictures a user adds are kept, and the type check a file passes before it is kept.
//!
//! - **Layout.** `<root>/YYYY-MM/<run_id>-<n>.<ext>` for results (the month the run was
//!   made, on this computer's clock), `<root>/YYYY-MM/<run_id>.json` for each run's record
//!   (its sidecar, so the history survives a lost database; the run record never holds a
//!   key), `<root>/refs/<sha256>.<ext>` for added pictures, `<root>/.thumbs/<output_id>.jpg`
//!   for thumbnails and `<root>/.partial/` for downloads and local runs in progress. The
//!   default root is `<home>/harnessUI/studio`, next to the models folder.
//! - **Type check.** A result is kept only as one of seven types (`MEDIA_TYPES`), read
//!   from its first bytes ([`sniff_media`]); a company's own word for the type is checked
//!   against them and never becomes a file extension ([`Gallery::finalize`]).
//! - **Added pictures** pass the chat composer's checks (`attachments::checked_picture`:
//!   20 MiB, the bytes are the type claimed, WebP turned into JPEG or PNG) and are kept once
//!   by content hash.

use std::collections::BTreeSet;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use jiff::tz::TimeZone;
use sha2::{Digest, Sha256};

use crate::attachments::{ImageInput, PictureWorker, checked_picture};
use crate::error::AppResult;

/// The types the gallery keeps, with the extension each is saved under: PNG, JPEG, WebP,
/// GIF and SVG pictures, MP4 and WebM videos.
const MEDIA_TYPES: [(&str, &str); 7] = [
    (PNG, "png"),
    (JPEG, "jpg"),
    (WEBP, "webp"),
    (GIF, "gif"),
    (SVG, "svg"),
    (MP4, "mp4"),
    (WEBM, "webm"),
];

/// The types an added picture can be kept as.
const PICTURE_TYPES: [(&str, &str); 4] =
    [(PNG, "png"), (JPEG, "jpg"), (WEBP, "webp"), (GIF, "gif")];

const PNG: &str = "image/png";
const JPEG: &str = "image/jpeg";
const WEBP: &str = "image/webp";
const GIF: &str = "image/gif";
const SVG: &str = "image/svg+xml";
const MP4: &str = "video/mp4";
const WEBM: &str = "video/webm";

/// The major brands (bytes 8..12, after `ftyp` at byte 4) of ISO media files that are not
/// MP4 video: HEIF and AVIF still pictures and image sequences, and QuickTime movies. Every
/// other brand is MP4.
const NOT_MP4_BRANDS: [&[u8]; 11] = [
    b"avif", b"avis", b"heic", b"heix", b"heim", b"heis", b"hevc", b"hevx", b"mif1", b"msf1",
    b"qt  ",
];

/// How much of a file is read to tell its type: enough for an SVG's XML declaration,
/// comments and document type before its first element.
const SNIFF_BYTES: usize = 8 * 1024;

/// Why a file was not kept, or a gallery file could not be reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GalleryError {
    /// The disk (or the user's quota on it) is full.
    DiskFull,
    /// The file is larger than its cap, in bytes.
    TooLarge(u64),
    /// The file is none of the seven kept types.
    NotMedia,
    /// The file is a kept type, but not the one the company said it was.
    Mismatch {
        /// The type the company gave.
        claimed: String,
        /// The type its bytes are.
        found: String,
    },
    /// An added picture's name is not a SHA-256 (64 lowercase hex characters).
    BadName,
    /// Reading or writing failed.
    Io(String),
}

impl std::fmt::Display for GalleryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DiskFull => f.write_str("the disk is full"),
            Self::TooLarge(cap) => write!(f, "the file is larger than {cap} bytes"),
            Self::NotMedia => {
                f.write_str("the file is not a PNG, JPEG, WebP, GIF, SVG, MP4 or WebM file")
            }
            Self::Mismatch { claimed, found } => {
                write!(f, "the file was sent as {claimed} but is {found}")
            }
            Self::BadName => f.write_str("the name of an added picture is not a SHA-256"),
            Self::Io(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for GalleryError {}

/// A reading or writing error as a [`GalleryError`]: a full disk, or a full quota, is
/// [`GalleryError::DiskFull`], so the card can say so.
pub fn io_error(e: std::io::Error) -> GalleryError {
    match e.kind() {
        ErrorKind::StorageFull | ErrorKind::QuotaExceeded => GalleryError::DiskFull,
        _ => GalleryError::Io(e.to_string()),
    }
}

/// An added picture as kept in `refs/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefStored {
    /// The SHA-256 of the bytes kept (after a WebP picture became JPEG or PNG), in lowercase
    /// hex: the picture's name.
    pub sha256: String,
    /// The type kept: PNG, JPEG or GIF.
    pub mime: String,
    /// The size kept.
    pub size_bytes: u64,
}

/// The type a file is, read from its first bytes: one of the seven types the gallery keeps
/// (PNG, JPEG, WebP, GIF, SVG, MP4 with `ftyp` at byte 4, WebM by its EBML header), or
/// `None`. A file with `ftyp` at byte 4 whose major brand is a HEIF or AVIF picture's or a
/// QuickTime movie's (`NOT_MP4_BRANDS`) is `None`; any other brand is MP4. An SVG is text
/// whose first element, after any XML declaration, comments and an `svg` document type, is
/// `<svg` within the first 8 KiB (`SNIFF_BYTES`: only those are read, however many bytes
/// are given).
pub fn sniff_media(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(PNG)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(JPEG)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(GIF)
    } else if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(WEBP)
    } else if bytes.len() >= 8 && &bytes[4..8] == b"ftyp" {
        match bytes.get(8..12) {
            Some(brand) if NOT_MP4_BRANDS.contains(&brand) => None,
            _ => Some(MP4),
        }
    } else if bytes.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        Some(WEBM)
    } else if svg_start(&bytes[..bytes.len().min(SNIFF_BYTES)]) == SvgStart::Svg {
        Some(SVG)
    } else {
        None
    }
}

/// What the start of a file says about it being an SVG.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SvgStart {
    /// Its first element is `<svg`.
    Svg,
    /// Something else comes first: other text, another element, another document type.
    NotSvg,
    /// The bytes end before the first element (inside or after the XML declaration,
    /// comments or document type), so they cannot tell.
    CannotTell,
}

/// Reads past a byte-order mark, white space, `<?…?>`, `<!--…-->` and an `svg` document
/// type (with its internal subset) to the first element.
fn svg_start(bytes: &[u8]) -> SvgStart {
    let mut rest = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    loop {
        rest = rest.trim_ascii_start();
        let Some(tag) = rest.strip_prefix(b"<") else {
            return if rest.is_empty() {
                SvgStart::CannotTell
            } else {
                SvgStart::NotSvg
            };
        };
        let skip = if tag.starts_with(b"?") {
            find(tag, b"?>").map(|at| at + 2)
        } else if tag.starts_with(b"!--") {
            find(&tag[3..], b"-->").map(|at| 3 + at + 3)
        } else if tag.len() >= 8 && tag[..8].eq_ignore_ascii_case(b"!DOCTYPE") {
            match svg_doctype_len(&tag[8..]) {
                Ok(Some(len)) => Some(8 + len),
                Ok(None) => None,
                Err(()) => return SvgStart::NotSvg,
            }
        } else {
            return match svg_name(tag, b">/") {
                Some(true) => SvgStart::Svg,
                Some(false) => SvgStart::NotSvg,
                None => SvgStart::CannotTell,
            };
        };
        match skip {
            Some(len) => rest = &tag[len..],
            None => return SvgStart::CannotTell,
        }
    }
}

/// The length of a document type's body after `<!DOCTYPE`, up to and with its closing `>`:
/// `Err` when it is not an `svg` document type, `Ok(None)` when the bytes end inside it.
fn svg_doctype_len(body: &[u8]) -> Result<Option<usize>, ()> {
    let name = body.trim_ascii_start();
    let skipped = body.len() - name.len();
    match svg_name(name, b">[") {
        None => return Ok(None),
        Some(true) if skipped > 0 => {}
        Some(_) => return Err(()),
    }
    // An internal subset (`[…]`) holds `>` of its own: the document type then ends at the
    // first `>` after the subset's `]`. Only a `[` before the first `>` opens one, so the
    // search stops there and each document type costs only its own length.
    let close = find(name, b">");
    let end = match find(&name[..close.unwrap_or(name.len())], b"[") {
        Some(open) => find(&name[open..], b"]")
            .map(|at| open + at)
            .and_then(|shut| find(&name[shut..], b">").map(|at| shut + at)),
        None => close,
    };
    Ok(end.map(|at| skipped + at + 1))
}

/// Whether `bytes` start with the name `svg` followed by white space or one of `ends`;
/// `None` when they end before that can be told.
fn svg_name(bytes: &[u8], ends: &[u8]) -> Option<bool> {
    match bytes.get(3) {
        None => (!b"svg".starts_with(bytes)).then_some(false),
        Some(next) => {
            Some(bytes.starts_with(b"svg") && (next.is_ascii_whitespace() || ends.contains(next)))
        }
    }
}

/// Where `needle` first starts in `hay`.
fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// The extension a kept type is saved under (every type it is given is one of
/// [`MEDIA_TYPES`], so the fallback is never used).
fn extension_of(mime: &str) -> &'static str {
    MEDIA_TYPES
        .iter()
        .find(|(m, _)| *m == mime)
        .map_or("bin", |(_, ext)| ext)
}

/// `YYYY-MM` of the instant `ms` in `tz`; an instant jiff cannot represent counts as the
/// Unix epoch's month.
fn month_in(tz: &TimeZone, ms: i64) -> String {
    let at = Timestamp::from_millisecond(ms)
        .unwrap_or(Timestamp::UNIX_EPOCH)
        .to_zoned(tz.clone());
    format!("{:04}-{:02}", at.year(), at.month())
}

/// `path` with `.<ext>` added (a dot in the name is kept, unlike `with_extension`).
fn with_ext(path: &Path, ext: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".");
    name.push(ext);
    PathBuf::from(name)
}

/// Whether `name` is a SHA-256 as the gallery writes it: 64 lowercase hex characters.
fn is_sha256(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Writes `bytes` to `path` through a temporary file beside it and a rename, making its
/// folder first, so a reader never sees half a file. The temporary file is removed when
/// either step fails.
pub(super) fn write_whole(path: &Path, bytes: &[u8]) -> Result<(), GalleryError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io_error)?;
    }
    let tmp = with_ext(path, &format!("{}.tmp", uuid::Uuid::new_v4().simple()));
    if let Err(e) = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(io_error(e));
    }
    Ok(())
}

/// Removes a file; one already gone is fine, any other failure goes to the log.
fn remove_quietly(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "a Studio file could not be removed")
        }
    }
}

/// How the type the company gave counts when the bytes are another of the seven.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Claim {
    /// The file is refused ([`GalleryError::Mismatch`]).
    Checked,
    /// The bytes win, and the type given is only logged: a result already paid for.
    BytesWin,
}

/// The type a file at `partial` may be kept as, read from its first bytes and checked
/// against `claimed` (as `claim` says) and `cap`. The file is closed before this returns.
fn check_partial(
    partial: &Path,
    claimed: Option<&str>,
    claim: Claim,
    cap: u64,
) -> Result<&'static str, GalleryError> {
    let mut file = std::fs::File::open(partial).map_err(io_error)?;
    let size = file.metadata().map_err(io_error)?.len();
    if size > cap {
        return Err(GalleryError::TooLarge(cap));
    }
    let mut head = Vec::with_capacity(SNIFF_BYTES);
    file.by_ref()
        .take(SNIFF_BYTES as u64)
        .read_to_end(&mut head)
        .map_err(io_error)?;
    drop(file);
    let whole = head.len() as u64 >= size;
    match media_type(&head, whole, claimed) {
        Err(GalleryError::Mismatch { claimed, found }) if claim == Claim::BytesWin => {
            tracing::warn!(
                claimed = %claimed,
                found = %found,
                "a Studio result was sent as one type and is another: kept as its bytes are"
            );
            MEDIA_TYPES
                .iter()
                .map(|(m, _)| *m)
                .find(|m| *m == found)
                .ok_or(GalleryError::NotMedia)
        }
        other => other,
    }
}

/// The type to keep a file as, from its first bytes `head` (`whole` when they are the whole
/// file) and the type the company gave. The bytes decide; a given type that is one of the
/// seven and differs is a mismatch, and any other given type is ignored. The given type is
/// used only for an SVG whose start cannot tell, cut off before its first element.
fn media_type(
    head: &[u8],
    whole: bool,
    claimed: Option<&str>,
) -> Result<&'static str, GalleryError> {
    let claimed = claimed.and_then(|c| {
        let c = c
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        MEDIA_TYPES.iter().map(|(m, _)| *m).find(|m| *m == c)
    });
    match (sniff_media(head), claimed) {
        (Some(found), Some(claimed)) if claimed != found => Err(GalleryError::Mismatch {
            claimed: claimed.to_owned(),
            found: found.to_owned(),
        }),
        (Some(found), _) => Ok(found),
        (None, Some(SVG)) if !whole && svg_start(head) == SvgStart::CannotTell => Ok(SVG),
        (None, _) => Err(GalleryError::NotMedia),
    }
}

/// The file of the added picture named `sha256` in the first of `galleries` whose `refs/`
/// has it, and its type: the gallery folder now, then the earlier ones, whose added pictures
/// stay where they were when Settings → Studio moved the gallery. Refused as
/// [`Gallery::ref_file`] refuses (a name that is not a SHA-256; `GalleryError::Io` when none
/// has it).
pub fn ref_file_in(galleries: &[Gallery], sha256: &str) -> Result<(PathBuf, String), GalleryError> {
    let mut missing = GalleryError::Io(format!(
        "the added picture {sha256} is not in the gallery folder"
    ));
    for gallery in galleries {
        match gallery.ref_file(sha256) {
            Ok(found) => return Ok(found),
            Err(GalleryError::Io(why)) => missing = GalleryError::Io(why),
            Err(e) => return Err(e),
        }
    }
    Err(missing)
}

/// The gallery folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gallery {
    root: PathBuf,
}

impl Gallery {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// The default gallery folder: `<home>/<app_name>/studio`, next to the models folder,
    /// outside app data (which the Windows uninstaller may delete).
    pub fn default_root(home: &Path, app_name: &str) -> PathBuf {
        home.join(app_name).join("studio")
    }

    /// The gallery folder itself.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `<root>/.partial`, made if missing, for downloads and local runs in progress. A
    /// folder that cannot be made shows up as a failure to write into it.
    pub fn partial_dir(&self) -> PathBuf {
        let dir = self.root.join(".partial");
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    /// `<root>/YYYY-MM/<run_id>-<idx>`: where an output goes before its extension is known
    /// ([`Self::finalize`] adds it).
    pub fn output_stem(&self, created_ms: i64, run_id: &str, idx: u32) -> PathBuf {
        self.month_dir(created_ms).join(format!("{run_id}-{idx}"))
    }

    /// `<root>/YYYY-MM/<run_id>-<idx>.<ext>`: where the tests put a result (the app keeps
    /// one through [`Self::finalize`], which adds the extension its bytes call for).
    #[cfg(test)]
    pub fn output_path(&self, created_ms: i64, run_id: &str, idx: u32, ext: &str) -> PathBuf {
        with_ext(&self.output_stem(created_ms, run_id, idx), ext)
    }

    /// `<root>/YYYY-MM/<run_id>.json`, the run's record.
    pub fn sidecar_path(&self, created_ms: i64, run_id: &str) -> PathBuf {
        self.month_dir(created_ms).join(format!("{run_id}.json"))
    }

    /// `<root>/.thumbs/<output_id>.jpg`.
    pub fn thumb_path(&self, output_id: &str) -> PathBuf {
        self.root.join(".thumbs").join(format!("{output_id}.jpg"))
    }

    /// The folder of the month `created_ms` falls in, on this computer's clock.
    fn month_dir(&self, created_ms: i64) -> PathBuf {
        self.root.join(month_in(&TimeZone::system(), created_ms))
    }

    fn refs_dir(&self) -> PathBuf {
        self.root.join("refs")
    }

    /// Keeps the file at `partial` as `<target_no_ext>.<ext>` and returns that path and its
    /// type. The type comes from the file's bytes; `claimed` is the type the company gave,
    /// if any, and decides only for an SVG whose start cannot tell. A file over `cap` bytes,
    /// one that is none of the seven types, and one the company called another of the seven
    /// are refused, and on every refusal or failure the partial file is removed.
    pub fn finalize(
        &self,
        partial: &Path,
        claimed: Option<&str>,
        target_no_ext: &Path,
        cap: u64,
    ) -> Result<(PathBuf, String), GalleryError> {
        self.keep(partial, claimed, Claim::Checked, target_no_ext, cap)
    }

    /// [`Self::finalize`] for a result already paid for (a cloud company's): when the type
    /// the company gave is another of the seven than the file's bytes, the file is kept as
    /// its bytes are and the type given is only logged, so a charged result is never thrown
    /// away for a wrong label.
    pub fn finalize_paid(
        &self,
        partial: &Path,
        claimed: Option<&str>,
        target_no_ext: &Path,
        cap: u64,
    ) -> Result<(PathBuf, String), GalleryError> {
        self.keep(partial, claimed, Claim::BytesWin, target_no_ext, cap)
    }

    /// [`Self::finalize`] and [`Self::finalize_paid`].
    fn keep(
        &self,
        partial: &Path,
        claimed: Option<&str>,
        claim: Claim,
        target_no_ext: &Path,
        cap: u64,
    ) -> Result<(PathBuf, String), GalleryError> {
        let mime = match check_partial(partial, claimed, claim, cap) {
            Ok(mime) => mime,
            Err(e) => {
                remove_quietly(partial);
                return Err(e);
            }
        };
        let target = with_ext(target_no_ext, extension_of(mime));
        let moved = target
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::rename(partial, &target));
        if let Err(e) = moved {
            remove_quietly(partial);
            return Err(io_error(e));
        }
        Ok((target, mime.to_owned()))
    }

    /// Writes output `output_id`'s thumbnail (JPEG bytes) to `<root>/.thumbs/<output_id>.jpg`,
    /// replacing an earlier one, and returns its path.
    pub fn write_thumb(&self, output_id: &str, jpeg: &[u8]) -> Result<PathBuf, GalleryError> {
        let path = self.thumb_path(output_id);
        write_whole(&path, jpeg)?;
        Ok(path)
    }

    /// Writes the run's record as its sidecar, replacing an earlier one.
    pub fn write_sidecar(&self, run: &store::CreationRun) -> Result<(), GalleryError> {
        let json = serde_json::to_vec_pretty(run).map_err(|e| GalleryError::Io(e.to_string()))?;
        write_whole(&self.sidecar_path(run.created_at, &run.id), &json)
    }

    /// Checks a picture the user added, as a chat picture is checked (at most 20 MiB, its
    /// bytes the type it claims, a WebP picture turned into JPEG or PNG), and keeps it once,
    /// as `refs/<sha256>.<ext>`.
    pub async fn add_ref(&self, worker: &PictureWorker, img: &ImageInput) -> AppResult<RefStored> {
        let (mime, ext, bytes) = checked_picture(worker, img).await?;
        let sha256: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let path = self.refs_dir().join(format!("{sha256}.{ext}"));
        if !path.exists() {
            write_whole(&path, &bytes).map_err(|e| super::errors::from_gallery(&e))?;
        }
        Ok(RefStored {
            sha256,
            mime,
            size_bytes: bytes.len() as u64,
        })
    }

    /// The file of the added picture named `sha256`, and its type. A name that is not 64
    /// lowercase hex characters is refused, so it can never reach outside `refs/`; a name
    /// with no file of any picture type is `GalleryError::Io` (the only `Io` it gives).
    pub fn ref_file(&self, sha256: &str) -> Result<(PathBuf, String), GalleryError> {
        if !is_sha256(sha256) {
            return Err(GalleryError::BadName);
        }
        let dir = self.refs_dir();
        PICTURE_TYPES
            .iter()
            .map(|(mime, ext)| (dir.join(format!("{sha256}.{ext}")), *mime))
            .find(|(path, _)| path.is_file())
            .map(|(path, mime)| (path, mime.to_owned()))
            .ok_or_else(|| {
                GalleryError::Io(format!(
                    "the added picture {sha256} is not in the gallery folder"
                ))
            })
    }

    /// Whether `refs/` holds an added picture (a `<sha256>.<picture type>` file).
    pub fn holds_refs(&self) -> bool {
        let Ok(entries) = std::fs::read_dir(self.refs_dir()) else {
            return false;
        };
        entries.flatten().any(|entry| {
            let name = entry.file_name();
            entry.file_type().is_ok_and(|kind| kind.is_file())
                && name
                    .to_str()
                    .and_then(|name| name.split_once('.'))
                    .is_some_and(|(sha256, ext)| {
                        is_sha256(sha256) && PICTURE_TYPES.iter().any(|(_, known)| *known == ext)
                    })
        })
    }

    /// Removes what a delete left behind: each output's file and thumbnail, and each deleted
    /// run's sidecar (also beside its outputs, for a run kept in an earlier gallery folder).
    /// Files already gone are fine; other failures go to the log.
    pub fn remove_files(&self, deleted: &store::Deleted) {
        for out in &deleted.outputs {
            remove_quietly(Path::new(&out.path));
            if let Some(thumb) = &out.thumb_path {
                remove_quietly(Path::new(thumb));
            }
            remove_quietly(&self.thumb_path(&out.id));
        }
        for run in &deleted.runs {
            remove_quietly(&self.sidecar_path(run.created_at, &run.id));
            for out in &run.outputs {
                if let Some(dir) = Path::new(&out.path).parent() {
                    remove_quietly(&dir.join(format!("{}.json", run.id)));
                }
            }
        }
    }

    /// Removes each added picture in `refs/` (`<sha256>.<ext>` of a picture type) that is
    /// not `in_use` by a run still kept. Any other file is left as it is.
    pub fn remove_unused_refs(&self, in_use: &BTreeSet<String>) {
        let dir = self.refs_dir();
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return;
        };
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
                continue;
            }
            let name = entry.file_name();
            let Some((sha256, ext)) = name.to_str().and_then(|name| name.split_once('.')) else {
                continue;
            };
            let a_picture = PICTURE_TYPES.iter().any(|(_, known)| *known == ext);
            if a_picture && is_sha256(sha256) && !in_use.contains(sha256) {
                remove_quietly(&dir.join(&name));
            }
        }
    }

    /// The bytes the added pictures take, for Settings → Studio's usage line.
    pub fn refs_bytes(&self) -> u64 {
        let Ok(entries) = std::fs::read_dir(self.refs_dir()) else {
            return 0;
        };
        entries
            .flatten()
            .filter_map(|entry| entry.metadata().ok())
            .filter(std::fs::Metadata::is_file)
            .map(|meta| meta.len())
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;

    use base64::Engine;
    use jiff::tz::TimeZone;
    use serde_json::json;

    use super::*;
    use crate::attachments::{ImageInput, PictureWorker};
    use crate::studio::test_support::{Scratch, mp4, png};

    /// Noon on the given day, in this computer's time zone, as Unix ms.
    fn ms(y: i16, m: i8, d: i8) -> i64 {
        jiff::civil::date(y, m, d)
            .at(12, 0, 0, 0)
            .to_zoned(TimeZone::system())
            .unwrap()
            .timestamp()
            .as_millisecond()
    }

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
    const WEBP: &[u8] = b"RIFF\x24\0\0\0WEBPVP8 ";
    const HTML: &[u8] = b"<!doctype html><p>";

    /// The start of an ISO media file whose `ftyp` box has the major brand `brand`.
    fn ftyp(brand: &[u8; 4]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&20u32.to_be_bytes());
        out.extend_from_slice(b"ftyp");
        out.extend_from_slice(brand);
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(brand);
        out.extend_from_slice(&8u32.to_be_bytes());
        out.extend_from_slice(b"mdat");
        out
    }

    fn sha_of(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    fn files_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn output(run_id: &str, idx: u32, path: &Path, thumb: Option<&Path>) -> store::CreationOutput {
        store::CreationOutput {
            id: format!("{run_id}-out-{idx}"),
            run_id: run_id.into(),
            idx,
            path: path.display().to_string(),
            thumb_path: thumb.map(|t| t.display().to_string()),
            mime: "image/png".into(),
            size_bytes: 10,
            width: Some(4),
            height: Some(4),
            duration_ms: None,
            seed: Some(7),
            starred: false,
            created_at: ms(2026, 11, 17),
        }
    }

    fn run(id: &str, outputs: Vec<store::CreationOutput>) -> store::CreationRun {
        store::CreationRun {
            id: id.into(),
            kind: store::CreationKind::Image,
            runs_on: store::RunsOn::Cloud,
            provider_id: Some(store::ProviderId(3)),
            provider_name: "OpenRouter".into(),
            provider_host: "openrouter.ai".into(),
            model: "example/picture-model".into(),
            model_label: "Picture Model".into(),
            prompt: "a lighthouse at dusk".into(),
            params: json!({"params": {"size": "1024x1024", "seed": 7}, "extra": {}}),
            refs: json!([]),
            status: store::CreationStatus::Done,
            remote_id: Some("gen-1".into()),
            progress: None,
            estimate_usd: Some(0.04),
            cost_usd: Some(0.04),
            error: None,
            audit_id: Some(store::AuditId(11)),
            upstream: None,
            gen_model_id: None,
            engine_build: None,
            device: None,
            elapsed_ms: None,
            created_at: ms(2026, 11, 17),
            started_at: Some(ms(2026, 11, 17)),
            finished_at: Some(ms(2026, 11, 17)),
            deleted_at: None,
            outputs,
        }
    }

    fn picture_input(bytes: &[u8]) -> ImageInput {
        ImageInput {
            mime: "image/png".into(),
            file_name: "lighthouse.png".into(),
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        }
    }

    #[test]
    fn files_go_under_their_month() {
        let g = Gallery::new("/g".into());
        assert_eq!(
            g.output_path(ms(2026, 11, 17), "r1", 0, "png"),
            Path::new("/g/2026-11/r1-0.png")
        );
        assert_eq!(
            g.output_stem(ms(2026, 11, 17), "r1", 2),
            Path::new("/g/2026-11/r1-2")
        );
        assert_eq!(
            g.sidecar_path(ms(2026, 11, 17), "r1"),
            Path::new("/g/2026-11/r1.json")
        );
        assert_eq!(g.thumb_path("o1"), Path::new("/g/.thumbs/o1.jpg"));
    }

    #[test]
    fn the_month_is_this_computers_month() {
        // 23:30 on 30 November in UTC-5 is already December in UTC.
        let tz = TimeZone::fixed(jiff::tz::Offset::from_hours(-5).unwrap());
        let late = jiff::civil::date(2026, 11, 30)
            .at(23, 30, 0, 0)
            .to_zoned(tz.clone())
            .unwrap()
            .timestamp()
            .as_millisecond();
        assert_eq!(month_in(&tz, late), "2026-11");
        assert_eq!(month_in(&TimeZone::UTC, late), "2026-12");
    }

    #[test]
    fn the_default_folder_is_next_to_the_models() {
        assert_eq!(
            Gallery::default_root(Path::new("/home/u"), "harnessUI"),
            Path::new("/home/u/harnessUI/studio")
        );
    }

    #[test]
    fn a_thumbnail_is_written_under_thumbs() {
        let scratch = Scratch::new();
        let g = Gallery::new(scratch.path().join("gallery"));
        assert_eq!(g.root(), scratch.path().join("gallery"));
        let path = g.write_thumb("o1", b"\xFF\xD8\xFFthumb").unwrap();
        assert_eq!(path, g.thumb_path("o1"));
        assert_eq!(std::fs::read(&path).unwrap(), b"\xFF\xD8\xFFthumb");
        assert_eq!(
            files_in(&scratch.path().join("gallery").join(".thumbs")),
            ["o1.jpg"]
        );
    }

    #[test]
    fn the_partial_folder_is_made_when_asked_for() {
        let scratch = Scratch::new();
        let g = Gallery::new(scratch.path().join("gallery"));
        let dir = g.partial_dir();
        assert_eq!(dir, scratch.path().join("gallery").join(".partial"));
        assert!(dir.is_dir());
    }

    #[test]
    fn the_type_is_read_from_the_bytes() {
        assert_eq!(sniff_media(PNG), Some("image/png"));
        assert_eq!(sniff_media(&png(2, 2)), Some("image/png"));
        assert_eq!(sniff_media(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(sniff_media(WEBP), Some("image/webp"));
        assert_eq!(sniff_media(b"GIF89a\x01\0"), Some("image/gif"));
        assert_eq!(sniff_media(b"GIF87a\x01\0"), Some("image/gif"));
        assert_eq!(sniff_media(&mp4()), Some("video/mp4"));
        assert_eq!(
            sniff_media(&[0x1A, 0x45, 0xDF, 0xA3, 0x9F, 0x42]),
            Some("video/webm")
        );
        assert_eq!(sniff_media(HTML), None);
        assert_eq!(sniff_media(b"<html><svg></svg></html>"), None);
        assert_eq!(sniff_media(b""), None);
        assert_eq!(sniff_media(b"hello"), None);
    }

    #[test]
    fn an_iso_media_file_is_mp4_only_for_a_video_brand() {
        for brand in [
            b"isom", b"mp42", b"mp41", b"iso2", b"avc1", b"dash", b"M4V ",
        ] {
            assert_eq!(
                sniff_media(&ftyp(brand)),
                Some("video/mp4"),
                "{}",
                String::from_utf8_lossy(brand)
            );
        }
        // HEIF and AVIF still pictures and image sequences, and QuickTime movies, are none
        // of the seven types.
        for brand in [
            b"avif", b"avis", b"heic", b"heix", b"heim", b"heis", b"hevc", b"hevx", b"mif1",
            b"msf1", b"qt  ",
        ] {
            assert_eq!(
                sniff_media(&ftyp(brand)),
                None,
                "{}",
                String::from_utf8_lossy(brand)
            );
        }
    }

    #[test]
    fn an_svg_is_known_by_its_first_element() {
        for svg in [
            &b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"[..],
            b"\xEF\xBB\xBF  \n<svg>",
            b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!-- made by hand -->\n<svg\n width=\"4\">",
            b"<!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \"x.dtd\" [ <!ENTITY a \"b\"> ]>\n<svg>",
        ] {
            assert_eq!(
                sniff_media(svg),
                Some("image/svg+xml"),
                "{}",
                String::from_utf8_lossy(svg)
            );
        }
        for not_svg in [
            &b"<svgx>"[..],
            b"<SVG>",
            b"<!DOCTYPE html><svg>",
            b"text <svg>",
            b"<?xml version=\"1.0\"?><html/>",
            // Cut off before its first element: an SVG cannot be told from this alone.
            b"<?xml version=\"1.0\"?><!-- a long comment",
        ] {
            assert_eq!(
                sniff_media(not_svg),
                None,
                "{}",
                String::from_utf8_lossy(not_svg)
            );
        }
    }

    #[test]
    fn a_long_run_of_document_types_is_read_quickly() {
        // About 1 MiB of `<!DOCTYPE svg>` over and over, as a picture labelled PNG would
        // bring it: each document type costs only its own length, and only the first
        // `SNIFF_BYTES` are read at all.
        let doctypes = b"<!DOCTYPE svg>".repeat(75_000);
        let quick = std::time::Duration::from_secs(2);

        let started = std::time::Instant::now();
        assert_eq!(svg_start(&doctypes), SvgStart::CannotTell);
        assert!(
            started.elapsed() < quick,
            "svg_start: {:?}",
            started.elapsed()
        );

        let started = std::time::Instant::now();
        assert_eq!(sniff_media(&doctypes), None);
        assert!(
            started.elapsed() < quick,
            "sniff_media: {:?}",
            started.elapsed()
        );

        let started = std::time::Instant::now();
        assert!(!crate::attachments::sniff("image/png", &doctypes));
        assert!(started.elapsed() < quick, "sniff: {:?}", started.elapsed());

        // An SVG whose first element comes after the first `SNIFF_BYTES` cannot be told
        // from its start (`finalize` then goes by the company's word).
        let mut late = b"<!--".to_vec();
        late.extend(std::iter::repeat_n(b'x', SNIFF_BYTES));
        late.extend_from_slice(b"--><svg/>");
        assert_eq!(svg_start(&late), SvgStart::Svg);
        assert_eq!(sniff_media(&late), None);
    }

    #[test]
    fn a_checked_file_is_moved_under_the_type_it_is() {
        let scratch = Scratch::new();
        let g = Gallery::new(scratch.path().join("gallery"));
        let partial = g.partial_dir().join("o1");
        std::fs::write(&partial, png(3, 3)).unwrap();
        let target = g.output_stem(ms(2026, 11, 17), "r1", 0);
        let (path, mime) = g.finalize(&partial, None, &target, 1 << 20).unwrap();
        assert_eq!(path, g.output_path(ms(2026, 11, 17), "r1", 0, "png"));
        assert_eq!(mime, "image/png");
        assert_eq!(std::fs::read(&path).unwrap(), png(3, 3));
        assert!(!partial.exists());

        // The company's own word for it agrees: kept, whatever its spelling.
        std::fs::write(&partial, mp4()).unwrap();
        let target = g.output_stem(ms(2026, 11, 17), "r2", 0);
        let (path, mime) = g
            .finalize(&partial, Some(" Video/MP4; codecs=avc1"), &target, 1 << 20)
            .unwrap();
        assert_eq!(path, g.output_path(ms(2026, 11, 17), "r2", 0, "mp4"));
        assert_eq!(mime, "video/mp4");
    }

    #[test]
    fn a_claimed_type_that_is_not_media_never_wins_over_the_bytes() {
        let scratch = Scratch::new();
        let g = Gallery::new(scratch.path().into());
        let partial = scratch.write("o1", &png(2, 2));
        let target = g.output_stem(ms(2026, 11, 17), "r1", 0);
        let (path, mime) = g
            .finalize(&partial, Some("text/html"), &target, 1 << 20)
            .unwrap();
        assert_eq!(mime, "image/png");
        assert_eq!(path.extension().unwrap(), "png");
    }

    #[test]
    fn a_file_that_is_not_what_it_claims_is_refused_and_removed() {
        let scratch = Scratch::new();
        let g = Gallery::new(scratch.path().into());
        let target = g.output_stem(ms(2026, 11, 17), "r1", 0);

        let partial = scratch.write("o1", HTML);
        let refused = g.finalize(&partial, Some("video/mp4"), &target, 1 << 20);
        assert!(
            matches!(refused, Err(GalleryError::NotMedia)),
            "{refused:?}"
        );
        assert!(!partial.exists());

        let partial = scratch.write("o2", &png(2, 2));
        let refused = g.finalize(&partial, Some("video/mp4"), &target, 1 << 20);
        assert_eq!(
            refused,
            Err(GalleryError::Mismatch {
                claimed: "video/mp4".into(),
                found: "image/png".into()
            })
        );
        assert!(!partial.exists());

        // A WebP picture called a PNG is a mismatch too.
        let partial = scratch.write("o3", WEBP);
        let refused = g.finalize(&partial, Some("image/png"), &target, 1 << 20);
        assert!(
            matches!(refused, Err(GalleryError::Mismatch { .. })),
            "{refused:?}"
        );
        assert!(!partial.exists());

        // Nothing reached the month folder.
        assert!(!target.parent().unwrap().exists());
    }

    #[test]
    fn a_paid_result_that_is_not_what_it_claims_is_kept_as_its_bytes_are() {
        let scratch = Scratch::new();
        let g = Gallery::new(scratch.path().into());
        let target = g.output_stem(ms(2026, 11, 17), "r1", 0);
        let partial = scratch.write("o1", &png(2, 2));
        let (path, mime) = g
            .finalize_paid(&partial, Some("video/mp4"), &target, 1 << 20)
            .unwrap();
        assert_eq!(mime, "image/png");
        assert_eq!(path.extension().unwrap(), "png");
        assert!(!partial.exists());
        // What is no picture or video at all is still refused.
        let partial = scratch.write("o2", HTML);
        let refused = g.finalize_paid(&partial, Some("image/png"), &target, 1 << 20);
        assert!(
            matches!(refused, Err(GalleryError::NotMedia)),
            "{refused:?}"
        );
        assert!(!partial.exists());
    }

    #[test]
    fn an_avif_picture_or_a_quicktime_movie_is_not_kept_as_mp4() {
        let scratch = Scratch::new();
        let g = Gallery::new(scratch.path().into());
        let target = g.output_stem(ms(2026, 11, 17), "r1", 0);

        for (name, brand, claimed) in [
            ("o1", b"avif", None),
            ("o2", b"avif", Some("image/avif")),
            ("o3", b"avif", Some("video/mp4")),
            ("o4", b"qt  ", None),
            ("o5", b"qt  ", Some("video/quicktime")),
            ("o6", b"qt  ", Some("video/mp4")),
        ] {
            let partial = scratch.write(name, &ftyp(brand));
            let refused = g.finalize(&partial, claimed, &target, 1 << 20);
            assert_eq!(
                refused,
                Err(GalleryError::NotMedia),
                "{} claimed {claimed:?}",
                String::from_utf8_lossy(brand)
            );
            assert!(!partial.exists());
        }
        assert!(!target.parent().unwrap().exists());

        // An `isom` or `mp42` file is still MP4 video.
        for (idx, brand) in [b"isom", b"mp42"].into_iter().enumerate() {
            let partial = scratch.write("o7", &ftyp(brand));
            let target = g.output_stem(ms(2026, 11, 17), "r2", idx as u32);
            let (path, mime) = g
                .finalize(&partial, Some("video/mp4"), &target, 1 << 20)
                .unwrap();
            assert_eq!(mime, "video/mp4");
            assert_eq!(
                path,
                g.output_path(ms(2026, 11, 17), "r2", idx as u32, "mp4")
            );
            assert!(!partial.exists());
        }
    }

    #[test]
    fn a_file_over_its_cap_is_refused_and_removed() {
        let scratch = Scratch::new();
        let g = Gallery::new(scratch.path().into());
        let bytes = png(8, 8);
        let partial = scratch.write("o1", &bytes);
        let target = g.output_stem(ms(2026, 11, 17), "r1", 0);
        let cap = bytes.len() as u64 - 1;
        assert_eq!(
            g.finalize(&partial, None, &target, cap),
            Err(GalleryError::TooLarge(cap))
        );
        assert!(!partial.exists());

        // Exactly at the cap is kept.
        let partial = scratch.write("o2", &bytes);
        assert!(g.finalize(&partial, None, &target, cap + 1).is_ok());
    }

    #[test]
    fn an_svg_whose_start_cannot_tell_is_taken_on_the_companys_word() {
        let scratch = Scratch::new();
        let g = Gallery::new(scratch.path().into());
        let target = g.output_stem(ms(2026, 11, 17), "r1", 0);
        let mut long = b"<?xml version=\"1.0\"?>\n<!--".to_vec();
        long.extend(std::iter::repeat_n(b'x', 20_000));
        long.extend_from_slice(b"-->\n<svg xmlns=\"http://www.w3.org/2000/svg\"/>");

        // Without the company's word, the start alone is not enough.
        let partial = scratch.write("o1", &long);
        assert_eq!(
            g.finalize(&partial, None, &target, 1 << 20),
            Err(GalleryError::NotMedia)
        );

        let partial = scratch.write("o2", &long);
        let (path, mime) = g
            .finalize(&partial, Some("image/svg+xml"), &target, 1 << 20)
            .unwrap();
        assert_eq!(mime, "image/svg+xml");
        assert_eq!(path.extension().unwrap(), "svg");

        // A short file has no more to show: an unfinished prolog is not an SVG.
        let partial = scratch.write("o3", b"<?xml version=\"1.0\"?><!-- cut");
        assert_eq!(
            g.finalize(&partial, Some("image/svg+xml"), &target, 1 << 20),
            Err(GalleryError::NotMedia)
        );
        // Nor is a page that says what it is.
        let partial = scratch.write("o4", HTML);
        assert_eq!(
            g.finalize(&partial, Some("image/svg+xml"), &target, 1 << 20),
            Err(GalleryError::NotMedia)
        );
        assert!(!partial.exists());
    }

    #[test]
    fn a_full_disk_is_named() {
        assert!(matches!(
            io_error(std::io::Error::from(std::io::ErrorKind::StorageFull)),
            GalleryError::DiskFull
        ));
        assert!(matches!(
            io_error(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
            GalleryError::Io(_)
        ));
    }

    #[test]
    fn the_sidecar_is_the_run_record() {
        let scratch = Scratch::new();
        let g = Gallery::new(scratch.path().into());
        let out_path = g.output_path(ms(2026, 11, 17), "r1", 0, "png");
        let record = run("r1", vec![output("r1", 0, &out_path, None)]);
        g.write_sidecar(&record).unwrap();

        let path = g.sidecar_path(record.created_at, "r1");
        let text = std::fs::read_to_string(&path).unwrap();
        let back: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(back["id"], "r1");
        assert_eq!(back["prompt"], "a lighthouse at dusk");
        assert_eq!(back["params"]["params"]["size"], "1024x1024");
        assert_eq!(back["outputs"][0]["path"], out_path.display().to_string());
        assert!(!text.contains("Authorization"));
        assert!(!text.contains("api_key"));
        assert_eq!(
            serde_json::from_str::<store::CreationRun>(&text).unwrap(),
            record
        );

        // Written again, it is replaced, and no temporary file stays behind.
        let mut later = record.clone();
        later.prompt = "a lighthouse at dawn".into();
        g.write_sidecar(&later).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("dawn"));
        assert_eq!(files_in(path.parent().unwrap()), ["r1.json"]);
    }

    #[test]
    fn a_reference_name_that_is_not_a_hash_is_refused() {
        let scratch = Scratch::new();
        let g = Gallery::new(scratch.path().into());
        assert!(matches!(
            g.ref_file("../../etc/passwd"),
            Err(GalleryError::BadName)
        ));
        assert!(matches!(g.ref_file(""), Err(GalleryError::BadName)));
        assert!(matches!(
            g.ref_file(&"A".repeat(64)),
            Err(GalleryError::BadName)
        ));
        assert!(matches!(
            g.ref_file(&"a".repeat(65)),
            Err(GalleryError::BadName)
        ));
        // A good name for a picture that is not there.
        assert!(matches!(
            g.ref_file(&"a".repeat(64)),
            Err(GalleryError::Io(_))
        ));
    }

    #[tokio::test]
    async fn references_are_kept_once_by_hash() {
        let scratch = Scratch::new();
        let g = Gallery::new(scratch.path().into());
        let worker = PictureWorker::app();
        let bytes = png(5, 4);
        let sha = sha_of(&bytes);

        let first = g.add_ref(&worker, &picture_input(&bytes)).await.unwrap();
        let second = g.add_ref(&worker, &picture_input(&bytes)).await.unwrap();
        assert_eq!(first.sha256, sha);
        assert_eq!(first.mime, "image/png");
        assert_eq!(first.size_bytes, bytes.len() as u64);
        assert_eq!(second.sha256, first.sha256);

        let refs = scratch.path().join("refs");
        assert_eq!(files_in(&refs), [format!("{sha}.png")]);
        assert_eq!(
            std::fs::read(refs.join(format!("{sha}.png"))).unwrap(),
            bytes
        );
        assert_eq!(
            g.ref_file(&sha).unwrap(),
            (refs.join(format!("{sha}.png")), "image/png".to_owned())
        );
        assert_eq!(g.refs_bytes(), bytes.len() as u64);
    }

    #[tokio::test]
    async fn an_added_picture_is_checked_like_a_chat_picture() {
        let scratch = Scratch::new();
        let g = Gallery::new(scratch.path().into());
        let mut input = picture_input(HTML);
        input.file_name = "page.png".into();
        let refused = g.add_ref(&PictureWorker::app(), &input).await.unwrap_err();
        assert_eq!(refused.code, "bad_request");
        assert!(!scratch.path().join("refs").exists());
        assert_eq!(g.refs_bytes(), 0);
    }

    #[test]
    fn a_reference_no_run_uses_any_more_is_removed() {
        let scratch = Scratch::new();
        let g = Gallery::new(scratch.path().into());
        let refs = scratch.path().join("refs");
        std::fs::create_dir_all(&refs).unwrap();
        let (a, b) = ("a".repeat(64), "b".repeat(64));
        std::fs::write(refs.join(format!("{a}.png")), png(2, 2)).unwrap();
        std::fs::write(
            refs.join(format!("{b}.jpg")),
            [0xFF, 0xD8, 0xFF, 0xE0, 1, 2],
        )
        .unwrap();

        // Not added pictures: another name, another type, a folder.
        std::fs::write(refs.join("notes.txt"), b"mine").unwrap();
        std::fs::write(refs.join(format!("{a}.txt")), b"mine").unwrap();
        std::fs::create_dir_all(refs.join(format!("{}.png", "c".repeat(64)))).unwrap();

        let in_use = BTreeSet::from([b.clone()]);
        g.remove_unused_refs(&in_use);

        assert_eq!(
            files_in(&refs),
            [
                format!("{a}.txt"),
                format!("{b}.jpg"),
                format!("{}.png", "c".repeat(64)),
                "notes.txt".to_owned()
            ]
        );
    }

    #[test]
    fn a_delete_removes_outputs_thumbnails_and_sidecars() {
        let scratch = Scratch::new();
        let g = Gallery::new(scratch.path().into());
        let made = ms(2026, 11, 17);
        let kept_path = scratch.write("keep.txt", b"not Studio's");

        // r1 is deleted whole; r2 loses one of its two outputs.
        let p1 = g.output_path(made, "r1", 0, "png");
        let p2a = g.output_path(made, "r2", 0, "png");
        let p2b = g.output_path(made, "r2", 1, "png");
        let thumb = g.thumb_path("r1-out-0");
        for path in [&p1, &p2a, &p2b, &thumb] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, png(2, 2)).unwrap();
        }
        let o1 = output("r1", 0, &p1, Some(&thumb));
        let o2a = output("r2", 0, &p2a, None);
        let r1 = run("r1", vec![o1.clone()]);
        let r2 = run("r2", vec![o2a.clone(), output("r2", 1, &p2b, None)]);
        g.write_sidecar(&r1).unwrap();
        g.write_sidecar(&r2).unwrap();

        g.remove_files(&store::Deleted {
            outputs: vec![o1, o2a],
            runs: vec![r1],
        });

        assert!(!p1.exists() && !thumb.exists() && !p2a.exists());
        assert!(!g.sidecar_path(made, "r1").exists());
        assert!(p2b.exists() && g.sidecar_path(made, "r2").exists());
        assert!(kept_path.exists());
    }
}
