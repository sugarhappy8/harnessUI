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
        let _ = self;
        unimplemented!()
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

/// The first version number in `text`: `1.38.2` in `^1.38.2`, `4.2` in `>=4.2,<5`, `18` in
/// `18.x`, `1.9.1` in `v1.9.1`. `None` when there is none (`*`, `latest`).
pub fn parse_version(text: &str) -> Option<Version> {
    let _ = text;
    unimplemented!()
}

/// `package.json`, with the text of `package-lock.json` when there is one.
pub fn parse_package_json(manifest: &str, lock: Option<&str>) -> Vec<Library> {
    let _ = (manifest, lock);
    unimplemented!()
}

/// `Cargo.toml`, with the text of `Cargo.lock` when there is one.
pub fn parse_cargo(manifest: &str, lock: Option<&str>) -> Vec<Library> {
    let _ = (manifest, lock);
    unimplemented!()
}

/// `pyproject.toml`: the standard `[project]` tables and Poetry's.
pub fn parse_pyproject(manifest: &str) -> Vec<Library> {
    let _ = manifest;
    unimplemented!()
}

/// `requirements.txt`.
pub fn parse_requirements(text: &str) -> Vec<Library> {
    let _ = text;
    unimplemented!()
}

/// `go.mod`: the modules it requires directly.
pub fn parse_go_mod(text: &str) -> Vec<Library> {
    let _ = text;
    unimplemented!()
}

/// The project's libraries, from whatever manifests `read` can give. `read` gets a file
/// name from [`MANIFESTS`] and returns its text, or `None` when the file is not there or
/// may not be read. A manifest that does not parse is skipped. The result is sorted and
/// has each library once.
pub fn from_manifests(read: &dyn Fn(&str) -> Option<String>) -> Vec<Library> {
    let _ = read;
    unimplemented!()
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

/// The library called `name`, however it is written: letter case, `-` and `_` do not
/// matter, and the last part of a Go module path is enough.
pub fn find_library<'a>(libraries: &'a [Library], name: &str) -> Option<&'a Library> {
    let _ = (libraries, name);
    unimplemented!()
}

/// The library a question is about, when it names one of the project's as whole words.
/// Names that are also everyday words ("time", "open", "next") never count: the model has
/// to pass those as the library. Of several, the longest name wins.
pub fn mentioned<'a>(libraries: &'a [Library], query: &str) -> Option<&'a Library> {
    let _ = (libraries, query);
    unimplemented!()
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
        let v = |major, minor, patch| Some(Version {
            major,
            minor,
            patch,
        });
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
        assert_eq!(lib("tokio", "1.38.2", Ecosystem::Cargo).version_label(), "1.38");
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
        assert_eq!(get(&libs, "github.com/gin-gonic/gin").ecosystem, Ecosystem::Go);
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
        assert_eq!(found("github.com/gin-gonic/gin"), Some("github.com/gin-gonic/gin"));
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
        assert_eq!(found("gin middleware order"), Some("github.com/gin-gonic/gin"));
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
