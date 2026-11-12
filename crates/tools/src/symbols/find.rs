//! The definition finder: line patterns over masked text (see [`super::lex`]).

#[cfg(test)]
mod tests;

/// Definitions kept from one file at most.
pub const MAX_SYMBOLS_PER_FILE: usize = 2_000;

/// A language the finder knows, told by the file extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    Rust,
    TypeScript,
    JavaScript,
    Python,
    Go,
    Java,
    CSharp,
    Kotlin,
    Swift,
    C,
    Cpp,
}

impl Lang {
    /// The language of a file, by its extension.
    pub fn from_path(path: &str) -> Option<Lang> {
        let _ = path;
        None
    }
}

/// What a definition is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Struct,
    Enum,
    Trait,
    Interface,
    Impl,
    Module,
    Const,
    Type,
}

impl SymbolKind {
    pub fn as_str(self) -> &'static str {
        "function"
    }
    pub fn parse(_s: &str) -> Option<Self> {
        None
    }
}

/// One definition found in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: Box<str>,
    pub kind: SymbolKind,
    pub container: Option<Box<str>>,
    pub line: u32,
    pub lead: u32,
    pub end_line: u32,
}

impl Symbol {
    pub fn qualified(&self, _lang: Lang) -> String {
        self.name.to_string()
    }
}

/// The definitions in `text`.
pub fn find_symbols(text: &str, lang: Lang) -> Vec<Symbol> {
    let _ = (text, lang);
    Vec::new()
}
