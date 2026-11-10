//! Exposed tool names (ARCHITECTURE.md 9.1).
//!
//! Built-in tools keep short fixed names (`read_file`). External tools are exposed as
//! `<server>__<tool>`, reduced to ASCII letters, digits, `_` and `-`, at most
//! [`MAX_TOOL_NAME_LEN`] characters (the OpenAI function-name limit), with a numeric
//! suffix on collision.

/// Longest exposed tool name.
pub const MAX_TOOL_NAME_LEN: usize = 64;

/// Separator between the server and the tool in an exposed name.
pub const SEPARATOR: &str = "__";

/// Replaces every character outside `[A-Za-z0-9_-]` with `_`.
pub fn sanitize_part(part: &str) -> String {
    part.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Cuts `s` to at most `max` bytes. `s` is ASCII here, so any byte index is a boundary.
fn cut(s: &str, max: usize) -> &str {
    &s[..s.len().min(max)]
}

/// The name an external tool is offered under: `<server>__<tool>`, sanitised, at most 64
/// characters, with `_2`, `_3`, … appended while `taken` says the name is in use.
///
/// The host's `taken` always includes every built-in tool name, so an added server can
/// never shadow a built-in. When the name is too long the server part is shortened first
/// (down to 16 characters), then the tool part.
pub fn exposed_name(server: &str, tool: &str, taken: &dyn Fn(&str) -> bool) -> String {
    let mut server = sanitize_part(server);
    let mut tool = sanitize_part(tool);
    if server.is_empty() {
        server = "server".into();
    }
    if tool.is_empty() {
        tool = "tool".into();
    }
    let base = shorten(&server, &tool, MAX_TOOL_NAME_LEN);
    if !taken(&base) {
        return base;
    }
    let mut n: u64 = 1;
    loop {
        n += 1;
        let suffix = format!("_{n}");
        let candidate = format!(
            "{}{suffix}",
            shorten(&server, &tool, MAX_TOOL_NAME_LEN - suffix.len())
        );
        if !taken(&candidate) {
            return candidate;
        }
    }
}

fn shorten(server: &str, tool: &str, max: usize) -> String {
    let full = server.len() + SEPARATOR.len() + tool.len();
    if full <= max {
        return format!("{server}{SEPARATOR}{tool}");
    }
    let room_for_server = max
        .saturating_sub(SEPARATOR.len() + tool.len())
        .max(16.min(server.len()));
    let server = cut(server, room_for_server);
    let room_for_tool = max.saturating_sub(server.len() + SEPARATOR.len());
    format!("{server}{SEPARATOR}{}", cut(tool, room_for_tool))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn free(_: &str) -> bool {
        false
    }

    #[test]
    fn joins_and_sanitises() {
        assert_eq!(
            exposed_name("github", "create_issue", &free),
            "github__create_issue"
        );
        assert_eq!(
            exposed_name("my server", "do.it!", &free),
            "my_server__do_it_"
        );
        assert_eq!(
            exposed_name("café", "naïve-tool", &free),
            "caf___na_ve-tool"
        );
        assert_eq!(exposed_name("", "", &free), "server__tool");
    }

    #[test]
    fn caps_the_length_at_64() {
        let long_server = "s".repeat(80);
        let long_tool = "t".repeat(80);
        let name = exposed_name(&long_server, "list", &free);
        assert_eq!(name.len(), 64);
        assert!(name.ends_with("__list"));
        let name = exposed_name("srv", &long_tool, &free);
        assert_eq!(name.len(), 64);
        assert!(name.starts_with("srv__ttt"));
        let name = exposed_name(&long_server, &long_tool, &free);
        assert_eq!(name.len(), 64);
        assert!(name.starts_with(&"s".repeat(16)), "{name}");
        assert!(
            name.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        );
    }

    #[test]
    fn collisions_get_numeric_suffixes() {
        let taken: BTreeSet<String> = ["a__b".to_owned(), "a__b_2".to_owned()].into();
        let is_taken = |n: &str| taken.contains(n);
        assert_eq!(exposed_name("a", "b", &is_taken), "a__b_3");
        // "a b" and "a?b" sanitise to the same name.
        let first = exposed_name("x", "a b", &free);
        let second = exposed_name("x", "a?b", &|n| n == first);
        assert_eq!(second, "x__a_b_2");
        // The suffix still fits in 64 characters.
        let long = "t".repeat(70);
        let base = exposed_name("srv", &long, &free);
        let next = exposed_name("srv", &long, &|n| n == base);
        assert_eq!(next.len(), 64);
        assert!(next.ends_with("_2"));
    }

    #[test]
    fn built_in_names_are_never_shadowed() {
        // A built-in that happens to use the separator: the external tool moves aside.
        let builtins: BTreeSet<&str> = ["read_file", "files__read"].into();
        let taken = |n: &str| builtins.contains(n);
        assert_eq!(exposed_name("files", "read", &taken), "files__read_2");
        assert_eq!(exposed_name("files", "write", &taken), "files__write");
    }
}
