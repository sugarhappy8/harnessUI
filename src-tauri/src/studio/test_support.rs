//! Helpers for Studio's tests: a folder of their own and small media samples.

use std::path::{Path, PathBuf};

/// A folder of its own for one test, removed when it is dropped.
pub(crate) struct Scratch(PathBuf);

impl Scratch {
    pub(crate) fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("studio-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    /// Writes `bytes` to the file `name` in the folder and returns its path.
    pub(crate) fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A valid PNG picture of one colour, `w` x `h` pixels.
pub(crate) fn png(w: u32, h: u32) -> Vec<u8> {
    let picture = image::RgbImage::from_pixel(w, h, image::Rgb([200, 120, 40]));
    let mut out = Vec::new();
    picture
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

/// The start of an MP4 file, enough for a type check: an `ftyp` box (brand `isom`) and an
/// empty `mdat` box.
pub(crate) fn mp4() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&24u32.to_be_bytes());
    out.extend_from_slice(b"ftypisom");
    out.extend_from_slice(&0x200u32.to_be_bytes());
    out.extend_from_slice(b"isomiso2");
    out.extend_from_slice(&8u32.to_be_bytes());
    out.extend_from_slice(b"mdat");
    out
}
