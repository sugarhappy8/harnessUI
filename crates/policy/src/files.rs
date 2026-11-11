//! Which file writes always ask, whatever was granted (ARCHITECTURE.md 13.3):
//! deletes; replacing a file with an empty one; anything under a `.git` folder; shell
//! start-up files; git and ssh configuration; autostart locations.

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
    let _ = (canonical, kind, flavor);
    None
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

    #[test]
    fn case_does_not_hide_sensitive_names() {
        assert!(always_ask_reason("C:\\P\\.GIT\\config", WriteKind::Edit, W).is_some());
        assert!(always_ask_reason("/Users/u/.ZSHRC", WriteKind::Edit, P).is_some());
    }
}
