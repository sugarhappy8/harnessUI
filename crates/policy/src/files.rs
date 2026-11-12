//! Which file writes always ask, whatever was granted (ARCHITECTURE.md 13.3):
//! deletes; replacing a file with an empty one; anything under a `.git` folder; shell
//! start-up files; git and ssh configuration; autostart locations; the settings of the
//! coding apps Claude Code and Codex (0.0.6: `.claude/`, `.codex/`, `.mcp.json`,
//! `.claude.json`).

use crate::engine::KeepAsking;
use crate::path::Flavor;

/// Shell and PowerShell start-up files, by file name.
const STARTUP_FILES: &[&str] = &[
    ".bashrc",
    ".bash_profile",
    ".bash_login",
    ".bash_logout",
    ".profile",
    ".zshrc",
    ".zshenv",
    ".zprofile",
    ".zlogin",
    ".zlogout",
    ".cshrc",
    ".tcshrc",
    ".kshrc",
    ".inputrc",
    "config.fish",
    "microsoft.powershell_profile.ps1",
    "profile.ps1",
    "microsoft.vscode_profile.ps1",
];

/// Git and ssh configuration, by file name.
const CONFIG_FILES: &[&str] = &[
    ".gitconfig",
    ".git-credentials",
    ".gitattributes_global",
    ".netrc",
    "_netrc",
];

/// Folder names whose contents are always sensitive.
const SENSITIVE_DIRS: &[&str] = &[".git", ".ssh", ".gnupg", "launchagents", "launchdaemons"];

/// The settings folders of the coding apps the app can hand tasks to (0.0.6): a project's
/// and the user's `.claude` and `.codex`. Hooks and MCP servers configured there run
/// programs, and their permission settings decide what the app may do on its own.
pub(crate) const CODING_APP_DIRS: &[&str] = &[".claude", ".codex"];

/// The settings files of those apps, by file name: a project's MCP servers for Claude Code,
/// and Claude Code's own `~/.claude.json`.
const CODING_APP_FILES: &[&str] = &[".mcp.json", ".claude.json"];

/// What kind of write is planned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteKind {
    Create,
    Overwrite { existing_len: u64, new_len: u64 },
    Edit,
    Delete,
}

/// Why a write always asks, or `None` if a grant may cover it.
pub fn always_ask_reason(canonical: &str, kind: WriteKind, flavor: Flavor) -> Option<&'static str> {
    if kind == WriteKind::Delete {
        return Some("deleting a file always asks");
    }
    if let WriteKind::Overwrite {
        existing_len,
        new_len,
    } = kind
        && existing_len > 0
        && new_len == 0
    {
        return Some("replacing a file with an empty one always asks");
    }
    let lower = canonical.to_lowercase();
    let parts: Vec<&str> = match flavor {
        Flavor::Windows => lower.split(['\\', '/']).collect(),
        Flavor::Posix => lower.split('/').collect(),
    };
    let (name, dirs) = parts.split_last()?;
    if dirs.contains(&".git") || *name == ".git" {
        return Some("writing inside a .git folder always asks");
    }
    if dirs.iter().any(|d| SENSITIVE_DIRS.contains(d)) {
        return Some("writing to ssh, gpg or autostart folders always asks");
    }
    if dirs.iter().any(|d| CODING_APP_DIRS.contains(d))
        || CODING_APP_DIRS.contains(name)
        || CODING_APP_FILES.contains(name)
    {
        return Some("changing the settings of Claude Code or Codex always asks");
    }
    if STARTUP_FILES.contains(name) {
        return Some("changing a shell start-up file always asks");
    }
    if CONFIG_FILES.contains(name) {
        return Some("changing git or credential configuration always asks");
    }
    // .config/git/config and .config/fish/... are covered by name above or here.
    let joined = parts.join("/");
    if joined.contains("/.config/git/")
        || joined.contains("/.config/fish/")
        || joined.contains("/.config/autostart/")
    {
        return Some("changing git, shell or autostart configuration always asks");
    }
    // Windows Startup folders.
    if joined.contains("/start menu/programs/startup/")
        || joined.ends_with("/start menu/programs/startup")
    {
        return Some("writing to the Windows Startup folder always asks");
    }
    // PowerShell profile folders.
    if (joined.contains("/documents/windowspowershell/")
        || joined.contains("/documents/powershell/"))
        && name.ends_with(".ps1")
    {
        return Some("changing a PowerShell profile always asks");
    }
    None
}

/// Allow all (0.0.4): why a write still asks while its chat is in Allow all, if it does.
/// Every write that always asks in Ask mode keeps asking: deleting a file and leaving one
/// empty count as deleting, the sensitive locations as sensitive.
pub fn keep_asking(canonical: &str, kind: WriteKind, flavor: Flavor) -> Option<KeepAsking> {
    always_ask_reason(canonical, kind, flavor)?;
    let emptied = matches!(
        kind,
        WriteKind::Overwrite {
            existing_len,
            new_len: 0,
        } if existing_len > 0
    );
    Some(if kind == WriteKind::Delete || emptied {
        KeepAsking::Delete
    } else {
        KeepAsking::Sensitive
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: Flavor = Flavor::Posix;
    const W: Flavor = Flavor::Windows;

    #[test]
    fn plain_writes_can_be_granted() {
        assert_eq!(
            always_ask_reason("/home/u/proj/src/main.rs", WriteKind::Create, P),
            None
        );
        assert_eq!(
            always_ask_reason(
                "/home/u/proj/a.txt",
                WriteKind::Overwrite {
                    existing_len: 10,
                    new_len: 5
                },
                P
            ),
            None
        );
        assert_eq!(
            always_ask_reason("/home/u/proj/.gitignore", WriteKind::Edit, P),
            None
        );
        assert_eq!(
            always_ask_reason("/home/u/proj/github/x", WriteKind::Edit, P),
            None
        );
        assert_eq!(
            always_ask_reason("C:\\Users\\me\\proj\\x.txt", WriteKind::Create, W),
            None
        );
    }

    #[test]
    fn deletes_and_emptying_always_ask() {
        assert!(always_ask_reason("/p/a.txt", WriteKind::Delete, P).is_some());
        assert!(
            always_ask_reason(
                "/p/a.txt",
                WriteKind::Overwrite {
                    existing_len: 1,
                    new_len: 0
                },
                P
            )
            .is_some()
        );
        assert_eq!(
            always_ask_reason(
                "/p/a.txt",
                WriteKind::Overwrite {
                    existing_len: 0,
                    new_len: 0
                },
                P
            ),
            None
        );
    }

    #[test]
    fn sensitive_locations_always_ask() {
        for p in [
            "/p/.git/config",
            "/p/.git/hooks/pre-commit",
            "/home/u/.bashrc",
            "/home/u/.zshrc",
            "/home/u/.profile",
            "/home/u/.config/fish/config.fish",
            "/home/u/.gitconfig",
            "/home/u/.config/git/config",
            "/home/u/.ssh/config",
            "/home/u/.ssh/authorized_keys",
            "/Users/u/Library/LaunchAgents/com.evil.plist",
            "/Library/LaunchDaemons/x.plist",
            "/home/u/.config/autostart/x.desktop",
        ] {
            assert!(always_ask_reason(p, WriteKind::Edit, P).is_some(), "{p}");
        }
        for p in [
            "C:\\Users\\me\\proj\\.git\\HEAD",
            "C:\\Users\\me\\.gitconfig",
            "C:\\Users\\me\\.ssh\\id_ed25519",
            "C:\\Users\\me\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs\\Startup\\x.lnk",
            "C:\\Users\\me\\Documents\\WindowsPowerShell\\Microsoft.PowerShell_profile.ps1",
            "C:\\Users\\me\\Documents\\PowerShell\\profile.ps1",
            "C:\\Users\\me\\Documents\\PowerShell\\other.ps1",
            "C:\\Users\\me\\_netrc",
        ] {
            assert!(always_ask_reason(p, WriteKind::Create, W).is_some(), "{p}");
        }
    }

    /// Allow all: every write that always asks keeps asking, as a delete (deleting,
    /// emptying) or as a sensitive place; no other write does.
    #[test]
    fn what_always_asks_keeps_asking_in_allow_all() {
        let emptied = WriteKind::Overwrite {
            existing_len: 9,
            new_len: 0,
        };
        let shorter = WriteKind::Overwrite {
            existing_len: 9,
            new_len: 1,
        };
        assert_eq!(
            keep_asking("/p/a.txt", WriteKind::Delete, P),
            Some(KeepAsking::Delete)
        );
        assert_eq!(
            keep_asking("/p/a.txt", emptied, P),
            Some(KeepAsking::Delete)
        );
        // Deleting inside a sensitive place is still a delete.
        assert_eq!(
            keep_asking("/p/.git/index", WriteKind::Delete, P),
            Some(KeepAsking::Delete)
        );
        for path in ["/p/.git/config", "/home/u/.bashrc", "/home/u/.ssh/config"] {
            for kind in [WriteKind::Create, WriteKind::Edit, shorter] {
                assert_eq!(keep_asking(path, kind, P), Some(KeepAsking::Sensitive));
            }
        }
        assert_eq!(
            keep_asking("C:\\P\\.GIT\\config", WriteKind::Edit, W),
            Some(KeepAsking::Sensitive)
        );
        for kind in [WriteKind::Create, WriteKind::Edit, shorter] {
            assert_eq!(keep_asking("/p/src/main.rs", kind, P), None);
            assert_eq!(keep_asking("C:\\Users\\me\\proj\\x.txt", kind, W), None);
        }
    }

    /// 0.0.6: the settings of the coding apps the app can hand tasks to (Claude Code,
    /// Codex) keep asking like `.git`: a project's `.claude/` folder and `.mcp.json`, the
    /// user's `~/.claude/`, `~/.claude.json` and `~/.codex/`. A file there could start a
    /// program the next time the app runs, or turn off its permission checks.
    #[test]
    fn coding_app_settings_always_ask() {
        for p in [
            "/home/u/proj/.claude/settings.json",
            "/home/u/proj/.claude/settings.local.json",
            "/home/u/proj/.claude/hooks/pre.sh",
            "/home/u/proj/.claude",
            "/home/u/proj/.mcp.json",
            "/home/u/proj/sub/.mcp.json",
            "/home/u/proj/.codex/config.toml",
            "/home/u/proj/.codex",
            "/home/u/.claude/settings.json",
            "/home/u/.claude.json",
            "/home/u/.codex/config.toml",
            "/home/u/.codex/hooks.json",
            "/Users/u/.CLAUDE/Settings.json",
        ] {
            assert!(always_ask_reason(p, WriteKind::Create, P).is_some(), "{p}");
            assert_eq!(
                keep_asking(p, WriteKind::Edit, P),
                Some(KeepAsking::Sensitive),
                "{p}"
            );
        }
        for p in [
            "C:\\Users\\me\\proj\\.claude\\settings.json",
            "C:\\Users\\me\\.claude.json",
            "C:\\Users\\me\\.codex\\config.toml",
            "C:\\Users\\me\\proj\\.mcp.json",
        ] {
            assert!(always_ask_reason(p, WriteKind::Edit, W).is_some(), "{p}");
        }
        // Names that only look alike stay grantable.
        for p in [
            "/home/u/proj/claude/notes.md",
            "/home/u/proj/.claudeignore",
            "/home/u/proj/docs/claude.json",
            "/home/u/proj/mcp.json",
            "/home/u/proj/.mcp.json.example",
            "/home/u/proj/codex/src/main.rs",
            "/home/u/proj/.codexrc-notes/x",
            "/home/u/proj/CLAUDE.md",
        ] {
            assert_eq!(always_ask_reason(p, WriteKind::Create, P), None, "{p}");
        }
    }

    #[test]
    fn case_does_not_hide_sensitive_names() {
        assert!(always_ask_reason("C:\\P\\.GIT\\config", WriteKind::Edit, W).is_some());
        assert!(always_ask_reason("/Users/u/.ZSHRC", WriteKind::Edit, P).is_some());
    }
}
