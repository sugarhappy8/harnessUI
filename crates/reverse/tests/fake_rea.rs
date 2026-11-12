//! A fake rea for the `reverse` server's call tests (item5-spec.md WP9g): started with its
//! flag, this test binary is a small MCP server on stdio built on rmcp's server side (like
//! `crates/mcp-host/tests/stdio_e2e.rs`), listing the 255 names of the pinned catalog
//! fixture and recording every call.
//!
//! TODO(0.0.6): WP9g writes the fake and its tests. This placeholder exists so the crate's
//! `[[test]]` entry (WP9a) builds.

fn main() {
    if std::env::args().any(|a| a == "--list") {
        println!("fake_rea: test");
        return;
    }
    println!("fake_rea: no tests yet (TODO(0.0.6): WP9g)");
}
