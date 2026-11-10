//! Secret placeholders (ARCHITECTURE.md 9.2).
//!
//! Values in `env` and `headers`, and credentials found in `args` and URLs, are moved to
//! the OS keychain when a server is saved. What stays in the stored config is a
//! placeholder of the form `${secret:<name>}`, where `<name>` is made of ASCII letters,
//! digits, `_`, `-` and `.`. The host resolves placeholders right before it starts a
//! server, through the resolver the app passes in, and never logs the resolved values.

use std::sync::Arc;

/// Looks a secret up by name (normally in the OS keychain). `None` when it is missing.
pub type SecretResolver = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// The opening of a placeholder.
pub const PLACEHOLDER_PREFIX: &str = "${secret:";

/// Longest secret name accepted in a placeholder.
pub const MAX_SECRET_NAME_LEN: usize = 128;

/// Builds the placeholder for `name`: `${secret:<name>}`.
pub fn placeholder(name: &str) -> String {
    format!("{PLACEHOLDER_PREFIX}{name}}}")
}

/// Whether `name` may appear in a placeholder.
pub fn is_valid_secret_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_SECRET_NAME_LEN
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

/// Why a string could not be resolved.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecretError {
    /// The resolver has no secret of this name.
    #[error("the secret \"{0}\" is not in the keychain")]
    Missing(String),
    /// A `${secret:` that is not closed or names an invalid secret.
    #[error("malformed secret placeholder: {0}")]
    Malformed(String),
}

/// The names of every placeholder in `text`, in order. Malformed placeholders are
/// skipped.
pub fn placeholder_names(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(PLACEHOLDER_PREFIX) {
        let after = &rest[start + PLACEHOLDER_PREFIX.len()..];
        match after.find('}') {
            Some(end) if is_valid_secret_name(&after[..end]) => {
                names.push(after[..end].to_owned());
                rest = &after[end + 1..];
            }
            _ => rest = after,
        }
    }
    names
}

/// Whether `text` holds at least one placeholder.
pub fn has_placeholder(text: &str) -> bool {
    !placeholder_names(text).is_empty()
}

/// Replaces every placeholder in `text` with the secret it names.
///
/// A missing secret or a malformed placeholder is an error; nothing is guessed. Text that
/// merely contains `$` or `${...}` of another kind is left alone.
pub fn resolve(
    text: &str,
    resolver: &dyn Fn(&str) -> Option<String>,
) -> Result<String, SecretError> {
    if !text.contains(PLACEHOLDER_PREFIX) {
        return Ok(text.to_owned());
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(PLACEHOLDER_PREFIX) {
        out.push_str(&rest[..start]);
        let after = &rest[start + PLACEHOLDER_PREFIX.len()..];
        let Some(end) = after.find('}') else {
            return Err(SecretError::Malformed(
                "a \"${secret:\" placeholder is not closed with \"}\"".into(),
            ));
        };
        let name = &after[..end];
        if !is_valid_secret_name(name) {
            return Err(SecretError::Malformed(format!(
                "\"{}\" is not a valid secret name (letters, digits, _ - . only)",
                policy::sanitize::one_line(name, 40)
            )));
        }
        match resolver(name) {
            Some(value) => out.push_str(&value),
            None => return Err(SecretError::Missing(name.to_owned())),
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keychain(name: &str) -> Option<String> {
        match name {
            "gh" => Some("ghp_secret".into()),
            "a.b-c_d" => Some("v".into()),
            _ => None,
        }
    }

    #[test]
    fn resolves_placeholders_anywhere_in_the_text() {
        assert_eq!(
            resolve("Bearer ${secret:gh}", &keychain).unwrap(),
            "Bearer ghp_secret"
        );
        assert_eq!(
            resolve("${secret:gh}:${secret:a.b-c_d}", &keychain).unwrap(),
            "ghp_secret:v"
        );
        assert_eq!(resolve("plain", &keychain).unwrap(), "plain");
        assert_eq!(resolve("${HOME} $X", &keychain).unwrap(), "${HOME} $X");
    }

    #[test]
    fn missing_and_malformed_placeholders_are_errors() {
        assert_eq!(
            resolve("${secret:nope}", &keychain),
            Err(SecretError::Missing("nope".into()))
        );
        assert!(matches!(
            resolve("${secret:gh", &keychain),
            Err(SecretError::Malformed(_))
        ));
        assert!(matches!(
            resolve("${secret:bad name}", &keychain),
            Err(SecretError::Malformed(_))
        ));
        assert!(matches!(
            resolve("${secret:}", &keychain),
            Err(SecretError::Malformed(_))
        ));
    }

    #[test]
    fn lists_placeholder_names() {
        assert_eq!(
            placeholder_names("x ${secret:a} y ${secret:b_2} ${secret:bad name}"),
            vec!["a".to_owned(), "b_2".to_owned()]
        );
        assert!(has_placeholder("${secret:a}"));
        assert!(!has_placeholder("${a}"));
        assert_eq!(placeholder("tok"), "${secret:tok}");
    }

    #[test]
    fn secret_names_are_restricted() {
        assert!(is_valid_secret_name("GITHUB_TOKEN"));
        assert!(is_valid_secret_name("srv.api-key"));
        assert!(!is_valid_secret_name(""));
        assert!(!is_valid_secret_name("a b"));
        assert!(!is_valid_secret_name("a}"));
        assert!(!is_valid_secret_name(&"x".repeat(MAX_SECRET_NAME_LEN + 1)));
    }
}
