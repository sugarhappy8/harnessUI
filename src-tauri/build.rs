//! Build script: Windows manifest (per-monitor DPI awareness) and the app-command ACL.
//!
//! Declaring the app's commands here makes Tauri require an explicit `allow-<command>`
//! permission per window. Without it every local window could call every command.

fn main() {
    println!("cargo:rerun-if-changed=commands.list");
    println!("cargo:rerun-if-changed=windows-app-manifest.xml");

    // The diagram library for artifact previews is built into the app from the npm
    // package (src/artifacts/protocol.rs), so that diagrams are drawn without a network.
    const MERMAID: &str = "../node_modules/mermaid/dist/mermaid.min.js";
    println!("cargo:rerun-if-changed={MERMAID}");
    assert!(
        std::path::Path::new(MERMAID).is_file(),
        "{MERMAID} is missing: run npm ci first (the diagram library for artifact previews is built into the app from node_modules)"
    );

    let list = std::fs::read_to_string("commands.list").expect("read commands.list");
    let commands: Vec<&'static str> = list
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| &*Box::leak(l.to_string().into_boxed_str()))
        .collect();
    let commands: &'static [&'static str] = Box::leak(commands.into_boxed_slice());

    let windows = tauri_build::WindowsAttributes::new()
        .app_manifest(include_str!("windows-app-manifest.xml"));
    let attributes = tauri_build::Attributes::new()
        .windows_attributes(windows)
        .app_manifest(tauri_build::AppManifest::new().commands(commands));
    tauri_build::try_build(attributes).expect("tauri-build failed");
}
