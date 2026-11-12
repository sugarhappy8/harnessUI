//! Which application input goes to, and which applications always ask (D7, 13.3).
//!
//! A pointer or keyboard grant is bound to one application, identified by
//! [`AppInfo::id`]: the full executable path (lower-case) on Windows, the bundle id on
//! macOS (the executable path when an app has none). The path, not the bare file name, so
//! that a program named `notepad.exe` somewhere else does not inherit Notepad's grant.
//!
//! Terminals and shell hosts, system settings and password managers always ask, whatever
//! was granted: keyboard access to a terminal is a shell, and the other two hold the keys
//! to everything else. So do console windows whatever program runs in them, remote-desktop
//! windows, applications the platform cannot identify, and typing into the Windows shell
//! (`explorer.exe`'s Run box and address bar, Start's search box), where typed text runs
//! as a command ([`target_always_ask_reason`]).

use serde::{Deserialize, Serialize};

use crate::geometry::Rect;

/// An application, as the platform identifies it. All strings come from the OS or the
/// application itself and are untrusted: they reach cards only through
/// `policy::sanitize`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    /// The grant key (see the module docs).
    pub id: String,
    /// Display name ("Notepad", "Safari").
    pub name: String,
    /// Executable file name, lower-case (`notepad.exe`, `safari`).
    pub exe_name: String,
    /// macOS bundle identifier.
    pub bundle_id: Option<String>,
}

impl AppInfo {
    /// Windows: from the full image path of the process.
    pub fn from_windows_path(path: &str) -> Self {
        let exe = path
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or(path)
            .to_ascii_lowercase();
        let name = exe.strip_suffix(".exe").unwrap_or(&exe).to_string();
        Self {
            id: path.to_lowercase(),
            name,
            exe_name: exe,
            bundle_id: None,
        }
    }

    /// macOS: bundle id when there is one, else the executable path.
    pub fn from_macos(bundle_id: Option<&str>, name: &str, executable: Option<&str>) -> Self {
        let exe_name = executable
            .and_then(|p| p.rsplit('/').next())
            .unwrap_or(name)
            .to_lowercase();
        let id = match (bundle_id, executable) {
            (Some(b), _) if !b.is_empty() => b.to_string(),
            (_, Some(p)) if !p.is_empty() => p.to_string(),
            _ => format!("name:{name}"),
        };
        Self {
            id,
            name: name.to_string(),
            exe_name,
            bundle_id: bundle_id.filter(|b| !b.is_empty()).map(str::to_string),
        }
    }

    /// The name and id for a card, sanitised.
    pub fn display(&self) -> String {
        let name = policy::sanitize::one_line(&self.name, 60);
        let id = policy::sanitize::one_line(&self.id, 160);
        if name.is_empty() || name.eq_ignore_ascii_case(&id) {
            id
        } else {
            format!("{name} ({id})")
        }
    }
}

/// The window an event would reach, as the platform found it just before the event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    /// Every process owning a window in the chain that would receive the event: the
    /// window itself, its top-level window and that window's owner. A WebView2 child
    /// window belongs to a browser process, but its top-level window is this app's.
    pub pids: Vec<u32>,
    /// The application the input goes to (the grant is checked against it).
    pub app: AppInfo,
    /// Windows: the target process runs elevated and we do not, so input would be dropped
    /// silently (UIPI). Always false on macOS.
    pub elevated: bool,
    /// The window's title (untrusted).
    pub window_title: Option<String>,
    /// Platform window id (`HWND` value or `CGWindowID`), for logs.
    pub window_id: u64,
    /// The window's frame in global input coordinates, when known.
    pub window_rect: Option<Rect>,
    /// The window is a console or terminal window, whatever process owns it (Windows: its
    /// window class). Windows reports a console window as owned by the program running in
    /// it (`python.exe`, `ssh.exe`, …), so the executable alone does not show it is a
    /// shell.
    #[serde(default)]
    pub console: bool,
}

/// Window classes of console and terminal windows (Windows): the classic console host,
/// Windows Terminal, the pseudo-console, ConEmu, mintty and PuTTY.
pub fn is_console_window_class(class: &str) -> bool {
    const CLASSES: &[&str] = &[
        "consolewindowclass",
        "cascadia_hosting_window_class",
        "pseudoconsolewindow",
        "virtualconsoleclass",
        "mintty",
        "putty",
    ];
    let c = class.to_ascii_lowercase();
    CLASSES.contains(&c.as_str())
}

/// Why input of this kind to this window always asks, if it does: the application's own
/// reason ([`always_ask_reason`]), a console window, or, for keyboard input, a Windows
/// shell surface where typed text runs as a command (the Run box and File Explorer's
/// address bar in `explorer.exe`, Start's search box).
pub fn target_always_ask_reason(target: &Target, keyboard: bool) -> Option<&'static str> {
    const CONSOLE: &str =
        "it is a console or terminal window: keyboard and pointer access to it can run any command";
    const COMMAND_ENTRY: &str = "it is part of the Windows shell, where typed text runs as a command (the Run box, File Explorer's address bar, Start's search)";
    // Windows executables (file stem, lower-case) whose text fields run what is typed.
    const WIN_COMMAND_ENTRY: &[&str] = &[
        "explorer",
        "searchhost",
        "searchapp",
        "searchui",
        "startmenuexperiencehost",
    ];
    if target.console {
        return Some(CONSOLE);
    }
    if let Some(why) = always_ask_reason(&target.app) {
        return Some(why);
    }
    let exe = target.app.exe_name.as_str();
    let stem = exe.strip_suffix(".exe").unwrap_or(exe);
    if keyboard && target.app.bundle_id.is_none() && WIN_COMMAND_ENTRY.contains(&stem) {
        return Some(COMMAND_ENTRY);
    }
    None
}

/// Whether the application is a file manager, where the Delete key deletes the selected
/// files (and Command-Backspace on a Mac moves them to the Trash).
pub fn is_file_manager(app: &AppInfo) -> bool {
    let exe = app.exe_name.to_ascii_lowercase();
    let stem = exe.strip_suffix(".exe").unwrap_or(&exe);
    let bundle = app.bundle_id.as_deref().unwrap_or("").to_ascii_lowercase();
    bundle == "com.apple.finder"
        || matches!(
            stem,
            "explorer"
                | "finder"
                | "nautilus"
                | "org.gnome.nautilus"
                | "dolphin"
                | "thunar"
                | "nemo"
                | "pcmanfm"
                | "pcmanfm-qt"
                | "caja"
                | "totalcmd"
                | "totalcmd64"
                | "doublecmd"
        )
}

/// Whether the application is a web browser: what is typed there can go to any site.
pub fn is_browser(app: &AppInfo) -> bool {
    const EXES: &[&str] = &[
        "chrome",
        "google-chrome",
        "chromium",
        "chromium-browser",
        "msedge",
        "microsoft-edge",
        "firefox",
        "brave",
        "brave-browser",
        "opera",
        "vivaldi",
        "safari",
        "arc",
        "iexplore",
        "librewolf",
        "waterfox",
        "zen",
    ];
    const BUNDLES: &[&str] = &[
        "com.google.chrome",
        "org.chromium.chromium",
        "com.microsoft.edgemac",
        "org.mozilla.firefox",
        "com.apple.safari",
        "com.brave.browser",
        "com.operasoftware.opera",
        "com.vivaldi.vivaldi",
        "company.thebrowser.browser",
    ];
    let exe = app.exe_name.to_ascii_lowercase();
    let stem = exe.strip_suffix(".exe").unwrap_or(&exe);
    let bundle = app.bundle_id.as_deref().unwrap_or("").to_ascii_lowercase();
    EXES.contains(&stem) || BUNDLES.iter().any(|b| bundle.starts_with(b))
}

/// Why an application always asks, if it does.
pub fn always_ask_reason(app: &AppInfo) -> Option<&'static str> {
    let exe = app.exe_name.as_str();
    let exe_stem = exe.strip_suffix(".exe").unwrap_or(exe);
    let bundle = app.bundle_id.as_deref().unwrap_or("").to_ascii_lowercase();
    let name = app.name.to_ascii_lowercase();

    const TERMINAL: &str =
        "it is a terminal or shell host: keyboard and pointer access to it can run any command";
    const SETTINGS: &str =
        "it is a system settings or administration tool: it can change how the computer works";
    const PASSWORDS: &str = "it is a password manager or credential prompt";
    const UNRESOLVED_HOST: &str =
        "it is a host window for a Windows app that could not be identified (it may be Settings)";
    const UNIDENTIFIED: &str = "its application could not be identified, so it may be a terminal, settings or a password manager";
    const REMOTE: &str = "it is a remote-desktop or virtual-machine window: input reaches another computer's terminals and settings";

    // Windows executables (file stem, lower-case).
    const WIN_TERMINALS: &[&str] = &[
        "cmd",
        "powershell",
        "powershell_ise",
        "pwsh",
        "windowsterminal",
        "wt",
        "openconsole",
        "conhost",
        "wezterm",
        "wezterm-gui",
        "alacritty",
        "kitty",
        "mintty",
        "bash",
        "wsl",
        "wslhost",
        "wslg",
        "msrdc",
        "ubuntu",
        "debian",
        "tabby",
        "hyper",
        "conemu",
        "conemu64",
        "cmder",
        "putty",
        "kitty_portable",
        "fluentterminal.app",
        "warp",
        "ghostty",
        "terminus",
        "mobaxterm",
        "securecrt",
        "termius",
    ];
    const WIN_SETTINGS: &[&str] = &[
        "systemsettings",
        "control",
        "mmc",
        "regedit",
        "regedt32",
        "taskmgr",
        "msconfig",
        "gpedit",
        "secpol",
        "services",
        "compmgmt",
        "devmgmt",
        "diskmgmt",
        "eventvwr",
        "lusrmgr",
        "netplwiz",
        "useraccountcontrolsettings",
        "systempropertiesadvanced",
        "systempropertiesprotection",
        "optionalfeatures",
        "rundll32",
        "windowsdefender",
        "securityhealthui",
        "msiexec",
        "sysdm",
        "wf",
    ];
    const WIN_PASSWORDS: &[&str] = &[
        "1password",
        "bitwarden",
        "keepass",
        "keepassxc",
        "keepassxc-proxy",
        "dashlane",
        "lastpass",
        "enpass",
        "roboform",
        "nordpass",
        "keeper",
        "keeperpasswordmanager",
        "proton pass",
        "protonpass",
        "credentialuibroker",
        "consent",
        "logonui",
        "credwiz",
    ];
    // macOS bundle identifiers (lower-case); a trailing '.' matches a prefix.
    const MAC_TERMINALS: &[&str] = &[
        "com.apple.terminal",
        "com.googlecode.iterm2",
        "com.github.wez.wezterm",
        "org.alacritty",
        "io.alacritty",
        "net.kovidgoyal.kitty",
        "dev.warp.",
        "co.zeit.hyper",
        "com.mitchellh.ghostty",
        "org.tabby",
        "com.panic.prompt",
        "com.termius-dmg.mac",
        "com.apple.scripteditor2",
        "com.apple.automator",
        "com.apple.shortcuts",
        "com.microsoft.vscode.terminal",
    ];
    const MAC_SETTINGS: &[&str] = &[
        "com.apple.systempreferences",
        "com.apple.systemsettings",
        "com.apple.settings.",
        "com.apple.activitymonitor",
        "com.apple.diskutility",
        "com.apple.console",
        "com.apple.directoryutility",
        "com.apple.installer",
        "com.apple.migrateassistant",
        "com.apple.bootcampassistant",
        "com.apple.securityagent",
        "com.apple.loginwindow",
        "com.apple.coreservices.uiagent",
    ];
    const MAC_PASSWORDS: &[&str] = &[
        "com.apple.keychainaccess",
        "com.apple.passwords",
        "com.agilebits.",
        "com.1password.",
        "com.bitwarden.",
        "org.keepassxc.",
        "com.dashlane.",
        "com.lastpass.",
        "in.sinew.enpass",
        "com.nordpass.",
        "com.callpod.keepermac",
        "me.proton.pass",
        "com.roboform.",
    ];
    // Display-name fragments that catch the rest (renamed binaries, portable builds).
    const NAME_PASSWORDS: &[&str] = &[
        "1password",
        "bitwarden",
        "keepass",
        "lastpass",
        "dashlane",
        "keychain",
        "password",
        "enpass",
        "nordpass",
        "roboform",
    ];
    const NAME_TERMINALS: &[&str] = &[
        "terminal",
        "powershell",
        "iterm",
        "command prompt",
        "wezterm",
        "alacritty",
        "kitty",
        "ghostty",
    ];

    // Remote-desktop and virtual-machine consoles forward input to another computer.
    const WIN_REMOTE: &[&str] = &["mstsc", "vmconnect", "vncviewer", "tvnviewer"];
    const MAC_REMOTE: &[&str] = &[
        "com.apple.screensharing",
        "com.microsoft.rdc.macos",
        "com.microsoft.rdc.mac",
    ];

    let bundle_match = |list: &[&str]| -> bool {
        list.iter().any(|b| match b.strip_suffix('.') {
            Some(prefix) => bundle.starts_with(&format!("{prefix}.")),
            None => bundle == *b,
        })
    };

    // The platform could not tell which application this is (`pid:` on Windows when the
    // process cannot be opened, `name:` on macOS without a bundle or executable): fail
    // closed rather than let it slip past the lists below.
    if app.id.starts_with("pid:") || app.id.starts_with("name:") || app.id.is_empty() {
        return Some(UNIDENTIFIED);
    }
    if exe_stem == "applicationframehost" {
        return Some(UNRESOLVED_HOST);
    }
    if WIN_REMOTE.contains(&exe_stem) || bundle_match(MAC_REMOTE) {
        return Some(REMOTE);
    }
    if WIN_TERMINALS.contains(&exe_stem) || bundle_match(MAC_TERMINALS) {
        return Some(TERMINAL);
    }
    if WIN_SETTINGS.contains(&exe_stem) || bundle_match(MAC_SETTINGS) {
        return Some(SETTINGS);
    }
    if WIN_PASSWORDS.contains(&exe_stem)
        || exe_stem.starts_with("keepass")
        || bundle_match(MAC_PASSWORDS)
    {
        return Some(PASSWORDS);
    }
    if NAME_PASSWORDS
        .iter()
        .any(|n| name.contains(n) || exe_stem.contains(n))
    {
        return Some(PASSWORDS);
    }
    if NAME_TERMINALS.iter().any(|n| name.contains(n)) {
        return Some(TERMINAL);
    }
    if name == "system settings" || name == "system preferences" || name == "settings" {
        return Some(SETTINGS);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(path: &str) -> AppInfo {
        AppInfo::from_windows_path(path)
    }
    fn mac(bundle: &str, name: &str) -> AppInfo {
        AppInfo::from_macos(
            Some(bundle),
            name,
            Some(&format!("/Applications/{name}.app/Contents/MacOS/{name}")),
        )
    }

    #[test]
    fn windows_identity_is_the_full_path() {
        let a = win(r"C:\Windows\System32\notepad.exe");
        assert_eq!(a.id, r"c:\windows\system32\notepad.exe");
        assert_eq!(a.exe_name, "notepad.exe");
        assert_eq!(a.name, "notepad");
        // A look-alike elsewhere is a different application.
        let b = win(r"C:\Users\x\Downloads\notepad.exe");
        assert_ne!(a.id, b.id);
        assert_eq!(a.display(), r"notepad (c:\windows\system32\notepad.exe)");
    }

    #[test]
    fn macos_identity_is_the_bundle_id() {
        let a = mac("com.apple.TextEdit", "TextEdit");
        assert_eq!(a.id, "com.apple.TextEdit");
        assert_eq!(a.exe_name, "textedit");
        let b = AppInfo::from_macos(None, "tool", Some("/usr/local/bin/tool"));
        assert_eq!(b.id, "/usr/local/bin/tool");
        let c = AppInfo::from_macos(None, "Mystery", None);
        assert_eq!(c.id, "name:Mystery");
    }

    #[test]
    fn terminals_and_shell_hosts_always_ask() {
        for p in [
            r"C:\Windows\System32\cmd.exe",
            r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
            r"C:\Program Files\PowerShell\7\pwsh.exe",
            r"C:\Program Files\WindowsApps\Microsoft.WindowsTerminal_1.21\WindowsTerminal.exe",
            r"C:\Windows\System32\conhost.exe",
            r"C:\Program Files\WezTerm\wezterm-gui.exe",
            r"C:\tools\alacritty.exe",
            r"C:\Program Files\Git\usr\bin\mintty.exe",
        ] {
            assert!(always_ask_reason(&win(p)).is_some(), "{p}");
        }
        for (b, n) in [
            ("com.apple.Terminal", "Terminal"),
            ("com.googlecode.iterm2", "iTerm2"),
            ("com.github.wez.wezterm", "WezTerm"),
            ("org.alacritty", "Alacritty"),
            ("net.kovidgoyal.kitty", "kitty"),
            ("dev.warp.Warp-Stable", "Warp"),
            ("com.apple.ScriptEditor2", "Script Editor"),
        ] {
            assert!(always_ask_reason(&mac(b, n)).is_some(), "{b}");
        }
    }

    #[test]
    fn settings_and_password_managers_always_ask() {
        for p in [
            r"C:\Windows\ImmersiveControlPanel\SystemSettings.exe",
            r"C:\Windows\System32\control.exe",
            r"C:\Windows\regedit.exe",
            r"C:\Windows\System32\mmc.exe",
            r"C:\Users\x\AppData\Local\1Password\app\8\1Password.exe",
            r"C:\Program Files\Bitwarden\Bitwarden.exe",
            r"C:\Program Files\KeePass Password Safe 2\KeePass.exe",
            r"C:\Program Files\KeePassXC\KeePassXC.exe",
            r"C:\Windows\System32\CredentialUIBroker.exe",
            r"C:\Windows\System32\ApplicationFrameHost.exe",
        ] {
            assert!(always_ask_reason(&win(p)).is_some(), "{p}");
        }
        for (b, n) in [
            ("com.apple.systempreferences", "System Settings"),
            ("com.apple.keychainaccess", "Keychain Access"),
            ("com.apple.Passwords", "Passwords"),
            ("com.1password.1password", "1Password"),
            ("com.agilebits.onepassword7", "1Password 7"),
            ("com.bitwarden.desktop", "Bitwarden"),
            ("org.keepassxc.keepassxc", "KeePassXC"),
            ("com.apple.ActivityMonitor", "Activity Monitor"),
        ] {
            assert!(always_ask_reason(&mac(b, n)).is_some(), "{b}");
        }
        // A renamed password manager is still caught by its name.
        let renamed = AppInfo::from_macos(Some("com.example.vault"), "KeePassium", None);
        assert!(always_ask_reason(&renamed).is_some());
    }

    fn target(app: AppInfo, console: bool) -> Target {
        Target {
            pids: vec![42],
            app,
            elevated: false,
            window_title: None,
            window_id: 1,
            window_rect: None,
            console,
        }
    }

    #[test]
    fn console_windows_always_ask_whatever_program_runs_in_them() {
        // Windows reports a console window as owned by the program in it.
        let python = win(r"C:\Python312\python.exe");
        assert_eq!(always_ask_reason(&python), None);
        assert!(target_always_ask_reason(&target(python.clone(), true), false).is_some());
        assert!(target_always_ask_reason(&target(python.clone(), true), true).is_some());
        assert_eq!(target_always_ask_reason(&target(python, false), true), None);
        for class in [
            "ConsoleWindowClass",
            "CASCADIA_HOSTING_WINDOW_CLASS",
            "PseudoConsoleWindow",
            "mintty",
            "PuTTY",
        ] {
            assert!(is_console_window_class(class), "{class}");
        }
        for class in ["Notepad", "Chrome_WidgetWin_1", "CabinetWClass", ""] {
            assert!(!is_console_window_class(class), "{class}");
        }
    }

    #[test]
    fn typing_into_the_windows_shell_always_asks() {
        // The Run box and File Explorer's address bar run what is typed.
        for p in [
            r"C:\Windows\explorer.exe",
            r"C:\Windows\SystemApps\MicrosoftWindows.Client.CBS_cw5n1h2txyewy\SearchHost.exe",
            r"C:\Windows\SystemApps\Microsoft.Windows.StartMenuExperienceHost_cw5n1h2txyewy\StartMenuExperienceHost.exe",
        ] {
            let t = target(win(p), false);
            assert!(target_always_ask_reason(&t, true).is_some(), "{p}");
            // Clicking the desktop, taskbar or a folder can still be granted.
            assert_eq!(target_always_ask_reason(&t, false), None, "{p}");
        }
        // Not a macOS rule (a bundle named "explorer" is just an app).
        let mac_app = AppInfo::from_macos(Some("com.example.explorer"), "explorer", None);
        assert_eq!(
            target_always_ask_reason(&target(mac_app, false), true),
            None
        );
    }

    #[test]
    fn unidentified_and_remote_applications_always_ask() {
        let unknown = AppInfo {
            id: "pid:4242".into(),
            name: "process 4242".into(),
            exe_name: String::new(),
            bundle_id: None,
        };
        assert!(always_ask_reason(&unknown).is_some());
        assert!(always_ask_reason(&AppInfo::from_macos(None, "helper", None)).is_some());
        for p in [
            r"C:\Windows\System32\mstsc.exe",
            r"C:\Windows\System32\vmconnect.exe",
        ] {
            assert!(always_ask_reason(&win(p)).is_some(), "{p}");
        }
        assert!(always_ask_reason(&mac("com.apple.ScreenSharing", "Screen Sharing")).is_some());
    }

    #[test]
    fn ordinary_applications_do_not() {
        for p in [
            r"C:\Windows\System32\notepad.exe",
            r"C:\Program Files\Mozilla Firefox\firefox.exe",
            r"C:\Program Files\Microsoft Office\root\Office16\WINWORD.EXE",
            r"C:\Windows\explorer.exe",
        ] {
            assert_eq!(always_ask_reason(&win(p)), None, "{p}");
        }
        for (b, n) in [
            ("com.apple.TextEdit", "TextEdit"),
            ("com.apple.Safari", "Safari"),
            ("com.apple.finder", "Finder"),
            ("org.mozilla.firefox", "Firefox"),
        ] {
            assert_eq!(always_ask_reason(&mac(b, n)), None, "{b}");
        }
        // Prefix entries need a dot boundary.
        assert_eq!(always_ask_reason(&mac("com.agilebitsx.app", "Other")), None);
    }
}
