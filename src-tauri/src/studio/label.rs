//! The AI label every local Studio result carries in its own file (0.1.0), and never the
//! prompt.
//!
//! The label ([`AiLabel`]) says the file was made by a trained model (IPTC's
//! [`IPTC_TRAINED`]), with which app, model and revision, under a unique id and with the date
//! it was made. [`label_file`] writes it into the file the engine made, after the gallery has
//! checked its type:
//!
//! - **PNG:** every `tEXt`, `zTXt` and `iTXt` chunk goes (the engine's text chunks are where a
//!   prompt would be), then one `iTXt` chunk "XML:com.adobe.xmp" holding [`xmp`] goes in
//!   before the first `IDAT`.
//! - **WebP:** every `XMP ` chunk goes and one holding [`xmp`] is appended; a simple file
//!   (`VP8 ` or `VP8L`) gets a `VP8X` header with the XMP flag (and the alpha flag when its
//!   lossless data says it has see-through pixels), an extended one has the flag set.
//! - **WebM:** each Tags element of the Segment is overwritten by a Void of the same size,
//!   zeroed, so nothing after it moves and the SeekHead and Cues still point at their
//!   elements; one Tags element (`TITLE` the id, `COMMENT` "Made with harnessUI <version>,
//!   <model> <revision>", `DATE_RECORDED` and `DIGITAL_SOURCE_TYPE`, for the whole file) is
//!   appended at the Segment's end and the Segment's size rewritten in its existing width.
//!   The SeekHead gets an entry for the new Tags when it and the Void after it have room, so
//!   a player that stops reading at the first Cluster still finds the label.
//!
//! No new dependency: PNG's CRC-32 table, RIFF sizes and a minimal EBML reader and writer are
//! here. [`read_label`] gives the label back as text.

use std::path::Path;

use super::gallery::{GalleryError, io_error, write_whole};

/// IPTC's digital source type for media made by a trained model.
pub const IPTC_TRAINED: &str =
    "http://cv.iptc.org/newscodes/digitalsourcetype/trainedAlgorithmicMedia";

/// What a local result's label says. It never holds the prompt, nor any setting of the run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiLabel {
    /// harnessUI's version, such as `0.1.0`.
    pub app_version: String,
    /// The model's name as the catalog gives it.
    pub model: String,
    /// The catalog revision of the model.
    pub revision: String,
    /// The result's unique id.
    pub id: String,
    /// When the result was made, in RFC 3339.
    pub created: String,
}

impl AiLabel {
    /// "Made with harnessUI <version>, <model> <revision>".
    fn description(&self) -> String {
        format!(
            "Made with harnessUI {}, {} {}",
            self.app_version, self.model, self.revision
        )
    }
}

/// The label as an XMP packet: `Iptc4xmpExt:DigitalSourceType`, `dc:description` ("Made
/// with harnessUI <version>, <model> <revision>"), `xmpMM:DocumentID` (the id) and
/// `xmp:CreateDate`.
pub fn xmp(label: &AiLabel) -> String {
    format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
         <x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n \
         <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
         <rdf:Description rdf:about=\"\"\n    \
         xmlns:Iptc4xmpExt=\"http://iptc.org/std/Iptc4xmpExt/2008-02-29/\"\n    \
         xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n    \
         xmlns:xmpMM=\"http://ns.adobe.com/xap/1.0/mm/\"\n    \
         xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\">\n   \
         <Iptc4xmpExt:DigitalSourceType>{IPTC_TRAINED}</Iptc4xmpExt:DigitalSourceType>\n   \
         <dc:description>\n    \
         <rdf:Alt>\n     \
         <rdf:li xml:lang=\"x-default\">{}</rdf:li>\n    \
         </rdf:Alt>\n   \
         </dc:description>\n   \
         <xmpMM:DocumentID>{}</xmpMM:DocumentID>\n   \
         <xmp:CreateDate>{}</xmp:CreateDate>\n  \
         </rdf:Description>\n \
         </rdf:RDF>\n\
         </x:xmpmeta>\n\
         <?xpacket end=\"r\"?>",
        escape(&label.description()),
        escape(&label.id),
        escape(&label.created),
    )
}

/// `text` with XML's five special characters escaped.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// Writes `label` into the file at `path`, a PNG, WebP or WebM file as `mime` says (see the
/// module's notes), replacing the file whole (through a temporary file beside it). A file of
/// another type, or one whose structure does not hold together, is
/// [`GalleryError::NotMedia`] and is left as it was.
pub fn label_file(path: &Path, mime: &str, label: &AiLabel) -> Result<(), GalleryError> {
    let bytes = std::fs::read(path).map_err(io_error)?;
    let labelled = match mime {
        "image/png" => png::label(&bytes, &xmp(label)),
        "image/webp" => webp::label(&bytes, &xmp(label)),
        "video/webm" => webm::label(&bytes, label),
        _ => None,
    }
    .ok_or(GalleryError::NotMedia)?;
    write_whole(path, &labelled)
}

/// The label in the file at `path` as text: the XMP of a PNG or WebP file, and the tags of a
/// WebM file, one `NAME=value` line each. `None` when there is none, or the file cannot be
/// read as `mime`.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "only tests read a label back so far")
)]
pub fn read_label(path: &Path, mime: &str) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    match mime {
        "image/png" => png::read(&bytes),
        "image/webp" => webp::read(&bytes),
        "video/webm" => webm::read(&bytes),
        _ => None,
    }
}

/// The CRC-32 table PNG uses (polynomial `0xEDB88320`, reflected).
const CRC_TABLE: [u32; 256] = crc_table();

const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut n = 0;
    while n < 256 {
        let mut c = n as u32;
        let mut bit = 0;
        while bit < 8 {
            c = if c & 1 == 1 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            bit += 1;
        }
        table[n] = c;
        n += 1;
    }
    table
}

/// The CRC-32 of `parts` one after the other.
fn crc32(parts: &[&[u8]]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for part in parts {
        for b in *part {
            c = CRC_TABLE[((c ^ u32::from(*b)) & 0xFF) as usize] ^ (c >> 8);
        }
    }
    c ^ 0xFFFF_FFFF
}

/// PNG: chunks of a length (four bytes, big-endian), a type, the data and a CRC-32 of the type
/// and data.
mod png {
    use super::crc32;

    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    /// The `iTXt` keyword XMP is kept under.
    const XMP_KEYWORD: &[u8] = b"XML:com.adobe.xmp";
    /// The chunks that hold text, where a prompt would be.
    const TEXT_CHUNKS: [[u8; 4]; 3] = [*b"tEXt", *b"zTXt", *b"iTXt"];

    /// One chunk: its type, its data and the whole chunk as it is in the file.
    struct Chunk<'a> {
        kind: [u8; 4],
        data: &'a [u8],
        whole: &'a [u8],
    }

    /// The chunks of a PNG file from its IHDR to its IEND (what follows IEND is not part of
    /// the picture), or `None` when it is not one.
    fn chunks(bytes: &[u8]) -> Option<Vec<Chunk<'_>>> {
        let mut at = SIGNATURE.len();
        if !bytes.starts_with(SIGNATURE) {
            return None;
        }
        let mut chunks = Vec::new();
        loop {
            let len = u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?) as usize;
            let end = at.checked_add(12)?.checked_add(len)?;
            let whole = bytes.get(at..end)?;
            let kind: [u8; 4] = whole[4..8].try_into().ok()?;
            chunks.push(Chunk {
                kind,
                data: &whole[8..8 + len],
                whole,
            });
            at = end;
            if kind == *b"IEND" {
                break;
            }
        }
        (chunks[0].kind == *b"IHDR").then_some(chunks)
    }

    /// The chunk `kind` holding `data`.
    fn chunk(kind: &[u8; 4], data: &[u8]) -> Option<Vec<u8>> {
        let mut out = u32::try_from(data.len()).ok()?.to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        out.extend_from_slice(&crc32(&[kind, data]).to_be_bytes());
        Some(out)
    }

    /// `bytes` without its text chunks, with one uncompressed `iTXt` chunk holding `xmp`
    /// before the first `IDAT`.
    pub(super) fn label(bytes: &[u8], xmp: &str) -> Option<Vec<u8>> {
        let chunks = chunks(bytes)?;
        let mut data = XMP_KEYWORD.to_vec();
        // The keyword's end, no compression, compression method 0, and no language tag or
        // translated keyword.
        data.extend_from_slice(&[0, 0, 0, 0, 0]);
        data.extend_from_slice(xmp.as_bytes());
        let mut label = Some(chunk(b"iTXt", &data)?);
        let mut out = SIGNATURE.to_vec();
        for c in &chunks {
            if TEXT_CHUNKS.contains(&c.kind) {
                continue;
            }
            if c.kind == *b"IDAT"
                && let Some(label) = label.take()
            {
                out.extend_from_slice(&label);
            }
            out.extend_from_slice(c.whole);
        }
        // A PNG without picture data is not one.
        label.is_none().then_some(out)
    }

    /// The text of the first uncompressed XMP `iTXt` chunk.
    pub(super) fn read(bytes: &[u8]) -> Option<String> {
        chunks(bytes)?.iter().find_map(|c| {
            let rest = c.data.strip_prefix(XMP_KEYWORD)?.strip_prefix(&[0, 0])?;
            // Skip the compression method, then the language tag and the translated keyword.
            let mut fields = rest.get(1..)?.splitn(3, |b| *b == 0);
            let (_language, _translated) = (fields.next()?, fields.next()?);
            String::from_utf8(fields.next()?.to_vec()).ok()
        })
    }
}

/// WebP: a RIFF file of chunks, each a type, a size (four bytes, little-endian), the data and
/// a padding byte when the size is odd.
mod webp {
    /// The `VP8X` flags for an XMP chunk and for see-through pixels.
    const XMP_FLAG: u8 = 0x04;
    const ALPHA_FLAG: u8 = 0x10;

    /// The chunks of a WebP file, or `None` when it is not one. What follows the RIFF size is
    /// not part of the picture.
    fn chunks(bytes: &[u8]) -> Option<Vec<([u8; 4], &[u8])>> {
        if bytes.get(..4)? != b"RIFF" || bytes.get(8..12)? != b"WEBP" {
            return None;
        }
        let riff_size = u32::from_le_bytes(bytes[4..8].try_into().ok()?) as usize;
        let end = riff_size.checked_add(8)?;
        let body = bytes.get(..end)?;
        let mut at = 12;
        let mut chunks = Vec::new();
        while at < end {
            let kind: [u8; 4] = body.get(at..at + 4)?.try_into().ok()?;
            let len = u32::from_le_bytes(body.get(at + 4..at + 8)?.try_into().ok()?) as usize;
            let data = body.get(at + 8..(at + 8).checked_add(len)?)?;
            chunks.push((kind, data));
            // The last chunk's padding byte may be missing.
            at += 8 + len + len % 2;
        }
        (!chunks.is_empty()).then_some(chunks)
    }

    /// A `VP8X` header for a canvas of `width` x `height` with `flags`.
    fn vp8x(flags: u8, width: u32, height: u32) -> Vec<u8> {
        let mut out = vec![flags, 0, 0, 0];
        out.extend_from_slice(&(width - 1).to_le_bytes()[..3]);
        out.extend_from_slice(&(height - 1).to_le_bytes()[..3]);
        out
    }

    /// The width and height in a lossy (`VP8 `) key frame's header.
    fn vp8_size(data: &[u8]) -> Option<(u32, u32)> {
        if data.get(3..6)? != [0x9d, 0x01, 0x2a] {
            return None;
        }
        let width = u32::from(u16::from_le_bytes(data[6..8].try_into().ok()?) & 0x3FFF);
        let height = u32::from(u16::from_le_bytes(data.get(8..10)?.try_into().ok()?) & 0x3FFF);
        (width > 0 && height > 0).then_some((width, height))
    }

    /// The width, height and alpha hint in a lossless (`VP8L`) header.
    fn vp8l_size(data: &[u8]) -> Option<(u32, u32, bool)> {
        if *data.first()? != 0x2f {
            return None;
        }
        let bits = u32::from_le_bytes(data.get(1..5)?.try_into().ok()?);
        Some((
            (bits & 0x3FFF) + 1,
            ((bits >> 14) & 0x3FFF) + 1,
            (bits >> 28) & 1 == 1,
        ))
    }

    /// Appends the chunk `kind` holding `data`, padded to an even length.
    fn put(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) -> Option<()> {
        out.extend_from_slice(kind);
        out.extend_from_slice(&u32::try_from(data.len()).ok()?.to_le_bytes());
        out.extend_from_slice(data);
        if data.len() % 2 == 1 {
            out.push(0);
        }
        Some(())
    }

    /// `bytes` as an extended WebP file whose only XMP chunk, the last one, holds `xmp`.
    pub(super) fn label(bytes: &[u8], xmp: &str) -> Option<Vec<u8>> {
        let chunks = chunks(bytes)?;
        let (first, data) = chunks[0];
        let (header, rest) = match &first {
            b"VP8X" => {
                let mut header = data.get(..10)?.to_vec();
                header[0] |= XMP_FLAG;
                header.extend_from_slice(&data[10..]);
                (header, &chunks[1..])
            }
            b"VP8 " => {
                let (width, height) = vp8_size(data)?;
                (vp8x(XMP_FLAG, width, height), &chunks[..])
            }
            b"VP8L" => {
                let (width, height, alpha) = vp8l_size(data)?;
                let flags = XMP_FLAG | if alpha { ALPHA_FLAG } else { 0 };
                (vp8x(flags, width, height), &chunks[..])
            }
            _ => return None,
        };
        let mut body = b"WEBP".to_vec();
        put(&mut body, b"VP8X", &header)?;
        for (kind, data) in rest.iter().filter(|(kind, _)| kind != b"XMP ") {
            put(&mut body, kind, data)?;
        }
        put(&mut body, b"XMP ", xmp.as_bytes())?;
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&u32::try_from(body.len()).ok()?.to_le_bytes());
        out.extend_from_slice(&body);
        Some(out)
    }

    /// The text of the first XMP chunk.
    pub(super) fn read(bytes: &[u8]) -> Option<String> {
        let (_, data) = chunks(bytes)?
            .into_iter()
            .find(|(kind, _)| kind == b"XMP ")?;
        String::from_utf8(data.to_vec()).ok()
    }
}

/// WebM: EBML elements, each an id and a size (variable-length integers, the size's value bits
/// all ones meaning "unknown") and the data.
///
/// The label's Tags element holds one Tag for the whole file (an empty Targets) with four
/// SimpleTags: `TITLE` the id, `COMMENT` "Made with harnessUI <version>, <model>
/// <revision>", `DATE_RECORDED` the date it was made and `DIGITAL_SOURCE_TYPE`
/// [`IPTC_TRAINED`](super::IPTC_TRAINED).
mod webm {
    use super::{AiLabel, IPTC_TRAINED};

    const EBML: u32 = 0x1A45_DFA3;
    /// The Segment, which holds everything after the EBML header.
    pub(super) const SEGMENT: u32 = 0x1853_8067;
    /// The SeekHead and its entries: each a SeekID (the id it points at) and a SeekPosition
    /// (where that element starts, counted from the start of the Segment's data).
    pub(super) const SEEK_HEAD: u32 = 0x114D_9B74;
    pub(super) const SEEK: u32 = 0x4DBB;
    pub(super) const SEEK_ID: u32 = 0x53AB;
    pub(super) const SEEK_POSITION: u32 = 0x53AC;
    /// The Tags and what they hold.
    pub(super) const TAGS: u32 = 0x1254_C367;
    pub(super) const TAG: u32 = 0x7373;
    pub(super) const TARGETS: u32 = 0x63C0;
    pub(super) const SIMPLE_TAG: u32 = 0x67C8;
    pub(super) const TAG_NAME: u32 = 0x45A3;
    pub(super) const TAG_STRING: u32 = 0x4487;
    /// Filler that readers skip.
    const VOID: u32 = 0xEC;

    /// Where an element is in the file: it starts at `start`, its size field at `size_at`, its
    /// data at `data` and it ends at `end`. `known` is false for a size of "unknown" (it then
    /// runs to the end of what holds it).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) struct Element {
        pub(super) id: u32,
        pub(super) start: usize,
        pub(super) size_at: usize,
        pub(super) data: usize,
        pub(super) end: usize,
        pub(super) known: bool,
    }

    /// The variable-length integer at `at`: its value without the length marker, its length,
    /// and whether its value bits are all ones. `max_len` is 4 for an id, 8 for a size.
    fn vint(bytes: &[u8], at: usize, max_len: usize) -> Option<(u64, usize, bool)> {
        let first = *bytes.get(at)?;
        let len = first.leading_zeros() as usize + 1;
        if len > max_len {
            return None;
        }
        let mut value = u64::from(first) & (0xFF >> len);
        for b in bytes.get(at + 1..at + len)? {
            value = value << 8 | u64::from(*b);
        }
        let all_ones = value == (1u64 << (7 * len)) - 1;
        Some((value, len, all_ones))
    }

    /// The element starting at `at`, which must end by `limit`.
    fn element(bytes: &[u8], at: usize, limit: usize) -> Option<Element> {
        let (_, id_len, _) = vint(bytes, at, 4)?;
        let id = bytes[at..at + id_len]
            .iter()
            .fold(0u32, |id, b| id << 8 | u32::from(*b));
        let size_at = at + id_len;
        let (size, size_len, unknown) = vint(bytes, size_at, 8)?;
        let data = size_at + size_len;
        let end = if unknown {
            limit
        } else {
            data.checked_add(usize::try_from(size).ok()?)?
        };
        (end <= limit).then_some(Element {
            id,
            start: at,
            size_at,
            data,
            end,
            known: !unknown,
        })
    }

    /// The elements from `from` to `to`, one after another with no gap. An element of unknown
    /// size cannot be stepped over, so it is refused.
    pub(super) fn children(bytes: &[u8], from: usize, to: usize) -> Option<Vec<Element>> {
        let mut at = from;
        let mut kids = Vec::new();
        while at < to {
            let e = element(bytes, at, to)?;
            if !e.known {
                return None;
            }
            kids.push(e);
            at = e.end;
        }
        Some(kids)
    }

    /// The first Segment of a file that starts with an EBML header.
    pub(super) fn segment(bytes: &[u8]) -> Option<Element> {
        let header = element(bytes, 0, bytes.len())?;
        if header.id != EBML || !header.known {
            return None;
        }
        let mut at = header.end;
        loop {
            let e = element(bytes, at, bytes.len())?;
            if e.id == SEGMENT {
                return Some(e);
            }
            if !e.known {
                return None;
            }
            at = e.end;
        }
    }

    /// An id's bytes, its length marker included.
    fn id_bytes(id: u32) -> Vec<u8> {
        id.to_be_bytes()
            .into_iter()
            .skip_while(|b| *b == 0)
            .collect()
    }

    /// `n` as a size `width` bytes wide, or `None` when it does not fit (all ones is kept for
    /// "unknown").
    fn size_bytes(n: u64, width: usize) -> Option<Vec<u8>> {
        if !(1..=8).contains(&width) || n >= (1u64 << (7 * width)) - 1 {
            return None;
        }
        Some((n | 1 << (7 * width)).to_be_bytes()[8 - width..].to_vec())
    }

    /// The fewest bytes a size of `n` needs.
    fn size_width(n: u64) -> usize {
        (1..=8)
            .find(|width| n < (1u64 << (7 * width)) - 1)
            .unwrap_or(8)
    }

    /// The element `id` holding `payload`, its size `width` bytes wide.
    fn element_bytes(id: u32, payload: &[u8], width: usize) -> Option<Vec<u8>> {
        let size = size_bytes(payload.len() as u64, width)?;
        Some([id_bytes(id), size, payload.to_vec()].concat())
    }

    /// The element `id` holding `payload`.
    fn el(id: u32, payload: &[u8]) -> Vec<u8> {
        let width = size_width(payload.len() as u64);
        element_bytes(id, payload, width).expect("the fewest bytes always fit")
    }

    /// An unsigned integer element with no leading zero bytes.
    fn uint_el(id: u32, value: u64) -> Vec<u8> {
        let bytes = value.to_be_bytes();
        let skip = (value.leading_zeros() / 8).min(7) as usize;
        el(id, &bytes[skip..])
    }

    /// A Void `total` bytes long (at least two), its data zeroed.
    fn void(total: usize) -> Vec<u8> {
        let (size, payload) = if total - 2 <= 126 {
            (vec![0x80 | (total - 2) as u8], total - 2)
        } else {
            let size = size_bytes((total - 9) as u64, 8).expect("a Void fits in eight bytes");
            (size, total - 9)
        };
        [vec![VOID as u8], size, vec![0; payload]].concat()
    }

    /// The label's Tags element.
    fn tags(label: &AiLabel) -> Vec<u8> {
        let simple = |name: &str, value: &str| {
            el(
                SIMPLE_TAG,
                &[
                    el(TAG_NAME, name.as_bytes()),
                    el(TAG_STRING, value.as_bytes()),
                ]
                .concat(),
            )
        };
        let tag = [
            el(TARGETS, &[]),
            simple("TITLE", &label.id),
            simple("COMMENT", &label.description()),
            simple("DATE_RECORDED", &label.created),
            simple("DIGITAL_SOURCE_TYPE", IPTC_TRAINED),
        ];
        el(TAGS, &el(TAG, &tag.concat()))
    }

    /// The id a SeekHead entry points at.
    fn sought(bytes: &[u8], entry: &Element) -> Option<u32> {
        let id = children(bytes, entry.data, entry.end)?
            .into_iter()
            .find(|e| e.id == SEEK_ID)?;
        let bytes = bytes.get(id.data..id.end)?;
        (bytes.len() <= 4).then(|| bytes.iter().fold(0, |id, b| id << 8 | u32::from(*b)))
    }

    /// Points the SeekHead's Tags entry at `position`, in the room the SeekHead and the Void
    /// right after it take together (the entry of earlier Tags, and any Void inside, is
    /// dropped). A SeekHead holding anything else (a CRC-32), or without the room, is left as
    /// it was.
    fn point_seek_head_at_tags(out: &mut [u8], kids: &[Element], position: u64) {
        let Some(i) = kids.iter().position(|e| e.id == SEEK_HEAD) else {
            return;
        };
        let head = kids[i];
        let room_end = match kids.get(i + 1) {
            Some(next) if next.id == VOID => next.end,
            _ => head.end,
        };
        let Some(entries) = children(out, head.data, head.end) else {
            return;
        };
        let mut payload = Vec::new();
        for entry in &entries {
            match (entry.id, sought(out, entry)) {
                (SEEK, Some(TAGS)) | (VOID, _) => {}
                (SEEK, Some(_)) => payload.extend_from_slice(&out[entry.start..entry.end]),
                _ => return,
            }
        }
        payload.extend(el(
            SEEK,
            &[
                el(SEEK_ID, &id_bytes(TAGS)),
                uint_el(SEEK_POSITION, position),
            ]
            .concat(),
        ));
        let room = room_end - head.start;
        // A wider size may be needed to leave no single byte, too small for a Void.
        for width in size_width(payload.len() as u64)..=8 {
            let Some(new_head) = element_bytes(SEEK_HEAD, &payload, width) else {
                return;
            };
            let Some(left) = room.checked_sub(new_head.len()) else {
                return;
            };
            if left != 1 {
                let filler = if left == 0 { Vec::new() } else { void(left) };
                out[head.start..room_end].copy_from_slice(&[new_head, filler].concat());
                return;
            }
        }
    }

    /// `bytes` with the Segment's Tags overwritten by Voids, the label's Tags appended at the
    /// Segment's end, the Segment's size rewritten in its existing width and the SeekHead
    /// pointing at the new Tags when it has room.
    pub(super) fn label(bytes: &[u8], label: &AiLabel) -> Option<Vec<u8>> {
        let segment = segment(bytes)?;
        let kids = children(bytes, segment.data, segment.end)?;
        let mut out = bytes.to_vec();
        for old in kids.iter().filter(|e| e.id == TAGS) {
            out[old.start..old.end].copy_from_slice(&void(old.end - old.start));
        }
        point_seek_head_at_tags(&mut out, &kids, (segment.end - segment.data) as u64);
        let tags = tags(label);
        if segment.known {
            let size = (segment.end - segment.data + tags.len()) as u64;
            let size = size_bytes(size, segment.data - segment.size_at)?;
            out[segment.size_at..segment.data].copy_from_slice(&size);
        }
        out.splice(segment.end..segment.end, tags);
        Some(out)
    }

    /// Adds a SimpleTag's `NAME=value` line, then those of the SimpleTags inside it.
    fn simple_tag_lines(bytes: &[u8], simple: &Element, lines: &mut Vec<String>) -> Option<()> {
        let parts = children(bytes, simple.data, simple.end)?;
        let text = |id: u32| {
            parts
                .iter()
                .find(|e| e.id == id)
                .map(|e| String::from_utf8_lossy(&bytes[e.data..e.end]).into_owned())
        };
        lines.push(format!(
            "{}={}",
            text(TAG_NAME)?,
            text(TAG_STRING).unwrap_or_default()
        ));
        for inner in parts.iter().filter(|e| e.id == SIMPLE_TAG) {
            simple_tag_lines(bytes, inner, lines)?;
        }
        Some(())
    }

    /// Every SimpleTag of the Segment's Tags, one `NAME=value` line each.
    pub(super) fn read(bytes: &[u8]) -> Option<String> {
        let segment = segment(bytes)?;
        let mut lines = Vec::new();
        let kids = children(bytes, segment.data, segment.end)?;
        for tags in kids.iter().filter(|e| e.id == TAGS) {
            for tag in children(bytes, tags.data, tags.end)?
                .iter()
                .filter(|e| e.id == TAG)
            {
                let simples = children(bytes, tag.data, tag.end)?;
                for simple in simples.iter().filter(|e| e.id == SIMPLE_TAG) {
                    simple_tag_lines(bytes, simple, &mut lines)?;
                }
            }
        }
        (!lines.is_empty()).then(|| lines.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use image::{GenericImageView, ImageEncoder};

    use super::webm::{
        Element, SEEK, SEEK_HEAD, SEEK_ID, SEEK_POSITION, SEGMENT, SIMPLE_TAG, TAG, TAG_NAME,
        TAG_STRING, TAGS, TARGETS,
    };
    use super::*;
    use crate::studio::test_support::{Scratch, png};

    fn label() -> AiLabel {
        AiLabel {
            app_version: "0.1.0".into(),
            model: "FLUX.2 klein 4B".into(),
            revision: "5f1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c".into(),
            id: "0e7c9d52-4a1b-4c8e-9f3a-6b2d1e0f7a95".into(),
            created: "2026-11-18T09:30:00Z".into(),
        }
    }

    #[test]
    fn the_crc_is_the_one_png_uses() {
        assert_eq!(crc32(&[b"IEND"]), 0xAE42_6082);
        assert_eq!(crc32(&[b"1234".as_slice(), b"56789"]), 0xCBF4_3926);
    }

    #[test]
    fn the_xmp_names_the_source_type_the_app_the_model_the_id_and_the_date() {
        let x = xmp(&label());
        for part in [
            "<Iptc4xmpExt:DigitalSourceType>http://cv.iptc.org/newscodes/digitalsourcetype/trainedAlgorithmicMedia</Iptc4xmpExt:DigitalSourceType>",
            "<rdf:li xml:lang=\"x-default\">Made with harnessUI 0.1.0, FLUX.2 klein 4B 5f1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c</rdf:li>",
            "<xmpMM:DocumentID>0e7c9d52-4a1b-4c8e-9f3a-6b2d1e0f7a95</xmpMM:DocumentID>",
            "<xmp:CreateDate>2026-11-18T09:30:00Z</xmp:CreateDate>",
        ] {
            assert!(x.contains(part), "{part}\n{x}");
        }
        let odd = AiLabel {
            model: "A <b> & \"c\" 'd'".into(),
            ..label()
        };
        assert!(xmp(&odd).contains(
            "Made with harnessUI 0.1.0, A &lt;b&gt; &amp; &quot;c&quot; &apos;d&apos; 5f1a"
        ));
    }

    // ---- PNG ----------------------------------------------------------------------------------

    /// A PNG chunk: its length, type, data and CRC-32.
    fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = (data.len() as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        out.extend_from_slice(&crc32(&[kind, data]).to_be_bytes());
        out
    }

    /// An 8 x 8 PNG with `chunks` right after its IHDR, where the engine puts its text.
    fn png_with(chunks: &[Vec<u8>]) -> Vec<u8> {
        let plain = png(8, 8);
        let after_ihdr = 8 + 8 + 13 + 4;
        [&plain[..after_ihdr], &chunks.concat(), &plain[after_ihdr..]].concat()
    }

    fn png_with_text_chunk(keyword: &str, text: &str) -> Vec<u8> {
        png_with(&[png_chunk(b"tEXt", format!("{keyword}\0{text}").as_bytes())])
    }

    /// A PNG's chunk types in order, every length and CRC checked.
    fn png_chunk_types(bytes: &[u8]) -> Vec<[u8; 4]> {
        assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        let mut at = 8;
        let mut kinds = Vec::new();
        while at < bytes.len() {
            let len = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
            let kind: [u8; 4] = bytes[at + 4..at + 8].try_into().unwrap();
            let data = &bytes[at + 8..at + 8 + len];
            let crc = u32::from_be_bytes(bytes[at + 8 + len..at + 12 + len].try_into().unwrap());
            assert_eq!(
                crc,
                crc32(&[&kind, data]),
                "{}",
                String::from_utf8_lossy(&kind)
            );
            kinds.push(kind);
            at += 12 + len;
        }
        assert_eq!(at, bytes.len());
        kinds
    }

    #[test]
    fn a_png_carries_the_ai_label_and_no_prompt() {
        let scratch = Scratch::new();
        let f = scratch.write(
            "a.png",
            &png_with_text_chunk("parameters", "a lighthouse at dusk, steps 4"),
        );
        label_file(&f, "image/png", &label()).unwrap();
        let text = read_label(&f, "image/png").unwrap();
        assert!(
            text.contains(IPTC_TRAINED)
                && text.contains("Made with harnessUI 0.1.0, FLUX.2 klein 4B")
        );
        assert!(!String::from_utf8_lossy(&std::fs::read(&f).unwrap()).contains("lighthouse"));
        assert!(image::load_from_memory(&std::fs::read(&f).unwrap()).is_ok());
    }

    #[test]
    fn every_text_chunk_goes_and_one_label_lands_before_the_picture_data() {
        let scratch = Scratch::new();
        let f = scratch.write(
            "b.png",
            &png_with(&[
                png_chunk(b"tEXt", b"parameters\0a lighthouse at dusk"),
                png_chunk(b"zTXt", b"Comment\0\0a lighthouse at dusk"),
                png_chunk(
                    b"iTXt",
                    b"XML:com.adobe.xmp\0\0\0\0\0<x:xmpmeta>a lighthouse at dusk</x:xmpmeta>",
                ),
            ]),
        );
        label_file(&f, "image/png", &label()).unwrap();
        // Labelling again replaces the label rather than adding a second one.
        label_file(&f, "image/png", &label()).unwrap();
        let after = std::fs::read(&f).unwrap();
        let kinds = png_chunk_types(&after);
        let texts = [*b"tEXt", *b"zTXt", *b"iTXt"];
        assert_eq!(kinds.iter().filter(|k| texts.contains(k)).count(), 1);
        let label_at = kinds.iter().position(|k| k == b"iTXt").unwrap();
        assert_eq!(kinds[label_at + 1], *b"IDAT");
        assert_eq!((kinds[0], kinds[kinds.len() - 1]), (*b"IHDR", *b"IEND"));
        assert!(!String::from_utf8_lossy(&after).contains("lighthouse"));
        assert_eq!(
            image::load_from_memory(&after).unwrap().dimensions(),
            (8, 8)
        );
        assert_eq!(read_label(&f, "image/png").unwrap(), xmp(&label()));
    }

    #[test]
    fn a_file_that_is_not_its_type_is_refused_and_left_as_it_was() {
        let scratch = Scratch::new();
        let junk = scratch.write("junk", b"not a picture");
        for mime in ["image/png", "image/webp", "video/webm"] {
            assert_eq!(
                label_file(&junk, mime, &label()),
                Err(GalleryError::NotMedia),
                "{mime}"
            );
            assert_eq!(read_label(&junk, mime), None, "{mime}");
        }
        assert_eq!(std::fs::read(&junk).unwrap(), b"not a picture");
        // A type no local run makes is refused, never kept without its label.
        let picture = scratch.write("a.jpg", &png(2, 2));
        assert_eq!(
            label_file(&picture, "image/jpeg", &label()),
            Err(GalleryError::NotMedia)
        );
        // A PNG cut off inside a chunk.
        let whole = png(4, 4);
        let cut = scratch.write("cut.png", &whole[..whole.len() - 20]);
        assert_eq!(
            label_file(&cut, "image/png", &label()),
            Err(GalleryError::NotMedia)
        );
        assert!(matches!(
            label_file(&scratch.path().join("gone.png"), "image/png", &label()),
            Err(GalleryError::Io(_))
        ));
    }

    // ---- WebP ---------------------------------------------------------------------------------

    /// A WebP file of `chunks`, each padded to an even length.
    fn riff(chunks: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut body = b"WEBP".to_vec();
        for (kind, data) in chunks {
            body.extend_from_slice(kind);
            body.extend_from_slice(&(data.len() as u32).to_le_bytes());
            body.extend_from_slice(data);
            if data.len() % 2 == 1 {
                body.push(0);
            }
        }
        [
            b"RIFF".as_slice(),
            &(body.len() as u32).to_le_bytes(),
            &body,
        ]
        .concat()
    }

    /// A WebP file's chunks, the RIFF size and every chunk size and padding checked.
    fn webp_chunks(bytes: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
        assert_eq!(
            (&bytes[..4], &bytes[8..12]),
            (b"RIFF".as_slice(), b"WEBP".as_slice())
        );
        let riff_size = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
        assert_eq!(riff_size, bytes.len() - 8);
        let mut at = 12;
        let mut chunks = Vec::new();
        while at < bytes.len() {
            let kind: [u8; 4] = bytes[at..at + 4].try_into().unwrap();
            let len = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
            chunks.push((kind, bytes[at + 8..at + 8 + len].to_vec()));
            at += 8 + len + len % 2;
        }
        assert_eq!(at, bytes.len());
        chunks
    }

    /// The `VP8 ` chunk of a 4 x 4 lossy WebP of one colour, as libwebp encodes it.
    const VP8_4X4: [u8; 42] = [
        0xb0, 0x01, 0x00, 0x9d, 0x01, 0x2a, 0x04, 0x00, 0x04, 0x00, 0x02, 0xc0, 0x4c, 0x25, 0xa0,
        0x02, 0x74, 0xba, 0x00, 0x05, 0xd4, 0x00, 0x00, 0xfd, 0xc7, 0x1f, 0xf8, 0x56, 0xb9, 0x87,
        0xe6, 0x1e, 0xd6, 0xff, 0xdc, 0x19, 0x9f, 0x57, 0x5f, 0xe0, 0x00, 0x00,
    ];

    /// A lossless WebP of 4 x 3 pixels: with see-through pixels when `alpha` (a simple `VP8L`
    /// file), and an extended one (`VP8X`) with an EXIF chunk when `exif`.
    fn lossless(alpha: bool, exif: bool) -> Vec<u8> {
        let mut out = Vec::new();
        let mut encoder = image::codecs::webp::WebPEncoder::new_lossless(&mut out);
        if exif {
            // A big-endian TIFF header with one empty IFD.
            encoder
                .set_exif_metadata(b"MM\0\x2a\0\0\0\x08\0\0\0\0\0\0".to_vec())
                .unwrap();
        }
        if alpha {
            let mut picture = image::RgbaImage::from_pixel(4, 3, image::Rgba([200, 120, 40, 255]));
            picture.put_pixel(0, 0, image::Rgba([0, 0, 0, 0]));
            encoder
                .write_image(picture.as_raw(), 4, 3, image::ExtendedColorType::Rgba8)
                .unwrap();
        } else {
            let picture = image::RgbImage::from_pixel(4, 3, image::Rgb([200, 120, 40]));
            encoder
                .write_image(picture.as_raw(), 4, 3, image::ExtendedColorType::Rgb8)
                .unwrap();
        }
        out
    }

    /// `webp` (an extended file) with an XMP chunk holding a prompt, flagged in its VP8X.
    fn with_old_xmp(webp: &[u8]) -> Vec<u8> {
        let mut chunks = webp_chunks(webp);
        assert_eq!(chunks[0].0, *b"VP8X");
        chunks[0].1[0] |= 0x04;
        chunks.push((
            *b"XMP ",
            b"<x:xmpmeta>a lighthouse at dusk</x:xmpmeta>".to_vec(),
        ));
        riff(&chunks)
    }

    #[test]
    fn a_webp_carries_the_label() {
        let scratch = Scratch::new();
        let simple_lossless = lossless(true, false);
        assert_eq!(webp_chunks(&simple_lossless)[0].0, *b"VP8L");
        let cases = [
            (
                "lossy",
                riff(&[(*b"VP8 ", VP8_4X4.to_vec())]),
                (4, 4),
                false,
            ),
            ("lossless", simple_lossless, (4, 3), true),
            (
                "extended",
                with_old_xmp(&lossless(false, true)),
                (4, 3),
                false,
            ),
        ];
        for (name, bytes, size, alpha) in cases {
            assert!(image::load_from_memory(&bytes).is_ok(), "{name}");
            let f = scratch.write(&format!("{name}.webp"), &bytes);
            label_file(&f, "image/webp", &label()).unwrap();
            assert!(
                read_label(&f, "image/webp").unwrap().contains(IPTC_TRAINED),
                "{name}"
            );
            let after = std::fs::read(&f).unwrap();
            let chunks = webp_chunks(&after);
            let (kind, vp8x) = &chunks[0];
            assert_eq!(kind, b"VP8X", "{name}");
            assert_eq!(vp8x.len(), 10, "{name}");
            assert_eq!(vp8x[0] & 0x04, 0x04, "{name}: the XMP flag");
            assert_eq!(vp8x[0] & 0x10 != 0, alpha, "{name}: the alpha flag");
            let canvas =
                |at: usize| u32::from_le_bytes([vp8x[at], vp8x[at + 1], vp8x[at + 2], 0]) + 1;
            assert_eq!((canvas(4), canvas(7)), size, "{name}: the canvas");
            let xmps = chunks.iter().filter(|(k, _)| k == b"XMP ").count();
            assert_eq!(xmps, 1, "{name}");
            assert_eq!(chunks[chunks.len() - 1].0, *b"XMP ", "{name}");
            assert!(
                !String::from_utf8_lossy(&after).contains("lighthouse"),
                "{name}"
            );
            let picture = image::load_from_memory(&after).unwrap();
            assert_eq!(picture.dimensions(), size, "{name}");
            assert_eq!(picture.color().has_alpha(), alpha, "{name}");
        }
        // The EXIF chunk and its flag stay.
        let f = scratch.path().join("extended.webp");
        let chunks = webp_chunks(&std::fs::read(&f).unwrap());
        assert_eq!(chunks[0].1[0] & 0x08, 0x08);
        assert!(chunks.iter().any(|(k, _)| k == b"EXIF"));
    }

    #[test]
    fn a_webp_label_of_either_length_keeps_the_riff_sizes_right() {
        let scratch = Scratch::new();
        let f = scratch.write("a.webp", &riff(&[(*b"VP8 ", VP8_4X4.to_vec())]));
        for model in ["FLUX.2 klein 4B", "FLUX.2 klein 4B x"] {
            let label = AiLabel {
                model: model.into(),
                ..label()
            };
            label_file(&f, "image/webp", &label).unwrap();
            let after = std::fs::read(&f).unwrap();
            let chunks = webp_chunks(&after);
            let xmps: Vec<_> = chunks.iter().filter(|(k, _)| k == b"XMP ").collect();
            assert_eq!(xmps.len(), 1);
            assert_eq!(xmps[0].1, xmp(&label).as_bytes());
            assert!(image::load_from_memory(&after).is_ok());
        }
    }

    // ---- WebM ---------------------------------------------------------------------------------

    const EBML: u32 = 0x1A45_DFA3;
    const INFO: u32 = 0x1549_A966;
    const TRACKS: u32 = 0x1654_AE6B;
    const CLUSTER: u32 = 0x1F43_B675;
    const CUES: u32 = 0x1C53_BB6B;
    const VOID: u32 = 0xEC;

    fn id_bytes(id: u32) -> Vec<u8> {
        id.to_be_bytes()
            .into_iter()
            .skip_while(|b| *b == 0)
            .collect()
    }

    /// An EBML size eight bytes wide, as libwebm writes a Segment's and a Cluster's.
    fn wide(n: u64) -> Vec<u8> {
        (n | 1 << 56).to_be_bytes().to_vec()
    }

    /// An EBML element with a one-byte size (under 127 bytes) or an eight-byte one.
    fn ebml(id: u32, payload: &[u8]) -> Vec<u8> {
        let size = if payload.len() < 127 {
            vec![0x80 | payload.len() as u8]
        } else {
            wide(payload.len() as u64)
        };
        [id_bytes(id), size, payload.to_vec()].concat()
    }

    fn uint(id: u32, value: u64) -> Vec<u8> {
        ebml(id, &value.to_be_bytes())
    }

    fn text(id: u32, value: &str) -> Vec<u8> {
        ebml(id, value.as_bytes())
    }

    /// A SeekHead entry, 21 bytes with its eight-byte position.
    fn seek(id: u32, position: u64) -> Vec<u8> {
        ebml(
            SEEK,
            &[ebml(SEEK_ID, &id_bytes(id)), uint(SEEK_POSITION, position)].concat(),
        )
    }

    /// The bytes libwebm keeps for its SeekHead: room for five entries, the rest a Void.
    const SEEK_ROOM: usize = 5 + 5 * 21;

    /// A WebM file laid out as the engine's muxer (libwebm) writes one: the EBML header, then a
    /// Segment with an eight-byte size holding a SeekHead (Info, Tracks, the Tags when there
    /// are any, Cues) in room for five entries, Info, Tracks, Tags with `comment` when given,
    /// one Cluster and the Cues. Without `seek_room`, the SeekHead has no Void after it.
    fn webm(comment: Option<&str>, seek_room: bool) -> Vec<u8> {
        let header = ebml(
            EBML,
            &[text(0x4282, "webm"), uint(0x4287, 2), uint(0x4285, 2)].concat(),
        );
        let info = ebml(
            INFO,
            &[
                uint(0x2A_D7B1, 1_000_000),
                text(0x4D80, "test"),
                text(0x5741, "test"),
            ]
            .concat(),
        );
        let video = ebml(0xE0, &[uint(0xB0, 4), uint(0xBA, 4)].concat());
        let track = [
            uint(0xD7, 1),
            uint(0x73C5, 1),
            uint(0x83, 1),
            text(0x86, "V_VP8"),
            video,
        ];
        let tracks = ebml(TRACKS, &ebml(0xAE, &track.concat()));
        let tags = comment
            .map(|comment| {
                let simple = [text(TAG_NAME, "COMMENT"), text(TAG_STRING, comment)].concat();
                let tag = [ebml(TARGETS, &[]), ebml(SIMPLE_TAG, &simple)].concat();
                ebml(TAGS, &ebml(TAG, &tag))
            })
            .unwrap_or_default();
        let frame = ebml(0xA3, &[0x81, 0, 0, 0x80, 1, 2, 3, 4, 5, 6]);
        let cluster_body = [uint(0xE7, 0), frame].concat();
        let cluster = [
            id_bytes(CLUSTER),
            wide(cluster_body.len() as u64),
            cluster_body,
        ]
        .concat();

        let entries = 3 + usize::from(comment.is_some());
        let room = if seek_room {
            SEEK_ROOM
        } else {
            5 + entries * 21
        };
        let info_at = room as u64;
        let tracks_at = info_at + info.len() as u64;
        let tags_at = tracks_at + tracks.len() as u64;
        let cluster_at = tags_at + tags.len() as u64;
        let cues_at = cluster_at + cluster.len() as u64;
        let position = [uint(0xF7, 1), uint(0xF1, cluster_at)].concat();
        let cue = [uint(0xB3, 0), ebml(0xB7, &position)].concat();
        let cues = ebml(CUES, &ebml(0xBB, &cue));
        let mut seeks = vec![seek(INFO, info_at), seek(TRACKS, tracks_at)];
        if comment.is_some() {
            seeks.push(seek(TAGS, tags_at));
        }
        seeks.push(seek(CUES, cues_at));
        let seek_head = ebml(SEEK_HEAD, &seeks.concat());
        let void = match room - seek_head.len() {
            0 => Vec::new(),
            n => [vec![0xEC, 0x80 | (n - 2) as u8], vec![0; n - 2]].concat(),
        };
        let body = [seek_head, void, info, tracks, tags, cluster, cues].concat();
        [header, id_bytes(SEGMENT), wide(body.len() as u64), body].concat()
    }

    fn uint_of(bytes: &[u8], e: &Element) -> u64 {
        bytes[e.data..e.end]
            .iter()
            .fold(0, |n, b| n << 8 | u64::from(*b))
    }

    /// The child of `e` with `id`.
    fn child(bytes: &[u8], e: &Element, id: u32) -> Element {
        let kids = webm::children(bytes, e.data, e.end).unwrap();
        *kids.iter().find(|k| k.id == id).unwrap()
    }

    /// Walks a WebM file as a player would: the Segment runs to the end of the file, its
    /// children follow each other with no gap or overlap, every SeekHead entry points at an
    /// element of its id, and the Cues point at the Cluster. Gives the ids of the Segment's
    /// children and of the elements its SeekHead points at.
    fn walk(bytes: &[u8]) -> (Vec<u32>, Vec<u32>) {
        let segment = webm::segment(bytes).unwrap();
        assert_eq!(segment.end, bytes.len());
        let kids = webm::children(bytes, segment.data, segment.end).unwrap();
        let at = |position: u64| {
            kids.iter()
                .find(|e| (e.start - segment.data) as u64 == position)
                .map(|e| e.id)
        };
        let head = kids.iter().find(|e| e.id == SEEK_HEAD).unwrap();
        let mut sought = Vec::new();
        for entry in webm::children(bytes, head.data, head.end).unwrap() {
            let id = uint_of(bytes, &child(bytes, &entry, SEEK_ID)) as u32;
            let position = uint_of(bytes, &child(bytes, &entry, SEEK_POSITION));
            assert_eq!(at(position), Some(id), "the entry for {id:#x}");
            sought.push(id);
        }
        let cues = kids.iter().find(|e| e.id == CUES).unwrap();
        let positions = child(bytes, &child(bytes, cues, 0xBB), 0xB7);
        let cluster_at = uint_of(bytes, &child(bytes, &positions, 0xF1));
        assert_eq!(at(cluster_at), Some(CLUSTER));
        (kids.iter().map(|e| e.id).collect(), sought)
    }

    fn the_label_tags() -> String {
        format!(
            "TITLE=0e7c9d52-4a1b-4c8e-9f3a-6b2d1e0f7a95\n\
             COMMENT=Made with harnessUI 0.1.0, FLUX.2 klein 4B 5f1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c\n\
             DATE_RECORDED=2026-11-18T09:30:00Z\n\
             DIGITAL_SOURCE_TYPE={IPTC_TRAINED}"
        )
    }

    #[test]
    fn a_webm_carries_the_label_and_its_old_tags_go() {
        let scratch = Scratch::new();
        let before = webm(Some("prompt: a lighthouse at dusk"), true);
        assert_eq!(walk(&before).1, [INFO, TRACKS, TAGS, CUES]);
        let f = scratch.write("a.webm", &before);
        assert_eq!(
            read_label(&f, "video/webm").unwrap(),
            "COMMENT=prompt: a lighthouse at dusk"
        );
        label_file(&f, "video/webm", &label()).unwrap();
        let after = std::fs::read(&f).unwrap();
        assert!(!String::from_utf8_lossy(&after).contains("lighthouse"));
        let (kids, sought) = walk(&after);
        // The old Tags became a Void of the same size, so nothing after them moved.
        assert_eq!(
            kids,
            [SEEK_HEAD, VOID, INFO, TRACKS, VOID, CLUSTER, CUES, TAGS]
        );
        assert_eq!(sought, [INFO, TRACKS, CUES, TAGS]);
        assert_eq!(read_label(&f, "video/webm").unwrap(), the_label_tags());
    }

    #[test]
    fn a_webm_from_the_engine_gets_its_label_and_a_seek_entry_for_it() {
        let scratch = Scratch::new();
        let f = scratch.write("a.webm", &webm(None, true));
        assert_eq!(read_label(&f, "video/webm"), None);
        label_file(&f, "video/webm", &label()).unwrap();
        let (kids, sought) = walk(&std::fs::read(&f).unwrap());
        assert_eq!(kids, [SEEK_HEAD, VOID, INFO, TRACKS, CLUSTER, CUES, TAGS]);
        assert_eq!(sought, [INFO, TRACKS, CUES, TAGS]);
        // A second label replaces the first.
        label_file(&f, "video/webm", &label()).unwrap();
        let (kids, sought) = walk(&std::fs::read(&f).unwrap());
        assert_eq!(
            kids,
            [SEEK_HEAD, VOID, INFO, TRACKS, CLUSTER, CUES, VOID, TAGS]
        );
        assert_eq!(sought, [INFO, TRACKS, CUES, TAGS]);
        assert_eq!(read_label(&f, "video/webm").unwrap(), the_label_tags());
    }

    #[test]
    fn a_webm_whose_seek_head_has_no_room_is_still_labelled() {
        let scratch = Scratch::new();
        // No room, and no Tags entry to give up: the SeekHead stays as it was.
        let f = scratch.write("a.webm", &webm(None, false));
        label_file(&f, "video/webm", &label()).unwrap();
        let (kids, sought) = walk(&std::fs::read(&f).unwrap());
        assert_eq!(kids, [SEEK_HEAD, INFO, TRACKS, CLUSTER, CUES, TAGS]);
        assert_eq!(sought, [INFO, TRACKS, CUES]);
        assert_eq!(read_label(&f, "video/webm").unwrap(), the_label_tags());
        // The entry of old Tags makes room for the new one, shorter with its position in
        // the fewest bytes, so a Void fills the rest.
        let g = scratch.write("b.webm", &webm(Some("a lighthouse at dusk"), false));
        label_file(&g, "video/webm", &label()).unwrap();
        let (kids, sought) = walk(&std::fs::read(&g).unwrap());
        assert_eq!(
            kids,
            [SEEK_HEAD, VOID, INFO, TRACKS, VOID, CLUSTER, CUES, TAGS]
        );
        assert_eq!(sought, [INFO, TRACKS, CUES, TAGS]);
    }

    #[test]
    fn a_webm_segment_of_unknown_size_keeps_it_unknown() {
        let scratch = Scratch::new();
        let mut bytes = webm(None, true);
        let segment = webm::segment(&bytes).unwrap();
        bytes[segment.size_at..segment.data]
            .copy_from_slice(&[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
        let f = scratch.write("a.webm", &bytes);
        label_file(&f, "video/webm", &label()).unwrap();
        let after = std::fs::read(&f).unwrap();
        assert_eq!(
            &after[segment.size_at..segment.data],
            &bytes[segment.size_at..segment.data]
        );
        assert_eq!(walk(&after).0.last(), Some(&TAGS));
        assert_eq!(read_label(&f, "video/webm").unwrap(), the_label_tags());
    }

    #[test]
    fn a_webm_cut_short_is_refused() {
        let scratch = Scratch::new();
        let whole = webm(None, true);
        let f = scratch.write("a.webm", &whole[..whole.len() - 3]);
        assert_eq!(
            label_file(&f, "video/webm", &label()),
            Err(GalleryError::NotMedia)
        );
        let g = scratch.write("b.webm", &png(2, 2));
        assert_eq!(
            label_file(&g, "video/webm", &label()),
            Err(GalleryError::NotMedia)
        );
    }
}
