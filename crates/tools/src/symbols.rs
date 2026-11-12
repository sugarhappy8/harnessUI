//! File and symbol mentions in the message box (ARCHITECTURE.md 9.3).
//!
//! - [`find_symbols`] is a light definition finder: line patterns over text whose comments
//!   and strings were blanked first ([`lex`]), not a parser. It gives each definition a
//!   name, a kind and a line range.
//! - [`index`] walks approved folders in the background and answers the picker's searches
//!   over file paths and definitions.
//! - [`resolve`] turns a chosen mention into the text the model gets. It reads the file
//!   through the path sandbox, exactly as `read_file` does.

mod find;
pub mod index;
mod lex;
pub mod resolve;

pub use find::{Lang, MAX_SYMBOLS_PER_FILE, Symbol, SymbolKind, find_symbols};
