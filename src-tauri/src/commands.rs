//! App commands callable from the main window.
//!
//! Every command here must also be listed in `commands.list`, and granted in
//! `capabilities/main.json` as `allow-<name-with-hyphens>`. A test checks both.

use serde::Serialize;

/// What the About box and the window title show.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub name: String,
    pub version: String,
    pub identifier: String,
    pub os: &'static str,
    pub arch: &'static str,
    /// The start of the commit this build was made from, so that two builds with the same
    /// version number can be told apart. Empty when the build could not tell.
    pub commit: &'static str,
}

#[tauri::command]
pub fn app_info(app: tauri::AppHandle) -> AppInfo {
    let config = app.config();
    AppInfo {
        name: config
            .product_name
            .clone()
            .unwrap_or_else(|| "App".to_string()),
        version: app.package_info().version.to_string(),
        identifier: config.identifier.clone(),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        commit: env!("APP_COMMIT"),
    }
}

#[cfg(test)]
mod tests {
    /// `Object.prototype` is frozen in every page of the app, by the page's first import
    /// (`src/lib/freeze.ts`), and not by Tauri's setting, which on Windows would freeze it
    /// in the preview frames of artifacts too, where the diagram library cannot start
    /// with it frozen. The setting and the imports are changed together or not at all.
    #[test]
    fn each_page_of_the_app_freezes_the_prototype_first() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(
            conf["app"]["security"]["freezePrototype"],
            serde_json::json!(false),
            "with freezePrototype on, Tauri freezes the prototype in preview frames on Windows"
        );
        let freeze = include_str!("../../src/lib/freeze.ts");
        // By lines: a checkout on Windows may end them differently.
        assert!(
            freeze
                .lines()
                .any(|line| line == "Object.freeze(Object.prototype);"),
            "src/lib/freeze.ts no longer freezes the prototype"
        );
        // Every page of the app (the HTML files next to package.json) starts one module.
        for (page, entry, source, import) in [
            (
                include_str!("../../index.html"),
                "/src/main.tsx",
                include_str!("../../src/main.tsx"),
                "import \"./lib/freeze\";",
            ),
            (
                include_str!("../../overlay.html"),
                "/src/overlay/main.tsx",
                include_str!("../../src/overlay/main.tsx"),
                "import \"../lib/freeze\";",
            ),
            (
                include_str!("../../indicator.html"),
                "/src/overlay/indicator.tsx",
                include_str!("../../src/overlay/indicator.tsx"),
                "import \"../lib/freeze\";",
            ),
        ] {
            assert_eq!(page.matches("<script").count(), 1, "{entry}: one script");
            assert!(
                page.contains(&format!("<script type=\"module\" src=\"{entry}\">")),
                "{entry} is not the page's script"
            );
            // Imports run in the order they are written: the first one runs before
            // anything else the page loads.
            assert_eq!(source.lines().next(), Some(import), "{entry}");
        }
    }

    /// Every command in `commands.list` is granted to the main window and to no other.
    #[test]
    fn commands_list_matches_capabilities() {
        let list = include_str!("../commands.list");
        let main: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/main.json")).unwrap();
        let overlay: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/overlay.json")).unwrap();
        let perms = |v: &serde_json::Value| -> Vec<String> {
            v["permissions"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|p| p.as_str().map(str::to_string))
                .collect()
        };
        let main_perms = perms(&main);
        let overlay_perms = perms(&overlay);
        let indicator: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/indicator.json")).unwrap();
        let indicator_perms = perms(&indicator);
        for command in list
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            let permission = format!("allow-{}", command.replace('_', "-"));
            assert!(
                main_perms.contains(&permission),
                "capabilities/main.json must grant {permission}"
            );
            assert!(
                !overlay_perms.contains(&permission),
                "the overlay must not be granted {permission}"
            );
            // The Stop window may call exactly one app command.
            assert_eq!(
                indicator_perms.contains(&permission),
                command == "overlay_stop",
                "the indicator window may only be granted allow-overlay-stop ({permission})"
            );
        }
    }

    /// The webview may open http(s), mailto and tel links, plus exactly the two macOS
    /// Privacy panes the computer-use settings link to (the scope is a glob; `[?]` is a
    /// literal `?`).
    #[test]
    fn open_url_scope_is_the_two_privacy_panes() {
        let main: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/main.json")).unwrap();
        let perms = main["permissions"].as_array().unwrap();
        assert!(perms.contains(&serde_json::json!("opener:allow-default-urls")));
        assert!(
            !perms.contains(&serde_json::json!("opener:allow-open-url")),
            "open_url is granted only with its scope"
        );
        let scoped: Vec<&serde_json::Value> = perms
            .iter()
            .filter(|p| p["identifier"] == "opener:allow-open-url")
            .collect();
        assert_eq!(scoped.len(), 1);
        let patterns: Vec<&str> = scoped[0]["allow"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["url"].as_str().unwrap())
            .collect();
        // No wildcard: a bare `?` would match any one character.
        assert!(patterns.iter().all(|p| !p.replace("[?]", "").contains('?')));
        let mut urls: Vec<String> = patterns.iter().map(|p| p.replace("[?]", "?")).collect();
        urls.sort();
        let mut want = vec![
            computer::MACOS_ACCESSIBILITY_URL.to_string(),
            computer::MACOS_SCREEN_RECORDING_URL.to_string(),
        ];
        want.sort();
        assert_eq!(urls, want);
        assert!(urls.iter().all(|u| !u.contains('*') && !u.contains('[')));
        assert!(scoped[0].get("deny").is_none());
    }

    /// The config names its capabilities explicitly, so a stray file is never enabled.
    #[test]
    fn capabilities_are_listed_explicitly() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let caps = &conf["app"]["security"]["capabilities"];
        assert_eq!(caps, &serde_json::json!(["main", "overlay", "indicator"]));
        let csp = conf["app"]["security"]["csp"].as_str().unwrap();
        assert!(csp.contains("script-src 'self';"));
        assert!(!csp.contains("unsafe-eval"));
        assert!(!csp.contains("127.0.0.1"));
        // The whole policy, word for word, so that loosening it is a deliberate change
        // here. `frame-src` names the one origin artifact previews load from, as the two
        // kinds of webview spell it (artifacts/protocol.rs), and nothing else.
        assert_eq!(
            csp,
            "default-src 'self'; script-src 'self'; connect-src 'self' ipc: http://ipc.localhost; \
             img-src 'self' data: blob:; style-src 'self' 'unsafe-inline'; font-src 'self' data:; \
             object-src 'none'; frame-src artifact://sandbox.localhost http://artifact.sandbox.localhost; \
             form-action 'none'; base-uri 'self'; require-trusted-types-for 'script'"
        );
        assert_eq!(conf["app"]["withGlobalTauri"], serde_json::json!(false));
    }
}
