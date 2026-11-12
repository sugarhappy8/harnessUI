//! Contract tests with the real rea, Node.js, Kuna, Ghidra and Java (item5-spec.md WP9e):
//! they run only when `REVERSE_PACK_CACHE` names a cache filled by
//! `scripts/pin_reverse.py --download-to`, install through `reverse::install`, start rea
//! through `ReaProcess` and open the fixtures with each decompiler.
//!
//! TODO(0.0.6): WP9e writes these tests. This placeholder exists so the crate's `[[test]]`
//! entry (WP9a) builds. The install half already runs in `tests/install.rs`
//! (`the_pinned_archives_install_from_a_cache`).

#[test]
fn needs_the_pack_cache() {
    if std::env::var_os("REVERSE_PACK_CACHE").is_none() {
        eprintln!("REVERSE_PACK_CACHE is not set; skipped");
    }
}
