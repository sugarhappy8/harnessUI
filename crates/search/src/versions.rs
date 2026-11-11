//! The libraries a project uses, and their versions.
//!
//! Read from the manifests in one folder: `package.json` (with `package-lock.json` for the
//! exact versions), `Cargo.toml` (with `Cargo.lock`), `pyproject.toml`, `requirements.txt`
//! and `go.mod`. Only what the project names itself is listed, not what its libraries pull
//! in. The language itself is listed too where a manifest states it (`node`, `rust`,
//! `python`, `go`), because its documentation has versions as well.
//!
//! Nothing here touches the disk except [`read_folder`]: the app passes a reader that goes
//! through its path sandbox ([`from_manifests`]).

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Where a library comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ecosystem {
    Npm,
    Cargo,
    Pypi,
    Go,
}

impl Ecosystem {
    /// The name used in stored data.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Cargo => "cargo",
            Self::Pypi => "pypi",
            Self::Go => "go",
        }
    }
}

/// One library of the project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Library {
    /// As the manifest names it: `tokio`, `@tauri-apps/api`, `github.com/gin-gonic/gin`.
    pub name: String,
    /// The exact version from the lock file when there is one, else the first version of
    /// the requirement (`^18.3.1` gives `18.3.1`). Empty when the manifest names none.
    pub version: String,
    pub ecosystem: Ecosystem,
}

/// A version number as far as it is known: `1`, `1.38` or `1.38.2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: Option<u64>,
    pub patch: Option<u64>,
}

impl Version {
    /// `1.38` (or `1` when no minor is known).
    pub fn label(self) -> String {
        match self.minor {
            Some(minor) => format!("{}.{minor}", self.major),
            None => self.major.to_string(),
        }
    }
}

impl Library {
    /// The parsed version, when one is known.
    pub fn parsed(&self) -> Option<Version> {
        parse_version(&self.version)
    }

    /// `1.38` for `1.38.2`; empty when no version is known.
    pub fn version_label(&self) -> String {
        self.parsed().map(Version::label).unwrap_or_default()
    }

    /// The name people use for it: the last part of a Go module path, else the name.
    pub fn short_name(&self) -> &str {
        if self.ecosystem != Ecosystem::Go {
            return &self.name;
        }
        self.name
            .split('/')
            .rev()
            // A trailing "/v5" is the module's major version, not its name.
            .find(|part| !is_major_suffix(part) && !part.is_empty())
            .unwrap_or(&self.name)
    }

    /// The language itself (`node`, `rust`, `python`, `go`), not a package.
    pub fn is_runtime(&self) -> bool {
        RUNTIMES.contains(&(self.ecosystem, self.name.as_str()))
    }
}

/// The files looked for in a project folder.
pub const MANIFESTS: &[&str] = &[
    "package.json",
    "package-lock.json",
    "Cargo.toml",
    "Cargo.lock",
    "pyproject.toml",
    "requirements.txt",
    "go.mod",
];

/// A manifest larger than this is not read (lock files of big projects are a few MB).
pub const MAX_MANIFEST_BYTES: u64 = 8 * 1024 * 1024;

/// Libraries listed at most.
pub const MAX_LIBRARIES: usize = 3000;

/// The languages, as they are listed next to the packages.
const RUNTIMES: &[(Ecosystem, &str)] = &[
    (Ecosystem::Npm, "node"),
    (Ecosystem::Cargo, "rust"),
    (Ecosystem::Pypi, "python"),
    (Ecosystem::Go, "go"),
];

/// `v2`, `v15`: the major-version part at the end of a Go module path.
fn is_major_suffix(part: &str) -> bool {
    part.strip_prefix('v')
        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

/// A run of at most six digits at the start of `text`, and what follows it. Longer runs
/// are not version numbers.
fn digits(text: &str) -> Option<(u64, &str)> {
    let end = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    if end == 0 || end > 6 {
        return None;
    }
    Some((text[..end].parse().ok()?, &text[end..]))
}

/// The first version number in `text`: `1.38.2` in `^1.38.2`, `4.2` in `>=4.2,<5`, `18` in
/// `18.x`, `1.9.1` in `v1.9.1`. `None` when there is none (`*`, `latest`).
pub fn parse_version(text: &str) -> Option<Version> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let (major, rest) = digits(&text[start..])?;
    let mut version = Version {
        major,
        minor: None,
        patch: None,
    };
    let Some((minor, rest)) = rest.strip_prefix('.').and_then(digits) else {
        return Some(version);
    };
    version.minor = Some(minor);
    if let Some((patch, _)) = rest.strip_prefix('.').and_then(digits) {
        version.patch = Some(patch);
    }
    Some(version)
}

/// The version of a requirement as text: `18.3.1` for `^18.3.1`. Empty when it has none.
fn requirement_version(requirement: &str) -> String {
    match parse_version(requirement) {
        Some(Version {
            major,
            minor: Some(minor),
            patch: Some(patch),
        }) => format!("{major}.{minor}.{patch}"),
        Some(version) => version.label(),
        None => String::new(),
    }
}

/// A library name as it is compared: lower case, `-` for `_` and `.`.
fn normal(name: &str) -> String {
    name.trim().to_lowercase().replace(['_', '.'], "-")
}

fn library(name: &str, version: String, ecosystem: Ecosystem) -> Library {
    Library {
        name: name.to_string(),
        version,
        ecosystem,
    }
}

/// The exact version of each package the lock holds at the top of `node_modules`.
fn npm_lock_versions(lock: &str) -> std::collections::HashMap<String, String> {
    let mut versions = std::collections::HashMap::new();
    let Ok(serde_json::Value::Object(root)) = serde_json::from_str::<serde_json::Value>(lock)
    else {
        return versions;
    };
    // Lock versions 2 and 3: "packages" keyed by path.
    if let Some(serde_json::Value::Object(packages)) = root.get("packages") {
        for (path, entry) in packages {
            let Some(name) = path.strip_prefix("node_modules/") else {
                continue;
            };
            // Only the project's own level, not a package nested inside another.
            if name.contains("node_modules/") {
                continue;
            }
            if let Some(version) = entry.get("version").and_then(serde_json::Value::as_str) {
                versions.insert(name.to_string(), version.to_string());
            }
        }
    }
    // Lock version 1: "dependencies" keyed by name.
    if let Some(serde_json::Value::Object(dependencies)) = root.get("dependencies") {
        for (name, entry) in dependencies {
            if let Some(version) = entry.get("version").and_then(serde_json::Value::as_str) {
                versions
                    .entry(name.clone())
                    .or_insert_with(|| version.to_string());
            }
        }
    }
    versions
}

/// `package.json`, with the text of `package-lock.json` when there is one.
pub fn parse_package_json(manifest: &str, lock: Option<&str>) -> Vec<Library> {
    let Ok(serde_json::Value::Object(root)) = serde_json::from_str::<serde_json::Value>(manifest)
    else {
        return Vec::new();
    };
    let locked = lock.map(npm_lock_versions).unwrap_or_default();
    let mut libraries = Vec::new();
    for table in [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ] {
        let Some(serde_json::Value::Object(dependencies)) = root.get(table) else {
            continue;
        };
        for (name, requirement) in dependencies {
            let requirement = requirement.as_str().unwrap_or("");
            // "npm:real-name@^1.2": installed under another name.
            let (real, requirement) = match requirement.strip_prefix("npm:") {
                Some(alias) => match alias.rfind('@').filter(|at| *at > 0) {
                    Some(at) => (&alias[..at], &alias[at + 1..]),
                    None => (alias, ""),
                },
                None => (name.as_str(), requirement),
            };
            // A folder, an archive or a git address has no version to read.
            let elsewhere = [
                "file:",
                "link:",
                "workspace:",
                "portal:",
                "git",
                "http:",
                "https:",
            ]
            .iter()
            .any(|prefix| requirement.starts_with(prefix))
                || requirement.contains('/');
            let version = match locked.get(name) {
                Some(exact) => exact.clone(),
                None if elsewhere => continue,
                None => requirement_version(requirement),
            };
            libraries.push(library(real, version, Ecosystem::Npm));
        }
    }
    if let Some(node) = root
        .get("engines")
        .and_then(|e| e.get("node"))
        .and_then(serde_json::Value::as_str)
    {
        let version = requirement_version(node);
        if !version.is_empty() {
            libraries.push(library("node", version, Ecosystem::Npm));
        }
    }
    libraries
}

/// Every version of every package in a `Cargo.lock`.
fn cargo_lock_versions(lock: &str) -> std::collections::HashMap<String, Vec<String>> {
    let mut versions: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    let Ok(root) = lock.parse::<toml::Table>() else {
        return versions;
    };
    let Some(toml::Value::Array(packages)) = root.get("package") else {
        return versions;
    };
    for package in packages {
        if let (Some(name), Some(version)) = (
            package.get("name").and_then(toml::Value::as_str),
            package.get("version").and_then(toml::Value::as_str),
        ) {
            versions
                .entry(normal(name))
                .or_default()
                .push(version.to_string());
        }
    }
    versions
}

/// Of the locked versions of one crate, the one a requirement asks for: the highest that
/// starts like the requirement (the same major, and for `0.x` the same minor), else the
/// highest there is.
fn pick_locked(locked: &[String], requirement: &str) -> Option<String> {
    let key = |v: &String| {
        parse_version(v).map(|p| (p.major, p.minor.unwrap_or(0), p.patch.unwrap_or(0)))
    };
    let wanted = parse_version(requirement);
    let fits = |v: &&String| match (wanted, parse_version(v)) {
        (Some(want), Some(have)) => {
            want.major == have.major
                && (want.major > 0 || want.minor.is_none() || want.minor == have.minor)
        }
        _ => false,
    };
    locked
        .iter()
        .filter(fits)
        .max_by_key(|v| key(v))
        .or_else(|| locked.iter().max_by_key(|v| key(v)))
        .cloned()
}

/// `Cargo.toml`, with the text of `Cargo.lock` when there is one.
pub fn parse_cargo(manifest: &str, lock: Option<&str>) -> Vec<Library> {
    let Ok(root) = manifest.parse::<toml::Table>() else {
        return Vec::new();
    };
    let locked = lock.map(cargo_lock_versions).unwrap_or_default();
    let workspace = root.get("workspace").and_then(toml::Value::as_table);
    let workspace_dependencies = workspace
        .and_then(|w| w.get("dependencies"))
        .and_then(toml::Value::as_table);
    // (name in the registry, requirement) for one entry of a dependency table.
    let entry = |name: &str, value: &toml::Value| -> Option<(String, String)> {
        match value {
            toml::Value::String(requirement) => Some((name.to_string(), requirement.clone())),
            toml::Value::Table(table) => {
                let real = table
                    .get("package")
                    .and_then(toml::Value::as_str)
                    .unwrap_or(name)
                    .to_string();
                if let Some(requirement) = table.get("version").and_then(toml::Value::as_str) {
                    return Some((real, requirement.to_string()));
                }
                if table.get("workspace").and_then(toml::Value::as_bool) == Some(true) {
                    // The requirement is the workspace's; that table is read as well.
                    return Some((real, String::new()));
                }
                // A path or git dependency: listed only when the lock knows a version.
                Some((real, String::new()))
            }
            _ => None,
        }
    };
    let mut tables: Vec<&toml::Table> = Vec::new();
    for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(table) = root.get(key).and_then(toml::Value::as_table) {
            tables.push(table);
        }
    }
    if let Some(targets) = root.get("target").and_then(toml::Value::as_table) {
        for target in targets.values() {
            for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
                if let Some(table) = target.get(key).and_then(toml::Value::as_table) {
                    tables.push(table);
                }
            }
        }
    }
    // The workspace's own table first, so its requirements are known to the others.
    let mut requirements: Vec<(String, String)> = Vec::new();
    for table in workspace_dependencies.into_iter().chain(tables) {
        for (name, value) in table {
            let Some((real, requirement)) = entry(name, value) else {
                continue;
            };
            match requirements
                .iter_mut()
                .find(|(n, _)| normal(n) == normal(&real))
            {
                Some((_, known)) if known.is_empty() => *known = requirement,
                Some(_) => {}
                None => requirements.push((real, requirement)),
            }
        }
    }
    let mut libraries = Vec::new();
    for (name, requirement) in requirements {
        let exact = locked
            .get(&normal(&name))
            .and_then(|versions| pick_locked(versions, &requirement));
        let version = match exact {
            Some(exact) => exact,
            None if requirement.is_empty() => continue,
            None => requirement_version(&requirement),
        };
        libraries.push(library(&name, version, Ecosystem::Cargo));
    }
    let rust_version = root
        .get("package")
        .and_then(|p| p.get("rust-version"))
        .and_then(toml::Value::as_str)
        .or_else(|| {
            workspace
                .and_then(|w| w.get("package"))
                .and_then(|p| p.get("rust-version"))
                .and_then(toml::Value::as_str)
        });
    if let Some(rust) = rust_version
        .map(requirement_version)
        .filter(|v| !v.is_empty())
    {
        libraries.push(library("rust", rust, Ecosystem::Cargo));
    }
    libraries
}

/// One requirement in the form Python tools write: `Django>=4.2,<5`,
/// `requests[socks]==2.31.0`, `uvicorn ; python_version < '3.12'`, `pkg @ https://…`.
fn python_requirement(line: &str) -> Option<Library> {
    // What follows ";" says when the requirement applies, not which version.
    let spec = line.split(';').next().unwrap_or("").trim();
    let end = spec
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
        .unwrap_or(spec.len());
    let name = normal(&spec[..end]);
    if name.is_empty() || !name.starts_with(|c: char| c.is_ascii_alphanumeric()) {
        return None;
    }
    let mut rest = spec[end..].trim_start();
    if let Some(after) = rest.strip_prefix('[') {
        rest = after.split_once(']').map_or("", |(_, r)| r).trim_start();
    }
    // "name @ address": installed from that address, version unknown.
    let version = if rest.starts_with('@') {
        String::new()
    } else {
        requirement_version(rest)
    };
    Some(library(&name, version, Ecosystem::Pypi))
}

/// `pyproject.toml`: the standard `[project]` tables and Poetry's.
pub fn parse_pyproject(manifest: &str) -> Vec<Library> {
    let Ok(root) = manifest.parse::<toml::Table>() else {
        return Vec::new();
    };
    let mut libraries = Vec::new();
    let mut python: Option<String> = None;
    let strings = |value: Option<&toml::Value>, out: &mut Vec<Library>| {
        if let Some(toml::Value::Array(items)) = value {
            out.extend(
                items
                    .iter()
                    .filter_map(toml::Value::as_str)
                    .filter_map(python_requirement),
            );
        }
    };
    if let Some(project) = root.get("project").and_then(toml::Value::as_table) {
        strings(project.get("dependencies"), &mut libraries);
        if let Some(groups) = project
            .get("optional-dependencies")
            .and_then(toml::Value::as_table)
        {
            for group in groups.values() {
                strings(Some(group), &mut libraries);
            }
        }
        python = project
            .get("requires-python")
            .and_then(toml::Value::as_str)
            .map(requirement_version);
    }
    if let Some(groups) = root
        .get("dependency-groups")
        .and_then(toml::Value::as_table)
    {
        for group in groups.values() {
            strings(Some(group), &mut libraries);
        }
    }
    let poetry = root
        .get("tool")
        .and_then(|t| t.get("poetry"))
        .and_then(toml::Value::as_table);
    let mut poetry_tables: Vec<&toml::Table> = Vec::new();
    if let Some(poetry) = poetry {
        for key in ["dependencies", "dev-dependencies"] {
            if let Some(table) = poetry.get(key).and_then(toml::Value::as_table) {
                poetry_tables.push(table);
            }
        }
        if let Some(groups) = poetry.get("group").and_then(toml::Value::as_table) {
            for group in groups.values() {
                if let Some(table) = group.get("dependencies").and_then(toml::Value::as_table) {
                    poetry_tables.push(table);
                }
            }
        }
    }
    for table in poetry_tables {
        for (name, value) in table {
            let requirement = match value {
                toml::Value::String(requirement) => requirement.as_str(),
                toml::Value::Table(t) => {
                    t.get("version").and_then(toml::Value::as_str).unwrap_or("")
                }
                _ => "",
            };
            if normal(name) == "python" {
                python.get_or_insert_with(|| requirement_version(requirement));
            } else {
                libraries.push(library(
                    &normal(name),
                    requirement_version(requirement),
                    Ecosystem::Pypi,
                ));
            }
        }
    }
    if let Some(python) = python.filter(|v| !v.is_empty()) {
        libraries.push(library("python", python, Ecosystem::Pypi));
    }
    libraries
}

/// `requirements.txt`.
pub fn parse_requirements(text: &str) -> Vec<Library> {
    text.lines()
        .map(|line| line.split(" #").next().unwrap_or("").trim())
        // Options (-r, -e, --index-url) and comments name no library.
        .filter(|line| !line.is_empty() && !line.starts_with(['#', '-']))
        // "--hash=…" after a requirement is not part of it.
        .map(|line| line.split(" --").next().unwrap_or(""))
        .filter_map(python_requirement)
        .collect()
}

/// `go.mod`: the modules it requires directly.
pub fn parse_go_mod(text: &str) -> Vec<Library> {
    let mut libraries = Vec::new();
    let mut in_block = false;
    let require = |line: &str, out: &mut Vec<Library>| {
        if line.contains("// indirect") {
            return;
        }
        let mut parts = line.split_whitespace();
        if let (Some(module), Some(version)) = (parts.next(), parts.next()) {
            let version = requirement_version(version);
            if !version.is_empty() && module.contains('.') {
                out.push(library(module, version, Ecosystem::Go));
            }
        }
    };
    for line in text.lines().map(str::trim) {
        if in_block {
            if line.starts_with(')') {
                in_block = false;
            } else {
                require(line, &mut libraries);
            }
        } else if line == "require (" || line == "require(" {
            in_block = true;
        } else if let Some(one) = line.strip_prefix("require ") {
            require(one, &mut libraries);
        } else if let Some(go) = line.strip_prefix("go ") {
            let version = requirement_version(go);
            if !version.is_empty() {
                libraries.push(library("go", version, Ecosystem::Go));
            }
        }
    }
    libraries
}

/// The project's libraries, from whatever manifests `read` can give. `read` gets a file
/// name from [`MANIFESTS`] and returns its text, or `None` when the file is not there or
/// may not be read. A manifest that does not parse is skipped. The result is sorted and
/// has each library once.
pub fn from_manifests(read: &dyn Fn(&str) -> Option<String>) -> Vec<Library> {
    let mut libraries = Vec::new();
    if let Some(manifest) = read("package.json") {
        let lock = read("package-lock.json");
        libraries.extend(parse_package_json(&manifest, lock.as_deref()));
    }
    if let Some(manifest) = read("Cargo.toml") {
        let lock = read("Cargo.lock");
        libraries.extend(parse_cargo(&manifest, lock.as_deref()));
    }
    if let Some(manifest) = read("pyproject.toml") {
        libraries.extend(parse_pyproject(&manifest));
    }
    if let Some(text) = read("requirements.txt") {
        libraries.extend(parse_requirements(&text));
    }
    if let Some(text) = read("go.mod") {
        libraries.extend(parse_go_mod(&text));
    }
    // Sorted by ecosystem and name; of two entries for one library, the one that knows a
    // version stays.
    libraries.sort_by(|a, b| {
        (a.ecosystem, &a.name, b.version.is_empty()).cmp(&(
            b.ecosystem,
            &b.name,
            a.version.is_empty(),
        ))
    });
    libraries
        .dedup_by(|later, first| later.ecosystem == first.ecosystem && later.name == first.name);
    libraries.truncate(MAX_LIBRARIES);
    libraries
}

/// [`from_manifests`] over the files directly in `dir`.
pub fn read_folder(dir: &Path) -> Vec<Library> {
    from_manifests(&|name| {
        let path = dir.join(name);
        let size = std::fs::metadata(&path).ok()?.len();
        if size > MAX_MANIFEST_BYTES {
            return None;
        }
        std::fs::read_to_string(path).ok()
    })
}

/// Library names that are also everyday words. A question that uses one of them is not
/// taken to be about the library; the model can still name it.
const EVERYDAY_WORDS: &[&str] = &[
    "all",
    "any",
    "async",
    "base",
    "build",
    "bytes",
    "cache",
    "can",
    "chalk",
    "click",
    "clock",
    "color",
    "colors",
    "commander",
    "config",
    "core",
    "data",
    "date",
    "debug",
    "error",
    "fast",
    "file",
    "files",
    "form",
    "future",
    "futures",
    "get",
    "glob",
    "go",
    "http",
    "image",
    "is",
    "it",
    "json",
    "list",
    "log",
    "make",
    "map",
    "mock",
    "moment",
    "ms",
    "new",
    "next",
    "node",
    "nose",
    "one",
    "open",
    "path",
    "pillow",
    "process",
    "queue",
    "rand",
    "random",
    "regex",
    "request",
    "requests",
    "rich",
    "run",
    "set",
    "six",
    "string",
    "test",
    "that",
    "this",
    "time",
    "tools",
    "two",
    "type",
    "types",
    "url",
    "use",
    "util",
    "utils",
    "uuid",
    "web",
    "when",
    "which",
    "zip",
];

/// The words of a name or a question as they are compared: lower-case runs of letters and
/// digits.
fn name_words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// The library called `name`, however it is written: letter case, `-` and `_` do not
/// matter, and the last part of a Go module path is enough.
pub fn find_library<'a>(libraries: &'a [Library], name: &str) -> Option<&'a Library> {
    let wanted = normal(name);
    if wanted.is_empty() {
        return None;
    }
    libraries
        .iter()
        .find(|l| normal(&l.name) == wanted)
        .or_else(|| libraries.iter().find(|l| normal(l.short_name()) == wanted))
}

/// The library a question is about, when it names one of the project's as whole words.
/// Names that are also everyday words ("time", "open", "next") never count: the model has
/// to pass those as the library. Of several, the longest name wins.
pub fn mentioned<'a>(libraries: &'a [Library], query: &str) -> Option<&'a Library> {
    let query = name_words(query);
    let mut best: Option<(usize, &Library)> = None;
    for library in libraries {
        let name = name_words(library.short_name());
        if name.is_empty() || name.len() > query.len() {
            continue;
        }
        if name.len() == 1 && EVERYDAY_WORDS.contains(&name[0].as_str()) && !library.is_runtime() {
            continue;
        }
        // A language named by an everyday word ("go") is not guessed either.
        if library.is_runtime() && matches!(library.name.as_str(), "go" | "node") {
            continue;
        }
        if !query
            .windows(name.len())
            .any(|window| window == name.as_slice())
        {
            continue;
        }
        let length: usize = name.iter().map(String::len).sum();
        if best.is_none_or(|(longest, _)| length > longest) {
            best = Some((length, library));
        }
    }
    best.map(|(_, library)| library)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lib(name: &str, version: &str, ecosystem: Ecosystem) -> Library {
        Library {
            name: name.into(),
            version: version.into(),
            ecosystem,
        }
    }

    fn get<'a>(libs: &'a [Library], name: &str) -> &'a Library {
        libs.iter()
            .find(|l| l.name == name)
            .unwrap_or_else(|| panic!("{name} is missing from {libs:?}"))
    }

    fn names(libs: &[Library]) -> Vec<&str> {
        libs.iter().map(|l| l.name.as_str()).collect()
    }

    #[test]
    fn versions_are_read_from_requirement_strings() {
        let v = |major, minor, patch| {
            Some(Version {
                major,
                minor,
                patch,
            })
        };
        assert_eq!(parse_version("1.38.2"), v(1, Some(38), Some(2)));
        assert_eq!(parse_version("^18.3.1"), v(18, Some(3), Some(1)));
        assert_eq!(parse_version("~2.12"), v(2, Some(12), None));
        assert_eq!(parse_version(">=4.2,<5"), v(4, Some(2), None));
        assert_eq!(parse_version("v1.9.1"), v(1, Some(9), Some(1)));
        assert_eq!(parse_version("18.x"), v(18, None, None));
        assert_eq!(parse_version("1"), v(1, None, None));
        assert_eq!(parse_version("=0.110.0"), v(0, Some(110), Some(0)));
        assert_eq!(parse_version("2.0.0-beta.3"), v(2, Some(0), Some(0)));
        assert_eq!(parse_version("*"), None);
        assert_eq!(parse_version("latest"), None);
        assert_eq!(parse_version(""), None);
        // A number too long to be a version is not one.
        assert_eq!(parse_version("99999999999999999999999"), None);
        assert_eq!(
            lib("tokio", "1.38.2", Ecosystem::Cargo).version_label(),
            "1.38"
        );
        assert_eq!(lib("react", "18", Ecosystem::Npm).version_label(), "18");
        assert_eq!(lib("numpy", "", Ecosystem::Pypi).version_label(), "");
    }

    const PACKAGE_JSON: &str = r#"{
      "name": "demo",
      "engines": { "node": ">=20.11" },
      "dependencies": {
        "react": "^18.3.1",
        "@tauri-apps/api": "~2.12.4",
        "left-pad": "*",
        "local": "file:../local",
        "aliased": "npm:real-pkg@^3.1.0",
        "gitdep": "github:user/repo#v1"
      },
      "devDependencies": { "vite": "^8.3.7", "typescript": "~6.0.3" },
      "peerDependencies": { "react-dom": ">=18" }
    }"#;

    const PACKAGE_LOCK: &str = r#"{
      "name": "demo",
      "lockfileVersion": 3,
      "packages": {
        "": { "name": "demo" },
        "node_modules/react": { "version": "18.3.1" },
        "node_modules/vite": { "version": "8.3.9" },
        "node_modules/@tauri-apps/api": { "version": "2.12.4" },
        "node_modules/left-pad": { "version": "1.3.0" },
        "node_modules/vite/node_modules/react": { "version": "17.0.2" }
      }
    }"#;

    #[test]
    fn package_json_with_its_lock() {
        let libs = parse_package_json(PACKAGE_JSON, Some(PACKAGE_LOCK));
        assert_eq!(get(&libs, "react").version, "18.3.1");
        assert_eq!(get(&libs, "react").ecosystem, Ecosystem::Npm);
        assert_eq!(get(&libs, "@tauri-apps/api").version, "2.12.4");
        // The lock is exact; the requirement is only where it starts.
        assert_eq!(get(&libs, "vite").version, "8.3.9");
        assert_eq!(get(&libs, "left-pad").version, "1.3.0");
        // Not in the lock: the requirement's own version.
        assert_eq!(get(&libs, "typescript").version, "6.0.3");
        assert_eq!(get(&libs, "react-dom").version, "18");
        // An alias is listed under the real package's name.
        assert_eq!(get(&libs, "real-pkg").version, "3.1.0");
        assert_eq!(get(&libs, "node").version, "20.11");
        // No version to be had: a local folder and a git address.
        assert!(!names(&libs).contains(&"local"), "{libs:?}");
        assert!(!names(&libs).contains(&"gitdep"), "{libs:?}");
        assert!(!names(&libs).contains(&"aliased"), "{libs:?}");
    }

    #[test]
    fn package_json_without_a_lock_and_with_an_old_lock() {
        let libs = parse_package_json(PACKAGE_JSON, None);
        assert_eq!(get(&libs, "react").version, "18.3.1");
        assert_eq!(get(&libs, "vite").version, "8.3.7");
        assert_eq!(get(&libs, "left-pad").version, "");
        let old = r#"{"lockfileVersion": 1, "dependencies": {"react": {"version": "16.14.0"}}}"#;
        let libs = parse_package_json(PACKAGE_JSON, Some(old));
        assert_eq!(get(&libs, "react").version, "16.14.0");
        // A lock that is not JSON is ignored; a manifest that is not JSON gives nothing.
        assert_eq!(
            get(&parse_package_json(PACKAGE_JSON, Some("{ nope")), "react").version,
            "18.3.1"
        );
        assert!(parse_package_json("{ nope", None).is_empty());
        assert!(parse_package_json("[1, 2]", None).is_empty());
    }

    const CARGO_TOML: &str = r#"
[package]
name = "demo"
rust-version = "1.75"

[dependencies]
tokio = { version = "1.38", features = ["full"] }
serde = "1"
my-http = { package = "reqwest", version = "0.12" }
local = { path = "../local" }
shared = { workspace = true }

[dev-dependencies]
proptest = "1.5"

[target.'cfg(windows)'.dependencies]
windows = "0.62"

[workspace.dependencies]
shared = "2.3"
"#;

    const CARGO_LOCK: &str = r#"
version = 4

[[package]]
name = "tokio"
version = "1.38.2"

[[package]]
name = "serde"
version = "1.0.219"

[[package]]
name = "reqwest"
version = "0.11.27"

[[package]]
name = "reqwest"
version = "0.12.9"

[[package]]
name = "windows"
version = "0.62.2"
"#;

    #[test]
    fn cargo_toml_with_its_lock() {
        let libs = parse_cargo(CARGO_TOML, Some(CARGO_LOCK));
        assert_eq!(get(&libs, "tokio").version, "1.38.2");
        assert_eq!(get(&libs, "tokio").ecosystem, Ecosystem::Cargo);
        assert_eq!(get(&libs, "serde").version, "1.0.219");
        // Two versions are locked: the one the requirement asks for.
        assert_eq!(get(&libs, "reqwest").version, "0.12.9");
        assert!(!names(&libs).contains(&"my-http"));
        assert_eq!(get(&libs, "proptest").version, "1.5");
        assert_eq!(get(&libs, "windows").version, "0.62.2");
        assert_eq!(get(&libs, "shared").version, "2.3");
        assert_eq!(get(&libs, "rust").version, "1.75");
        assert!(!names(&libs).contains(&"local"), "{libs:?}");
        assert!(!names(&libs).contains(&"demo"), "{libs:?}");
    }

    #[test]
    fn cargo_toml_without_a_lock_and_broken_files() {
        let libs = parse_cargo(CARGO_TOML, None);
        assert_eq!(get(&libs, "tokio").version, "1.38");
        assert_eq!(get(&libs, "reqwest").version, "0.12");
        assert!(parse_cargo("this is [not toml", None).is_empty());
        assert_eq!(
            get(&parse_cargo(CARGO_TOML, Some("[[package")), "tokio").version,
            "1.38"
        );
        // A workspace root without a package of its own.
        let root = "[workspace]\nmembers = [\"a\"]\n[workspace.package]\nrust-version = \"1.80\"\n[workspace.dependencies]\nurl = \"2\"\n";
        let libs = parse_cargo(root, None);
        assert_eq!(get(&libs, "url").version, "2");
        assert_eq!(get(&libs, "rust").version, "1.80");
    }

    const PYPROJECT: &str = r#"
[project]
name = "demo"
requires-python = ">=3.11"
dependencies = [
    "Django>=4.2,<5",
    "requests[socks]==2.31.0",
    "numpy",
    "uvicorn ; python_version < '3.12'",
]

[project.optional-dependencies]
dev = ["pytest~=8.1"]

[dependency-groups]
lint = ["ruff>=0.4.4", { include-group = "dev" }]

[tool.poetry.dependencies]
python = "^3.11"
fastapi = "^0.110.0"
pandas = { version = "~2.2", extras = ["excel"] }

[tool.poetry.group.dev.dependencies]
black = "24.3.0"
"#;

    #[test]
    fn pyproject_standard_and_poetry_tables() {
        let libs = parse_pyproject(PYPROJECT);
        assert_eq!(get(&libs, "django").version, "4.2");
        assert_eq!(get(&libs, "django").ecosystem, Ecosystem::Pypi);
        assert_eq!(get(&libs, "requests").version, "2.31.0");
        assert_eq!(get(&libs, "numpy").version, "");
        assert_eq!(get(&libs, "uvicorn").version, "");
        assert_eq!(get(&libs, "pytest").version, "8.1");
        assert_eq!(get(&libs, "ruff").version, "0.4.4");
        assert_eq!(get(&libs, "fastapi").version, "0.110.0");
        assert_eq!(get(&libs, "pandas").version, "2.2");
        assert_eq!(get(&libs, "black").version, "24.3.0");
        assert_eq!(get(&libs, "python").version, "3.11");
        assert_eq!(libs.iter().filter(|l| l.name == "python").count(), 1);
        assert!(parse_pyproject("not = [toml").is_empty());
    }

    #[test]
    fn requirements_txt() {
        let text = "# pinned for production\nDjango==4.2.11\nrequests>=2.31,<3\nnumpy\n\
                    uvicorn[standard]~=0.29.0 ; python_version >= \"3.8\"\n-r base.txt\n\
                    -e git+https://github.com/x/y.git#egg=y\n--index-url https://example.com/simple\n\
                    pkg @ https://example.com/pkg-1.0.whl\nflask==3.0.2 --hash=sha256:abc\n\
                    Typing_Extensions==4.11.0  # inline comment\n";
        let libs = parse_requirements(text);
        assert_eq!(get(&libs, "django").version, "4.2.11");
        assert_eq!(get(&libs, "requests").version, "2.31");
        assert_eq!(get(&libs, "numpy").version, "");
        assert_eq!(get(&libs, "uvicorn").version, "0.29.0");
        assert_eq!(get(&libs, "pkg").version, "");
        assert_eq!(get(&libs, "flask").version, "3.0.2");
        // Names are compared as PyPI does: lower case, "-" for "_" and ".".
        assert_eq!(get(&libs, "typing-extensions").version, "4.11.0");
        assert_eq!(libs.len(), 7, "{libs:?}");
    }

    #[test]
    fn go_mod() {
        let text = "module example.com/demo\n\ngo 1.22.3\n\nrequire (\n\tgithub.com/gin-gonic/gin v1.9.1\n\tgolang.org/x/sync v0.7.0 // indirect\n\tgopkg.in/yaml.v3 v3.0.1\n)\n\nrequire github.com/spf13/cobra v1.8.0\n\nreplace example.com/x => ../x\n";
        let libs = parse_go_mod(text);
        assert_eq!(get(&libs, "github.com/gin-gonic/gin").version, "1.9.1");
        assert_eq!(
            get(&libs, "github.com/gin-gonic/gin").ecosystem,
            Ecosystem::Go
        );
        assert_eq!(get(&libs, "gopkg.in/yaml.v3").version, "3.0.1");
        assert_eq!(get(&libs, "github.com/spf13/cobra").version, "1.8.0");
        assert_eq!(get(&libs, "go").version, "1.22.3");
        assert!(!names(&libs).contains(&"golang.org/x/sync"), "indirect");
        assert_eq!(libs.len(), 4, "{libs:?}");
        assert_eq!(get(&libs, "github.com/gin-gonic/gin").short_name(), "gin");
        assert_eq!(
            lib("github.com/go-chi/chi/v5", "5.0.12", Ecosystem::Go).short_name(),
            "chi"
        );
        assert_eq!(lib("tokio", "1", Ecosystem::Cargo).short_name(), "tokio");
        assert_eq!(
            lib("@tauri-apps/api", "2", Ecosystem::Npm).short_name(),
            "@tauri-apps/api"
        );
    }

    #[test]
    fn every_manifest_in_a_folder_is_read_once() {
        let asked = std::cell::RefCell::new(Vec::new());
        let read = |name: &str| -> Option<String> {
            asked.borrow_mut().push(name.to_string());
            match name {
                "package.json" => Some(PACKAGE_JSON.into()),
                "package-lock.json" => Some(PACKAGE_LOCK.into()),
                "Cargo.toml" => Some(CARGO_TOML.into()),
                "Cargo.lock" => Some(CARGO_LOCK.into()),
                "requirements.txt" => Some("Django==4.2.11\n".into()),
                "pyproject.toml" => Some("not [ toml".into()),
                _ => None,
            }
        };
        let libs = from_manifests(&read);
        let mut asked = asked.into_inner();
        asked.sort();
        let mut want: Vec<String> = MANIFESTS.iter().map(|m| m.to_string()).collect();
        want.sort();
        assert_eq!(asked, want);
        assert_eq!(get(&libs, "react").version, "18.3.1");
        assert_eq!(get(&libs, "tokio").version, "1.38.2");
        assert_eq!(get(&libs, "django").version, "4.2.11");
        // Sorted, each library once.
        let mut sorted = libs.clone();
        sorted.sort_by(|a, b| (a.ecosystem, &a.name).cmp(&(b.ecosystem, &b.name)));
        sorted.dedup_by(|a, b| a.ecosystem == b.ecosystem && a.name == b.name);
        assert_eq!(libs, sorted);
        // Nothing to read: nothing found, no panic.
        assert!(from_manifests(&|_| None).is_empty());
    }

    #[test]
    fn a_folder_on_disk() {
        let dir = std::env::temp_dir().join(format!("search-versions-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("go.mod"), "module x\n\ngo 1.22\n").unwrap();
        std::fs::write(dir.join("requirements.txt"), "flask==3.0.2\n").unwrap();
        let libs = read_folder(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(get(&libs, "go").version, "1.22");
        assert_eq!(get(&libs, "flask").version, "3.0.2");
        assert!(read_folder(Path::new("/no/such/folder/anywhere")).is_empty());
    }

    fn project() -> Vec<Library> {
        vec![
            lib("react", "18.3.1", Ecosystem::Npm),
            lib("react-router", "6.23.0", Ecosystem::Npm),
            lib("@tauri-apps/api", "2.12.4", Ecosystem::Npm),
            lib("next", "14.2.3", Ecosystem::Npm),
            lib("tokio", "1.38.2", Ecosystem::Cargo),
            lib("serde_json", "1.0.117", Ecosystem::Cargo),
            lib("time", "0.3.36", Ecosystem::Cargo),
            lib("open", "5.1.2", Ecosystem::Cargo),
            lib("django", "4.2.11", Ecosystem::Pypi),
            lib("python", "3.11", Ecosystem::Pypi),
            lib("github.com/gin-gonic/gin", "1.9.1", Ecosystem::Go),
        ]
    }

    #[test]
    fn a_library_is_found_however_its_name_is_written() {
        let libs = project();
        let found = |name: &str| find_library(&libs, name).map(|l| l.name.as_str());
        assert_eq!(found("tokio"), Some("tokio"));
        assert_eq!(found(" Tokio "), Some("tokio"));
        assert_eq!(found("serde-json"), Some("serde_json"));
        assert_eq!(found("SERDE_JSON"), Some("serde_json"));
        assert_eq!(found("@tauri-apps/api"), Some("@tauri-apps/api"));
        assert_eq!(found("gin"), Some("github.com/gin-gonic/gin"));
        assert_eq!(
            found("github.com/gin-gonic/gin"),
            Some("github.com/gin-gonic/gin")
        );
        // Named by the model, everyday words count too.
        assert_eq!(found("time"), Some("time"));
        assert_eq!(found("next"), Some("next"));
        assert_eq!(found("flask"), None);
        assert_eq!(found(""), None);
    }

    #[test]
    fn a_question_names_a_library_as_whole_words() {
        let libs = project();
        let found = |query: &str| mentioned(&libs, query).map(|l| l.name.as_str());
        assert_eq!(found("how do I use tokio spawn_blocking"), Some("tokio"));
        assert_eq!(found("Django forms validation"), Some("django"));
        assert_eq!(found("react useEffect cleanup"), Some("react"));
        // The longest name wins.
        assert_eq!(found("react-router loader example"), Some("react-router"));
        assert_eq!(found("serde_json from_str error"), Some("serde_json"));
        assert_eq!(found("serde json pretty print"), Some("serde_json"));
        assert_eq!(
            found("gin middleware order"),
            Some("github.com/gin-gonic/gin")
        );
        assert_eq!(found("python asyncio gather"), Some("python"));
        // Part of another word is not a mention.
        assert_eq!(found("reactive programming"), None);
        assert_eq!(found("tokiox"), None);
        // Everyday words are never taken for a library.
        assert_eq!(found("what time is it"), None);
        assert_eq!(found("how to open a file"), None);
        assert_eq!(found("what to do next"), None);
        assert_eq!(found(""), None);
        assert_eq!(mentioned(&[], "tokio"), None);
    }
}
