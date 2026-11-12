//! File and symbol mentions in the message box (ARCHITECTURE.md 9.3).
//!
//! - [`find_symbols`] is a light definition finder: line patterns over text whose comments
//!   and strings were blanked first ([`lex`]), not a parser. It gives each definition a
//!   name, a kind and a line range.
//! - [`index`] walks approved folders in the background and answers the picker's searches
//!   over file paths and definitions.
//! - [`resolve`] turns a chosen mention into the text the model gets. It reads the file
//!   through the path sandbox, exactly as `read_file` does.

// TODO(0.0.5): unfinished. The finder has the engine and Rust only; `index` and `resolve`
// are empty. The helpers the other languages need are written and not yet used.
#[allow(dead_code)]
mod find;
pub mod index;
#[allow(dead_code)]
mod lex;
pub mod resolve;

pub use find::{Lang, MAX_SYMBOLS_PER_FILE, Symbol, SymbolKind, find_symbols};
