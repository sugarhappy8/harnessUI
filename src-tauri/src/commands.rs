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
    }
}

#[cfg(test)]
mod tests {
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
        assert_eq!(conf["app"]["withGlobalTauri"], serde_json::json!(false));
    }
}
