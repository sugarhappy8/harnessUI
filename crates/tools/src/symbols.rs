//! Definitions in source files, for file and symbol mentions in the message box.
//!
//! TODO(0.0.5): only the first step exists. `lex` blanks comments and string contents so
//! that line patterns and bracket counts are not fooled by them. Still to write: the
//! definition finder per language, the folder index and the mention resolver.

// Used only by its own tests until the definition finder is written.
#[allow(dead_code)]
mod lex;
