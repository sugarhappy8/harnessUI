//! External server configuration (ARCHITECTURE.md 9.2): import, credential scanning and
//! redaction for logs.
//!
//! [`parse_config`] accepts what users paste from other hosts:
//!
//! - **Claude Desktop / Cursor:** `{"mcpServers": {"name": {"command", "args", "env"}}}`,
//!   and Cursor's remote form `{"url", "headers"}` with no `type`.
//! - **Claude Code:** the same, extended with `"type": "stdio" | "http" |
//!   "streamable-http" | "sse"`, `url` and `headers`.
//! - **VS Code:** `{"servers": {...}}` (also nested under `"mcp"`), JSONC comments and
//!   trailing commas included.
//! - A bare map of servers with no wrapper.
//!
//! Secrets never stay in a saved config: `env` and `headers` values move to the keychain
//! on save, and [`scan_credentials`] finds likely credentials in `args` and URLs so the UI
//! can offer to move them too, behind a [`crate::secrets::placeholder`].

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub use crate::names::exposed_name;
use crate::secrets;

/// Largest config text [`parse_config`] accepts.
pub const MAX_CONFIG_BYTES: usize = 1024 * 1024;

/// Longest server name accepted.
pub const MAX_SERVER_NAME_LEN: usize = 100;

/// Headers the transport sets itself; a config may not override them.
const RESERVED_HEADERS: &[&str] = &[
    "accept",
    "mcp-session-id",
    "mcp-protocol-version",
    "last-event-id",
    "content-type",
    "content-length",
    "host",
];

/// One server from a config: its name (the key in the config) and how to reach it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerEntry {
    /// The server's name as the user sees it; also the prefix of its exposed tool names.
    pub name: String,
    /// How to start or reach it.
    pub config: ServerConfig,
}

/// How to start or reach one external server.
///
/// `Debug` hides `env` and header values, argument text and URL credentials.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ServerConfig {
    /// A local process speaking MCP over stdin/stdout.
    Stdio {
        /// Program name or absolute path; resolved to an absolute path before spawning.
        command: String,
        /// Arguments, passed without a shell (on Windows `.cmd` shims go through
        /// `cmd.exe` with strict quoting, see [`crate::winquote`]).
        #[serde(default)]
        args: Vec<String>,
        /// Extra environment variables (values are usually secret placeholders).
        #[serde(default)]
        env: BTreeMap<String, String>,
        /// Working directory; an absolute path.
        #[serde(default)]
        cwd: Option<String>,
    },
    /// A remote server over Streamable HTTP.
    Http {
        /// The endpoint URL (`http` or `https`).
        url: String,
        /// Static headers sent with every request (values are usually placeholders).
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
    /// An entry declared as the legacy HTTP+SSE transport. It is tried as Streamable HTTP
    /// and reported as unsupported if that fails.
    LegacySse {
        /// The endpoint URL.
        url: String,
        /// Static headers sent with every request.
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
}

impl fmt::Debug for ServerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stdio {
                command,
                args,
                env,
                cwd,
            } => f
                .debug_struct("Stdio")
                .field("command", command)
                .field("args", &format_args!("<{} arguments>", args.len()))
                .field("env", &env.keys().collect::<Vec<_>>())
                .field("cwd", cwd)
                .finish(),
            Self::Http { url, headers } => f
                .debug_struct("Http")
                .field("url", &redact_url(url))
                .field("headers", &headers.keys().collect::<Vec<_>>())
                .finish(),
            Self::LegacySse { url, headers } => f
                .debug_struct("LegacySse")
                .field("url", &redact_url(url))
                .field("headers", &headers.keys().collect::<Vec<_>>())
                .finish(),
        }
    }
}

/// Why a config could not be imported.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// The text is larger than [`MAX_CONFIG_BYTES`].
    #[error("the config is larger than {} KiB", MAX_CONFIG_BYTES / 1024)]
    TooLarge,
    /// The text is not JSON (comments and trailing commas are accepted).
    #[error("not valid JSON (line {line}, column {column}): {message}")]
    InvalidJson {
        /// 1-based line of the error.
        line: usize,
        /// 1-based column of the error.
        column: usize,
        /// The parser's message.
        message: String,
    },
    /// The top level is not a JSON object.
    #[error("the config must be a JSON object")]
    NotAnObject,
    /// No `mcpServers` or `servers` object was found.
    #[error(
        "no servers found: expected an \"mcpServers\" (Claude Desktop, Cursor) or \"servers\" (VS Code) object"
    )]
    NoServers,
    /// One server entry is invalid.
    #[error("server \"{server}\": {reason}")]
    InvalidEntry {
        /// The entry's name.
        server: String,
        /// What is wrong with it.
        reason: String,
    },
    /// A `type` this app cannot connect to (for example `ws`).
    #[error("server \"{server}\": the transport type \"{kind}\" is not supported")]
    UnsupportedType {
        /// The entry's name.
        server: String,
        /// The `type` value.
        kind: String,
    },
}

/// Parses a pasted or imported MCP config into server entries, sorted by name.
///
/// See the module documentation for the accepted shapes. Unknown fields are ignored. An
/// empty `mcpServers` object gives an empty list.
pub fn parse_config(json: &str) -> Result<Vec<ServerEntry>, ConfigError> {
    if json.len() > MAX_CONFIG_BYTES {
        return Err(ConfigError::TooLarge);
    }
    let cleaned = strip_jsonc(json);
    let value: Value = serde_json::from_str(&cleaned).map_err(|e| ConfigError::InvalidJson {
        line: e.line(),
        column: e.column(),
        message: e.to_string(),
    })?;
    let Value::Object(root) = value else {
        return Err(ConfigError::NotAnObject);
    };
    let mut maps: Vec<&Map<String, Value>> = Vec::new();
    for key in ["mcpServers", "servers"] {
        if let Some(found) = root.get(key) {
            match found {
                Value::Object(map) => maps.push(map),
                _ => {
                    return Err(ConfigError::InvalidEntry {
                        server: key.to_owned(),
                        reason: "must be an object mapping server names to settings".into(),
                    });
                }
            }
        }
    }
    if maps.is_empty()
        && let Some(Value::Object(mcp)) = root.get("mcp")
        && let Some(Value::Object(servers)) = mcp.get("servers")
    {
        maps.push(servers);
    }
    if maps.is_empty() {
        if !root.is_empty() && root.values().all(looks_like_server) {
            maps.push(&root);
        } else {
            return Err(ConfigError::NoServers);
        }
    }
    let mut entries: BTreeMap<String, ServerConfig> = BTreeMap::new();
    for map in maps {
        for (name, value) in map {
            let name = validate_name(name)?;
            if entries.contains_key(&name) {
                return Err(ConfigError::InvalidEntry {
                    server: name,
                    reason: "appears twice".into(),
                });
            }
            let config = parse_entry(&name, value)?;
            entries.insert(name, config);
        }
    }
    Ok(entries
        .into_iter()
        .map(|(name, config)| ServerEntry { name, config })
        .collect())
}

fn looks_like_server(value: &Value) -> bool {
    value.as_object().is_some_and(|o| {
        o.contains_key("command") || o.contains_key("url") || o.contains_key("serverUrl")
    })
}

fn validate_name(name: &str) -> Result<String, ConfigError> {
    let trimmed = name.trim();
    let bad = |reason: &str| ConfigError::InvalidEntry {
        server: policy::sanitize::one_line(name, 60),
        reason: reason.to_owned(),
    };
    if trimmed.is_empty() {
        return Err(bad("the server name is empty"));
    }
    if trimmed.chars().count() > MAX_SERVER_NAME_LEN {
        return Err(bad("the server name is too long"));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(bad("the server name contains control characters"));
    }
    Ok(trimmed.to_owned())
}

fn parse_entry(name: &str, value: &Value) -> Result<ServerConfig, ConfigError> {
    let invalid = |reason: String| ConfigError::InvalidEntry {
        server: name.to_owned(),
        reason,
    };
    let Value::Object(obj) = value else {
        return Err(invalid("must be an object".into()));
    };
    let kind = match obj.get("type").or_else(|| obj.get("transport")) {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.trim().to_ascii_lowercase()),
        Some(_) => return Err(invalid("\"type\" must be a string".into())),
    };
    let command = obj.get("command");
    let url = obj.get("url").or_else(|| obj.get("serverUrl"));
    if command.is_some() && url.is_some() {
        return Err(invalid("has both a \"command\" and a \"url\"".into()));
    }
    enum Want {
        Stdio,
        Http,
        Sse,
    }
    let want = match kind.as_deref() {
        None if command.is_some() => Want::Stdio,
        None if url.is_some() => Want::Http,
        None => {
            return Err(invalid(
                "needs a \"command\" (local) or a \"url\" (remote)".into(),
            ));
        }
        Some("stdio") => Want::Stdio,
        Some("http" | "streamable-http" | "streamablehttp" | "streamable_http") => Want::Http,
        Some("sse") => Want::Sse,
        Some(other) => {
            return Err(ConfigError::UnsupportedType {
                server: name.to_owned(),
                kind: policy::sanitize::one_line(other, 40),
            });
        }
    };
    match want {
        Want::Stdio => {
            let Some(Value::String(command)) = command else {
                return Err(invalid(
                    "a local server needs \"command\" as a string".into(),
                ));
            };
            if command.trim().is_empty() || command.contains('\0') {
                return Err(invalid("\"command\" is empty or invalid".into()));
            }
            let args = match obj.get("args") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(items)) => items
                    .iter()
                    .enumerate()
                    .map(|(i, item)| {
                        scalar_string(item)
                            .filter(|s| !s.contains('\0'))
                            .ok_or_else(|| invalid(format!("argument {} must be a string", i + 1)))
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                Some(_) => return Err(invalid("\"args\" must be a list of strings".into())),
            };
            let env = string_map(obj.get("env"), "env", &invalid)?;
            for key in env.keys() {
                if key.is_empty() || key.contains('=') || key.contains('\0') {
                    return Err(invalid(format!(
                        "\"{}\" is not a valid environment variable name",
                        policy::sanitize::one_line(key, 40)
                    )));
                }
            }
            if env.values().any(|v| v.contains('\0')) {
                return Err(invalid(
                    "an environment value contains a NUL character".into(),
                ));
            }
            let cwd = match obj.get("cwd") {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) if !s.trim().is_empty() && !s.contains('\0') => {
                    Some(s.clone())
                }
                Some(_) => return Err(invalid("\"cwd\" must be a folder path".into())),
            };
            Ok(ServerConfig::Stdio {
                command: command.clone(),
                args,
                env,
                cwd,
            })
        }
        Want::Http | Want::Sse => {
            let Some(Value::String(url)) = url else {
                return Err(invalid("a remote server needs \"url\" as a string".into()));
            };
            validate_url(url).map_err(invalid)?;
            let headers = string_map(obj.get("headers"), "headers", &invalid)?;
            for (header, value) in &headers {
                validate_header(header, value).map_err(invalid)?;
            }
            Ok(if matches!(want, Want::Sse) {
                ServerConfig::LegacySse {
                    url: url.clone(),
                    headers,
                }
            } else {
                ServerConfig::Http {
                    url: url.clone(),
                    headers,
                }
            })
        }
    }
}

/// A JSON string, number or boolean as text.
fn scalar_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn string_map(
    value: Option<&Value>,
    field: &str,
    invalid: &dyn Fn(String) -> ConfigError,
) -> Result<BTreeMap<String, String>, ConfigError> {
    match value {
        None | Some(Value::Null) => Ok(BTreeMap::new()),
        Some(Value::Object(map)) => map
            .iter()
            .map(|(k, v)| {
                scalar_string(v).map(|v| (k.clone(), v)).ok_or_else(|| {
                    invalid(format!(
                        "\"{field}\" value for \"{}\" must be a string",
                        policy::sanitize::one_line(k, 40)
                    ))
                })
            })
            .collect(),
        Some(_) => Err(invalid(format!("\"{field}\" must be an object of strings"))),
    }
}

/// Checks a server URL, with any secret placeholders standing in for real values.
pub(crate) fn validate_url(url: &str) -> Result<(), String> {
    let probe = replace_placeholders(url, "x");
    let parsed =
        url::Url::parse(probe.trim()).map_err(|e| format!("\"url\" is not a valid URL ({e})"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("\"url\" must start with http:// or https://".into());
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err("\"url\" has no host".into());
    }
    Ok(())
}

/// Checks one configured header, with any secret placeholders standing in for real values.
pub(crate) fn validate_header(name: &str, value: &str) -> Result<(), String> {
    let header = http::HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
        format!(
            "\"{}\" is not a valid header name",
            policy::sanitize::one_line(name, 40)
        )
    })?;
    if RESERVED_HEADERS.contains(&header.as_str()) {
        return Err(format!(
            "the header \"{name}\" is set by the app and cannot be configured"
        ));
    }
    let probe = replace_placeholders(value, "x");
    http::HeaderValue::from_str(&probe)
        .map_err(|_| format!("the value of header \"{name}\" contains invalid characters"))?;
    Ok(())
}

fn replace_placeholders(text: &str, with: &str) -> String {
    let mut out = text.to_owned();
    for name in secrets::placeholder_names(text) {
        out = out.replace(&secrets::placeholder(&name), with);
    }
    out
}

/// Removes `//` and `/* */` comments outside strings, and trailing commas before `}` or
/// `]`, so VS Code's JSONC files parse as JSON. Positions of the remaining text shift
/// only where comments or commas were removed.
fn strip_jsonc(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut in_string = false;
    while i < chars.len() {
        let c = chars[i];
        if in_string {
            out.push(c);
            if c == '\\' {
                if let Some(&next) = chars.get(i + 1) {
                    out.push(next);
                    i += 1;
                }
            } else if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
                i += 1;
            }
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                i += 2;
                while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    // Keep line breaks so error positions stay meaningful.
                    if chars[i] == '\n' {
                        out.push('\n');
                    }
                    i += 1;
                }
                i += 2;
            }
            ',' => {
                let mut j = i + 1;
                while j < chars.len() && chars[j].is_whitespace() {
                    j += 1;
                }
                if !matches!(chars.get(j), Some('}') | Some(']')) {
                    out.push(c);
                }
                i += 1;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------------------
// Credential scanning
// ---------------------------------------------------------------------------------------

/// Where a likely credential was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum CredentialLocation {
    /// In `args[index]` (a stdio server).
    Arg {
        /// Index into `args`.
        index: usize,
    },
    /// In the server's `url` (a remote server).
    Url,
}

/// One likely credential in a config, with what to store and what to put in its place.
///
/// `Debug` hides [`value`](Self::value).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialFinding {
    /// Which string holds it.
    pub location: CredentialLocation,
    /// Byte range of the credential inside that string (`start..end`).
    pub start: usize,
    /// End of the byte range (exclusive).
    pub end: usize,
    /// Plain-language description for the UI, e.g. `the value after --api-key`.
    pub what: String,
    /// Suggested keychain name, unique within one scan.
    pub suggested_name: String,
    /// The placeholder to store instead: `${secret:<suggested_name>}`.
    pub placeholder: String,
    /// The credential itself, to move to the keychain. Never log it.
    pub value: String,
}

impl fmt::Debug for CredentialFinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CredentialFinding")
            .field("location", &self.location)
            .field("start", &self.start)
            .field("end", &self.end)
            .field("what", &self.what)
            .field("suggested_name", &self.suggested_name)
            .field("value", &"<redacted>")
            .finish()
    }
}

/// Finds likely credentials in a server's `args` (stdio) or `url` (remote).
///
/// What counts:
///
/// - **Arguments:** the value after a token-style flag (`--token X`, `--api-key=X`,
///   `--password X`, `--client-secret X`, …); `NAME=VALUE` arguments whose name looks
///   secret (`GITHUB_TOKEN=…`); header arguments (`Authorization: Bearer X`,
///   `X-Api-Key: X`); values with a well-known token prefix (`ghp_`, `github_pat_`,
///   `glpat-`, `xoxb-`, `sk-`); and URLs passed as arguments (checked as below).
/// - **URLs:** a password in `user:password@`, a long token used as the user name, and
///   query parameters named like `key`, `api_key`, `token`, `secret`, `password`,
///   `auth`, `sig`.
///
/// Values that already are placeholders or environment references (`${X}`, `$X`, `%X%`)
/// are skipped. Environment and header values are not scanned: they always move to the
/// keychain on save.
pub fn scan_credentials(cfg: &ServerConfig) -> Vec<CredentialFinding> {
    let mut raw: Vec<RawFinding> = Vec::new();
    match cfg {
        ServerConfig::Stdio { args, .. } => scan_args(args, &mut raw),
        ServerConfig::Http { url, .. } | ServerConfig::LegacySse { url, .. } => {
            scan_url(url, CredentialLocation::Url, &mut raw);
        }
    }
    let mut used: Vec<String> = Vec::new();
    raw.into_iter()
        .map(|r| {
            let mut name = r.name.clone();
            let mut n = 2;
            while used.contains(&name) {
                name = format!("{}_{n}", r.name);
                n += 1;
            }
            used.push(name.clone());
            CredentialFinding {
                location: r.location,
                start: r.start,
                end: r.end,
                what: r.what,
                placeholder: secrets::placeholder(&name),
                suggested_name: name,
                value: r.value,
            }
        })
        .collect()
}

/// Returns `cfg` with each finding's credential replaced by its placeholder. Findings
/// whose range no longer matches the config are skipped.
pub fn apply_findings(cfg: &ServerConfig, findings: &[CredentialFinding]) -> ServerConfig {
    let mut cfg = cfg.clone();
    let mut sorted: Vec<&CredentialFinding> = findings.iter().collect();
    // Replace from the end of each string so earlier ranges stay valid.
    sorted.sort_by_key(|f| std::cmp::Reverse((f.location, f.start)));
    for finding in sorted {
        let target: Option<&mut String> = match (&mut cfg, finding.location) {
            (ServerConfig::Stdio { args, .. }, CredentialLocation::Arg { index }) => {
                args.get_mut(index)
            }
            (
                ServerConfig::Http { url, .. } | ServerConfig::LegacySse { url, .. },
                CredentialLocation::Url,
            ) => Some(url),
            _ => None,
        };
        if let Some(text) = target
            && text.get(finding.start..finding.end) == Some(finding.value.as_str())
        {
            text.replace_range(finding.start..finding.end, &finding.placeholder);
        }
    }
    cfg
}

struct RawFinding {
    location: CredentialLocation,
    start: usize,
    end: usize,
    what: String,
    name: String,
    value: String,
}

/// Words that mark a name as secret whatever its value looks like.
const STRONG_WORDS: &[&str] = &[
    "token",
    "tokens",
    "secret",
    "password",
    "passwd",
    "pwd",
    "apikey",
    "credential",
    "credentials",
    "bearer",
    "passphrase",
    "authorization",
    "privatekey",
    "accesskey",
    "secretkey",
    "clientsecret",
];

/// Two-word forms of the same.
const STRONG_PAIRS: &[(&str, &str)] = &[
    ("api", "key"),
    ("access", "key"),
    ("private", "key"),
    ("secret", "key"),
    ("client", "secret"),
    ("account", "key"),
    ("auth", "token"),
];

/// Words that mark a name as secret only when the value looks like a credential.
const WEAK_WORDS: &[&str] = &[
    "key",
    "auth",
    "sig",
    "signature",
    "pat",
    "cookie",
    "session",
];

/// Prefixes of well-known token formats.
const TOKEN_PREFIXES: &[&str] = &[
    "ghp_",
    "gho_",
    "ghs_",
    "ghu_",
    "github_pat_",
    "glpat-",
    "xoxb-",
    "xoxp-",
    "xoxa-",
    "sk-",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Strength {
    None,
    Weak,
    Strong,
}

/// Splits a name into lowercase words on punctuation and on lower-to-upper case changes.
fn name_words(name: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut prev_lower = false;
    for c in name.chars() {
        if !c.is_ascii_alphanumeric() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            prev_lower = false;
            continue;
        }
        if c.is_ascii_uppercase() && prev_lower && !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
        prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        current.push(c.to_ascii_lowercase());
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn name_strength(name: &str) -> Strength {
    let words = name_words(name);
    let joined: String = words.concat();
    if words.iter().any(|w| STRONG_WORDS.contains(&w.as_str()))
        || STRONG_WORDS.contains(&joined.as_str())
        || words
            .windows(2)
            .any(|w| STRONG_PAIRS.contains(&(w[0].as_str(), w[1].as_str())))
    {
        Strength::Strong
    } else if words.iter().any(|w| WEAK_WORDS.contains(&w.as_str())) {
        Strength::Weak
    } else {
        Strength::None
    }
}

/// Whether `value` is worth flagging for a name of the given strength.
fn plausible_value(value: &str, strength: Strength) -> bool {
    let v = value.trim();
    if v.is_empty() || secrets::has_placeholder(v) || is_env_reference(v) {
        return false;
    }
    // Small numbers are limits and ports, not credentials.
    if v.len() <= 6 && v.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    if matches!(
        v.to_ascii_lowercase().as_str(),
        "true" | "false" | "none" | "null"
    ) {
        return false;
    }
    match strength {
        Strength::None => false,
        Strength::Strong => true,
        Strength::Weak => {
            v.len() >= 12
                && v.bytes().any(|b| b.is_ascii_alphabetic())
                && v.bytes().any(|b| b.is_ascii_digit())
        }
    }
}

fn is_env_reference(v: &str) -> bool {
    (v.starts_with("${") && v.ends_with('}'))
        || (v.starts_with('$')
            && v.len() > 1
            && v[1..]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_'))
        || (v.len() > 2 && v.starts_with('%') && v.ends_with('%'))
}

fn has_token_prefix(v: &str) -> bool {
    v.len() >= 20
        && !v.contains(char::is_whitespace)
        && TOKEN_PREFIXES.iter().any(|p| v.starts_with(p))
}

/// A suggested keychain name from a flag, variable or parameter name.
fn secret_name_from(name: &str) -> String {
    let trimmed = name.trim_start_matches('-');
    let mut out: String = trimmed
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    out.truncate(64);
    let out = out.trim_matches(['_', '-', '.']).to_owned();
    if out.is_empty() { "secret".into() } else { out }
}

fn is_env_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b) if b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn scan_args(args: &[String], out: &mut Vec<RawFinding>) {
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        let location = CredentialLocation::Arg { index: i };
        let before = out.len();
        if arg.starts_with("http://") || arg.starts_with("https://") {
            scan_url(arg, location, out);
        } else if arg.starts_with('-') {
            if let Some((flag, value)) = arg.split_once('=') {
                let strength = name_strength(flag);
                if plausible_value(value, strength) {
                    out.push(RawFinding {
                        location,
                        start: flag.len() + 1,
                        end: arg.len(),
                        what: format!("the value of {}", policy::sanitize::one_line(flag, 40)),
                        name: secret_name_from(flag),
                        value: value.to_owned(),
                    });
                }
            } else {
                let strength = name_strength(arg);
                if strength != Strength::None
                    && let Some(next) = args.get(i + 1)
                    && !next.starts_with('-')
                {
                    if plausible_value(next, strength) {
                        out.push(RawFinding {
                            location: CredentialLocation::Arg { index: i + 1 },
                            start: 0,
                            end: next.len(),
                            what: format!(
                                "the value after {}",
                                policy::sanitize::one_line(arg, 40)
                            ),
                            name: secret_name_from(arg),
                            value: next.clone(),
                        });
                    }
                    // The value was consumed by the flag either way.
                    i += 2;
                    continue;
                }
            }
        } else if let Some(finding) = scan_header_arg(arg, location) {
            out.push(finding);
        } else if let Some((name, value)) = arg.split_once('=')
            && is_env_name(name)
            && plausible_value(value, name_strength(name))
        {
            out.push(RawFinding {
                location,
                start: name.len() + 1,
                end: arg.len(),
                what: format!("the value of {name}"),
                name: secret_name_from(name),
                value: value.to_owned(),
            });
        }
        if out.len() == before && has_token_prefix(arg) {
            out.push(RawFinding {
                location,
                start: 0,
                end: arg.len(),
                what: "an argument that looks like an access token".into(),
                name: "token".into(),
                value: arg.clone(),
            });
        }
        i += 1;
    }
}

/// `Authorization: Bearer X`, `X-Api-Key: X` and similar header arguments.
fn scan_header_arg(arg: &str, location: CredentialLocation) -> Option<RawFinding> {
    let colon = arg.find(':')?;
    let name = arg[..colon].trim();
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return None;
    }
    let strength = name_strength(name);
    if strength == Strength::None {
        return None;
    }
    let rest = &arg[colon + 1..];
    let mut value = rest.trim_start();
    let mut start = colon + 1 + (rest.len() - value.len());
    for scheme in ["Bearer ", "bearer ", "Basic ", "basic ", "Token ", "token "] {
        if let Some(stripped) = value.strip_prefix(scheme) {
            let trimmed = stripped.trim_start();
            start += value.len() - trimmed.len();
            value = trimmed;
            break;
        }
    }
    let value = value.trim_end();
    if !plausible_value(value, strength) {
        return None;
    }
    Some(RawFinding {
        location,
        start,
        end: start + value.len(),
        what: format!("the {name} header value"),
        name: secret_name_from(name),
        value: value.to_owned(),
    })
}

fn scan_url(url: &str, location: CredentialLocation, out: &mut Vec<RawFinding>) {
    let Some(scheme_end) = url.find("://") else {
        return;
    };
    let authority_start = scheme_end + 3;
    let after = &url[authority_start..];
    let authority_len = after.find(['/', '?', '#']).unwrap_or(after.len());
    let authority = &after[..authority_len];
    if let Some(at) = authority.rfind('@') {
        let userinfo = &authority[..at];
        match userinfo.split_once(':') {
            Some((user, password))
                if !password.is_empty() && !secrets::has_placeholder(password) =>
            {
                let start = authority_start + user.len() + 1;
                out.push(RawFinding {
                    location,
                    start,
                    end: start + password.len(),
                    what: "the password in the URL".into(),
                    name: "password".into(),
                    value: password.to_owned(),
                });
            }
            None if userinfo.len() >= 16 && !secrets::has_placeholder(userinfo) => {
                out.push(RawFinding {
                    location,
                    start: authority_start,
                    end: authority_start + userinfo.len(),
                    what: "the token used as the URL's user name".into(),
                    name: "token".into(),
                    value: userinfo.to_owned(),
                });
            }
            _ => {}
        }
    }
    let Some(q) = url.find('?') else {
        return;
    };
    let query_end = url[q..].find('#').map_or(url.len(), |h| q + h);
    let mut pos = q + 1;
    for pair in url[q + 1..query_end].split('&') {
        if let Some((key, value)) = pair.split_once('=') {
            let strength = match name_strength(key) {
                Strength::Weak if matches!(key.to_ascii_lowercase().as_str(), "key" | "sig") => {
                    Strength::Strong
                }
                other => other,
            };
            if plausible_value(value, strength) {
                let start = pos + key.len() + 1;
                out.push(RawFinding {
                    location,
                    start,
                    end: start + value.len(),
                    what: format!(
                        "the \"{}\" parameter in the URL",
                        policy::sanitize::one_line(key, 40)
                    ),
                    name: secret_name_from(key),
                    value: value.to_owned(),
                });
            }
        }
        pos += pair.len() + 1;
    }
}

/// A URL safe to log: user name, password, query string and fragment removed. Text that
/// does not parse as a URL is cut at the first `?` or `#` and loses anything before an
/// `@` in its authority.
pub fn redact_url(url: &str) -> String {
    if let Ok(mut parsed) = url::Url::parse(url.trim())
        && parsed.has_host()
    {
        let _ = parsed.set_username("");
        let _ = parsed.set_password(None);
        parsed.set_query(None);
        parsed.set_fragment(None);
        return parsed.to_string();
    }
    let cut = url.find(['?', '#']).unwrap_or(url.len());
    let base = &url[..cut];
    match base.find("://") {
        Some(scheme_end) => {
            let after = &base[scheme_end + 3..];
            let authority_len = after.find('/').unwrap_or(after.len());
            match after[..authority_len].rfind('@') {
                Some(at) => format!("{}{}", &base[..scheme_end + 3], &after[at + 1..]),
                None => base.to_owned(),
            }
        }
        None => base.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stdio(args: &[&str]) -> ServerConfig {
        ServerConfig::Stdio {
            command: "npx".into(),
            args: args.iter().map(|s| (*s).to_owned()).collect(),
            env: BTreeMap::new(),
            cwd: None,
        }
    }

    #[test]
    fn parses_the_claude_desktop_shape() {
        let json = r#"{
          "mcpServers": {
            "filesystem": {
              "command": "npx",
              "args": ["-y", "@modelcontextprotocol/server-filesystem", "/Users/u/Desktop"]
            },
            "brave": {
              "command": "npx",
              "args": ["-y", "@modelcontextprotocol/server-brave-search"],
              "env": {"BRAVE_API_KEY": "abc", "PORT": 8080}
            }
          }
        }"#;
        let entries = parse_config(json).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "brave");
        let ServerConfig::Stdio { env, args, .. } = &entries[0].config else {
            panic!("stdio expected")
        };
        assert_eq!(env.get("BRAVE_API_KEY").map(String::as_str), Some("abc"));
        assert_eq!(env.get("PORT").map(String::as_str), Some("8080"));
        assert_eq!(args.len(), 2);
        assert_eq!(entries[1].name, "filesystem");
    }

    #[test]
    fn parses_the_extended_remote_shapes() {
        let json = r#"{"mcpServers": {
            "stripe": {"type": "http", "url": "https://mcp.stripe.com", "headers": {"Authorization": "Bearer ${secret:stripe}"}},
            "alias": {"type": "streamable-http", "url": "https://a.example/mcp"},
            "old": {"type": "sse", "url": "https://old.example/sse"},
            "local": {"type": "stdio", "command": "uvx", "args": ["mcp-server-git"], "cwd": "/tmp"},
            "cursor": {"url": "https://cursor.example/mcp", "headers": {"X-Api-Key": "k"}}
        }}"#;
        let entries = parse_config(json).unwrap();
        let by_name: BTreeMap<_, _> = entries.into_iter().map(|e| (e.name, e.config)).collect();
        assert!(
            matches!(&by_name["stripe"], ServerConfig::Http { url, headers }
            if url == "https://mcp.stripe.com" && headers["Authorization"] == "Bearer ${secret:stripe}")
        );
        assert!(matches!(&by_name["alias"], ServerConfig::Http { .. }));
        assert!(matches!(&by_name["old"], ServerConfig::LegacySse { .. }));
        assert!(
            matches!(&by_name["local"], ServerConfig::Stdio { cwd: Some(c), .. } if c == "/tmp")
        );
        assert!(matches!(&by_name["cursor"], ServerConfig::Http { .. }));
    }

    #[test]
    fn parses_the_vscode_shape_with_comments() {
        let json = r#"{
          // VS Code allows comments
          "inputs": [],
          "servers": {
            "git": { "type": "stdio", "command": "uvx", "args": ["mcp-server-git",], }, /* trailing */
            "remote": { "type": "http", "url": "https://x.example/mcp" },
          },
        }"#;
        let entries = parse_config(json).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "git");
        assert!(matches!(entries[1].config, ServerConfig::Http { .. }));
        // Nested under "mcp" as in settings.json, and a bare map.
        let nested = r#"{"mcp": {"servers": {"a": {"command": "node", "args": ["s.js"]}}}}"#;
        assert_eq!(parse_config(nested).unwrap()[0].name, "a");
        let bare = r#"{"a": {"command": "node"}, "b": {"url": "https://b.example"}}"#;
        assert_eq!(parse_config(bare).unwrap().len(), 2);
        // A comment marker inside a string is data.
        let in_string =
            r#"{"mcpServers": {"a": {"command": "node", "args": ["http://x//y", "/* no */"]}}}"#;
        let entries = parse_config(in_string).unwrap();
        let ServerConfig::Stdio { args, .. } = &entries[0].config else {
            panic!()
        };
        assert_eq!(args, &vec!["http://x//y".to_owned(), "/* no */".to_owned()]);
    }

    #[test]
    fn empty_server_maps_are_fine() {
        assert!(parse_config(r#"{"mcpServers": {}}"#).unwrap().is_empty());
    }

    #[test]
    fn rejects_malformed_input() {
        assert!(matches!(
            parse_config("{"),
            Err(ConfigError::InvalidJson { .. })
        ));
        assert!(matches!(parse_config("[1]"), Err(ConfigError::NotAnObject)));
        assert!(matches!(
            parse_config(r#"{"x": 1}"#),
            Err(ConfigError::NoServers)
        ));
        assert!(matches!(parse_config("{}"), Err(ConfigError::NoServers)));
        let cases = [
            r#"{"mcpServers": []}"#,
            r#"{"mcpServers": {"a": 1}}"#,
            r#"{"mcpServers": {"a": {}}}"#,
            r#"{"mcpServers": {"a": {"command": ""}}}"#,
            r#"{"mcpServers": {"a": {"command": 5}}}"#,
            r#"{"mcpServers": {"a": {"command": "x", "args": "-y"}}}"#,
            r#"{"mcpServers": {"a": {"command": "x", "args": [{"k": 1}]}}}"#,
            r#"{"mcpServers": {"a": {"command": "x", "env": {"K": null}}}}"#,
            r#"{"mcpServers": {"a": {"command": "x", "env": {"A=B": "1"}}}}"#,
            r#"{"mcpServers": {"a": {"command": "x", "url": "https://h"}}}"#,
            r#"{"mcpServers": {"a": {"type": "http", "url": "ftp://h/x"}}}"#,
            r#"{"mcpServers": {"a": {"type": "http", "url": "not a url"}}}"#,
            r#"{"mcpServers": {"a": {"type": "http"}}}"#,
            r#"{"mcpServers": {"a": {"url": "https://h", "headers": {"Bad Name": "v"}}}}"#,
            r#"{"mcpServers": {"a": {"url": "https://h", "headers": {"Accept": "v"}}}}"#,
            r#"{"mcpServers": {"a": {"url": "https://h", "headers": {"X": "a\nb"}}}}"#,
            r#"{"mcpServers": {"  ": {"command": "x"}}}"#,
            r#"{"mcpServers": {"a": {"command": "x"}}, "servers": {"a": {"command": "y"}}}"#,
        ];
        for case in cases {
            assert!(
                matches!(parse_config(case), Err(ConfigError::InvalidEntry { .. })),
                "{case} -> {:?}",
                parse_config(case)
            );
        }
        assert!(matches!(
            parse_config(r#"{"mcpServers": {"a": {"type": "ws", "url": "wss://h"}}}"#),
            Err(ConfigError::UnsupportedType { kind, .. }) if kind == "ws"
        ));
        let big = format!(
            "{{\"mcpServers\": {{}}, \"pad\": \"{}\"}}",
            "x".repeat(MAX_CONFIG_BYTES)
        );
        assert_eq!(parse_config(&big), Err(ConfigError::TooLarge));
    }

    #[test]
    fn server_config_round_trips_through_serde() {
        let cfg = ServerConfig::Http {
            url: "https://h/mcp".into(),
            headers: BTreeMap::from([("X".to_owned(), "${secret:x}".to_owned())]),
        };
        let json = serde_json::to_value(&cfg).unwrap();
        assert_eq!(json["type"], "http");
        assert_eq!(serde_json::from_value::<ServerConfig>(json).unwrap(), cfg);
        let sse = serde_json::to_value(ServerConfig::LegacySse {
            url: "https://h".into(),
            headers: BTreeMap::new(),
        })
        .unwrap();
        assert_eq!(sse["type"], "legacySse");
        let stdio: ServerConfig =
            serde_json::from_value(serde_json::json!({"type": "stdio", "command": "node"}))
                .unwrap();
        assert_eq!(stdio, stdio_cmd("node"));
    }

    fn stdio_cmd(command: &str) -> ServerConfig {
        ServerConfig::Stdio {
            command: command.into(),
            args: vec![],
            env: BTreeMap::new(),
            cwd: None,
        }
    }

    #[test]
    fn debug_output_hides_secrets() {
        let cfg = ServerConfig::Stdio {
            command: "npx".into(),
            args: vec!["--token".into(), "hunter2-secret".into()],
            env: BTreeMap::from([("API_KEY".to_owned(), "env-secret".to_owned())]),
            cwd: None,
        };
        let shown = format!("{cfg:?}");
        assert!(
            !shown.contains("hunter2-secret") && !shown.contains("env-secret"),
            "{shown}"
        );
        assert!(shown.contains("API_KEY"));
        let http = ServerConfig::Http {
            url: "https://u:pw-secret@h/x?token=q-secret".into(),
            headers: BTreeMap::from([("Authorization".to_owned(), "Bearer h-secret".to_owned())]),
        };
        let shown = format!("{http:?}");
        assert!(!shown.contains("secret"), "{shown}");
    }

    #[test]
    fn finds_token_flags_and_their_values() {
        let cfg = stdio(&[
            "-y",
            "server",
            "--token",
            "abc123secret",
            "--api-key=sk-live-1",
            "--max-tokens",
            "100",
            "--password",
            "${secret:pw}",
            "--client-secret",
            "s3cr3t",
        ]);
        let found = scan_credentials(&cfg);
        let summary: Vec<_> = found
            .iter()
            .map(|f| (f.location, f.value.as_str(), f.suggested_name.as_str()))
            .collect();
        assert_eq!(
            summary,
            vec![
                (
                    CredentialLocation::Arg { index: 3 },
                    "abc123secret",
                    "token"
                ),
                (CredentialLocation::Arg { index: 4 }, "sk-live-1", "api-key"),
                (
                    CredentialLocation::Arg { index: 10 },
                    "s3cr3t",
                    "client-secret"
                ),
            ]
        );
        assert_eq!(found[0].placeholder, "${secret:token}");
        assert_eq!((found[1].start, found[1].end), (10, 19));
    }

    #[test]
    fn finds_env_style_and_header_arguments() {
        let cfg = stdio(&[
            "GITHUB_PERSONAL_ACCESS_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz",
            "LOG_LEVEL=debug",
            "--header",
            "Authorization: Bearer eyJhbGciOi.abc",
            "X-Api-Key:k-1234567890",
            "ghp_0123456789abcdefghijklmn",
            "plain",
        ]);
        let found = scan_credentials(&cfg);
        let values: Vec<_> = found
            .iter()
            .map(|f| (f.location, f.value.as_str()))
            .collect();
        assert_eq!(
            values,
            vec![
                (
                    CredentialLocation::Arg { index: 0 },
                    "ghp_abcdefghijklmnopqrstuvwxyz"
                ),
                (CredentialLocation::Arg { index: 3 }, "eyJhbGciOi.abc"),
                (CredentialLocation::Arg { index: 4 }, "k-1234567890"),
                (
                    CredentialLocation::Arg { index: 5 },
                    "ghp_0123456789abcdefghijklmn"
                ),
            ]
        );
        assert_eq!(found[0].suggested_name, "GITHUB_PERSONAL_ACCESS_TOKEN");
        assert_eq!(found[1].suggested_name, "Authorization");
        assert_eq!(found[3].suggested_name, "token");
        let applied = apply_findings(&cfg, &found);
        let ServerConfig::Stdio { args, .. } = applied else {
            panic!()
        };
        assert_eq!(
            args[0],
            "GITHUB_PERSONAL_ACCESS_TOKEN=${secret:GITHUB_PERSONAL_ACCESS_TOKEN}"
        );
        assert_eq!(args[3], "Authorization: Bearer ${secret:Authorization}");
        assert_eq!(args[4], "X-Api-Key:${secret:X-Api-Key}");
        assert_eq!(args[5], "${secret:token}");
        assert_eq!(args[1], "LOG_LEVEL=debug");
    }

    #[test]
    fn duplicate_names_get_suffixes() {
        let cfg = stdio(&["--token", "first-secret", "--token", "second-secret"]);
        let found = scan_credentials(&cfg);
        assert_eq!(found[0].suggested_name, "token");
        assert_eq!(found[1].suggested_name, "token_2");
        assert_eq!(found[1].placeholder, "${secret:token_2}");
    }

    #[test]
    fn finds_url_credentials() {
        let cfg = ServerConfig::Http {
            url: "https://user:pa55word@mcp.example.com/v1?apiKey=AbC123&mode=fast&key=k123&sig=zz#frag"
                .into(),
            headers: BTreeMap::new(),
        };
        let found = scan_credentials(&cfg);
        let values: Vec<_> = found.iter().map(|f| f.value.as_str()).collect();
        assert_eq!(values, vec!["pa55word", "AbC123", "k123", "zz"]);
        let applied = apply_findings(&cfg, &found);
        let ServerConfig::Http { url, .. } = applied else {
            panic!()
        };
        assert_eq!(
            url,
            "https://user:${secret:password}@mcp.example.com/v1?apiKey=${secret:apiKey}&mode=fast&key=${secret:key}&sig=${secret:sig}#frag"
        );
        // A token used as the user name, and URLs inside args.
        let token_user = ServerConfig::LegacySse {
            url: "https://ghp_abcdefghijklmnop1234@host/x".into(),
            headers: BTreeMap::new(),
        };
        assert_eq!(
            scan_credentials(&token_user)[0].value,
            "ghp_abcdefghijklmnop1234"
        );
        let in_args = stdio(&["mcp-remote", "https://h.example/mcp?token=t0k3n"]);
        let found = scan_credentials(&in_args);
        assert_eq!(found[0].location, CredentialLocation::Arg { index: 1 });
        assert_eq!(found[0].value, "t0k3n");
        // Nothing to find.
        let clean = ServerConfig::Http {
            url: "https://h.example/mcp?mode=x&token=${secret:t}".into(),
            headers: BTreeMap::new(),
        };
        assert!(scan_credentials(&clean).is_empty());
    }

    #[test]
    fn finding_debug_hides_the_value() {
        let found = scan_credentials(&stdio(&["--token", "very-secret-value"]));
        let shown = format!("{found:?}");
        assert!(!shown.contains("very-secret-value"), "{shown}");
    }

    #[test]
    fn name_words_split_case_and_punctuation() {
        assert_eq!(name_words("accessToken"), vec!["access", "token"]);
        assert_eq!(name_words("--api-key"), vec!["api", "key"]);
        assert_eq!(name_words("APIKey"), vec!["apikey"]);
        assert!(name_strength("x-api-key") == Strength::Strong);
        assert!(name_strength("--key") == Strength::Weak);
        assert!(name_strength("--port") == Strength::None);
    }

    #[test]
    fn redacts_urls_for_logging() {
        assert_eq!(
            redact_url("https://user:pw@h.example:8443/mcp/v1?token=x#y"),
            "https://h.example:8443/mcp/v1"
        );
        assert_eq!(redact_url("http://h/"), "http://h/");
        // Placeholders in the host make the URL unparsable; the fallback still redacts.
        assert_eq!(
            redact_url("https://u:p@${secret:host}/x?k=v"),
            "https://${secret:host}/x"
        );
        assert_eq!(redact_url("nonsense?token=1"), "nonsense");
    }
}
