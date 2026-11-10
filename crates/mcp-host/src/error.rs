//! Errors from managing servers.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::secrets::SecretError;
use crate::winquote::QuoteError;

/// A runtime an external server needs, named so the UI can explain what to install
/// (ARCHITECTURE.md 9.2 "Missing runtime").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MissingRuntime {
    /// `node`, `npx`, `npm`, `pnpm`, `yarn`.
    NodeJs,
    /// `uv`, `uvx`, `python`, `python3`, `pip`.
    UvPython,
    /// `docker`.
    Docker,
}

impl MissingRuntime {
    /// The runtime a command belongs to, judged by its file name (any directory and any
    /// `.exe`, `.cmd`, `.bat` or `.ps1` extension ignored, case-insensitive).
    pub fn for_command(command: &str) -> Option<Self> {
        let file = command
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(command)
            .to_ascii_lowercase();
        let stem = [".exe", ".cmd", ".bat", ".ps1"]
            .iter()
            .find_map(|ext| file.strip_suffix(ext))
            .unwrap_or(&file);
        match stem {
            "node" | "npx" | "npm" | "pnpm" | "pnpx" | "yarn" | "corepack" => Some(Self::NodeJs),
            "uv" | "uvx" | "python" | "python3" | "py" | "pip" | "pip3" | "pipx" => {
                Some(Self::UvPython)
            }
            "docker" => Some(Self::Docker),
            _ => None,
        }
    }

    /// The name shown to the user.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::NodeJs => "Node.js",
            Self::UvPython => "uv (Python)",
            Self::Docker => "Docker",
        }
    }
}

impl fmt::Display for MissingRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.display_name())
    }
}

/// Why a server could not be added, started or used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HostError {
    /// A server with this name is already registered.
    #[error("a server named \"{0}\" already exists")]
    AlreadyExists(String),
    /// No server with this name.
    #[error("no server named \"{0}\"")]
    NotFound(String),
    /// The name is empty, too long or has control characters.
    #[error("\"{0}\" is not a valid server name")]
    InvalidName(String),
    /// [`crate::Host::shutdown`] has run.
    #[error("the app is shutting down")]
    ShuttingDown,
    /// The command belongs to a runtime that is not installed.
    #[error("this server needs {runtime}, which isn't installed (\"{command}\" was not found)")]
    MissingRuntime {
        /// The runtime to install.
        runtime: MissingRuntime,
        /// The command from the config.
        command: String,
    },
    /// macOS: the command is one of Apple's stubs that opens the developer-tools installer.
    #[error(
        "this server needs Apple's command line developer tools, which aren't installed (running \"{command}\" would only open Apple's installer)"
    )]
    DeveloperToolsMissing {
        /// The stub that was not run.
        command: String,
    },
    /// The command was not found and is not a known runtime.
    #[error("the program \"{command}\" was not found")]
    CommandNotFound {
        /// The command from the config.
        command: String,
    },
    /// Windows: an argument cannot be passed to a `.cmd` shim safely.
    #[error("{0}")]
    UnsafeArgument(#[from] QuoteError),
    /// A secret placeholder could not be resolved.
    #[error("{0}")]
    Secret(#[from] SecretError),
    /// The config is unusable (relative working folder, bad header…).
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    /// The process could not be started.
    #[error("could not start the server: {0}")]
    Spawn(String),
    /// The server started but the connection or handshake failed. The message includes
    /// the last lines the server wrote to stderr, when there are any.
    #[error("{0}")]
    Connect(String),
    /// The server demands an OAuth sign-in, which v1 does not support (D16).
    #[error(
        "this server requires signing in (OAuth), which this app does not support yet; use a URL with a static token in a header instead"
    )]
    AuthRequired,
    /// A `type: "sse"` entry that also failed as Streamable HTTP.
    #[error(
        "this server uses the legacy SSE transport, which is not supported; use the mcp-remote bridge"
    )]
    LegacySseUnsupported,
    /// The shared HTTP client could not be built.
    #[error("could not set up the HTTP client: {0}")]
    Http(String),
    /// The server exists but is not connected.
    #[error("the server is not available: {0}")]
    NotReady(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_runtime_commands() {
        assert_eq!(
            MissingRuntime::for_command("npx"),
            Some(MissingRuntime::NodeJs)
        );
        assert_eq!(
            MissingRuntime::for_command(r"C:\x\NPX.CMD"),
            Some(MissingRuntime::NodeJs)
        );
        assert_eq!(
            MissingRuntime::for_command("/usr/bin/node"),
            Some(MissingRuntime::NodeJs)
        );
        assert_eq!(
            MissingRuntime::for_command("uvx"),
            Some(MissingRuntime::UvPython)
        );
        assert_eq!(
            MissingRuntime::for_command("python3"),
            Some(MissingRuntime::UvPython)
        );
        assert_eq!(
            MissingRuntime::for_command("uv.exe"),
            Some(MissingRuntime::UvPython)
        );
        assert_eq!(
            MissingRuntime::for_command("docker"),
            Some(MissingRuntime::Docker)
        );
        assert_eq!(MissingRuntime::for_command("my-server"), None);
    }

    #[test]
    fn messages_name_the_runtime() {
        let e = HostError::MissingRuntime {
            runtime: MissingRuntime::NodeJs,
            command: "npx".into(),
        };
        assert_eq!(
            e.to_string(),
            "this server needs Node.js, which isn't installed (\"npx\" was not found)"
        );
    }
}
