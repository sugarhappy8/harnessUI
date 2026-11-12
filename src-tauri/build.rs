//! Build script: Windows manifest (per-monitor DPI awareness) and the app-command ACL.
//!
//! Declaring the app's commands here makes Tauri require an explicit `allow-<command>`
//! permission per window. Without it every local window could call every command.

/// The first seven characters of the commit this build is made from: from the build
/// service's environment, or read from the repository's own files. No program is run.
/// Empty when neither says.
fn commit() -> String {
    let short = |sha: &str| {
        let sha = sha.trim();
        (sha.len() >= 7 && sha.bytes().all(|b| b.is_ascii_hexdigit())).then(|| sha[..7].to_string())
    };
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
    if let Some(sha) = std::env::var("GITHUB_SHA").ok().as_deref().and_then(short) {
        return sha;
    }
    let git = std::path::Path::new("../.git");
    println!("cargo:rerun-if-changed=../.git/HEAD");
    let Ok(head) = std::fs::read_to_string(git.join("HEAD")) else {
        return String::new();
    };
    let Some(reference) = head.trim().strip_prefix("ref: ") else {
        // A detached checkout: HEAD is the commit.
        return short(&head).unwrap_or_default();
    };
    println!("cargo:rerun-if-changed=../.git/{reference}");
    if let Some(sha) = std::fs::read_to_string(git.join(reference))
        .ok()
        .as_deref()
        .and_then(short)
    {
        return sha;
    }
    // A branch that only exists in the packed list.
    std::fs::read_to_string(git.join("packed-refs"))
        .ok()
        .and_then(|packed| {
            packed.lines().find_map(|line| {
                let (sha, name) = line.split_once(' ')?;
                (name == reference).then(|| short(sha)).flatten()
            })
        })
        .unwrap_or_default()
}

fn main() {
    println!("cargo:rerun-if-changed=commands.list");
    println!("cargo:rerun-if-changed=windows-app-manifest.xml");
    println!("cargo:rustc-env=APP_COMMIT={}", commit());

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
