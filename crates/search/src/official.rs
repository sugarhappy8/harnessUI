//! Official documentation: which sites count as it, and which version a page is for.
//!
//! Three kinds of knowledge, all of them data in this file:
//!
//! - [`DOCUMENTATION_HOSTS`]: sites that publish the documentation of a language, a
//!   platform or a package index (`docs.rs`, `developer.mozilla.org`, `docs.python.org`…).
//! - [`KNOWN_LIBRARIES`]: where well-known libraries keep their own documentation
//!   (`react` at `react.dev`, `django` at `docs.djangoproject.com`…).
//! - For a library of the project, what its package index says: one request to npm, PyPI
//!   or crates.io for the library's documentation, home page and repository addresses
//!   ([`Registry`]). The answer is kept in memory.
//!
//! [`tier`] puts a search result into one of three groups, and the pipeline orders results
//! by group first: the library's own documentation, other official documentation, the
//! rest. [`version_note`] says whether a page is for the version the project uses.

use std::collections::HashMap;
use std::sync::Mutex;

use url::Url;

use crate::fetch::Fetcher;
use crate::versions::{Ecosystem, Library, Version};

/// A place that is official for one library: a whole host, or one part of a host that many
/// projects share (`github.com/tokio-rs/tokio`, `docs.rs/tokio`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfficialSource {
    /// Lower case, without `www.`.
    pub host: String,
    /// The start of the path, without a slash at the end. `None`: the whole host.
    pub path: Option<String>,
}

/// How official a search result is. Lower sorts first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    /// The documentation, home page or repository of the library the question is about.
    Library,
    /// Official documentation of something else (a language, a platform, another package).
    Documentation,
    Other,
}

/// Sites that publish official documentation. A host stands for itself and its `www.`
/// form; a leading `*.` also takes in every host below it.
pub const DOCUMENTATION_HOSTS: &[&str] = &[
    // Rust
    "docs.rs",
    "doc.rust-lang.org",
    "crates.io",
    "rust-lang.org",
    "rust-lang.github.io",
    // JavaScript, TypeScript and the web platform
    "npmjs.com",
    "docs.npmjs.com",
    "nodejs.org",
    "developer.mozilla.org",
    "typescriptlang.org",
    "tc39.es",
    "html.spec.whatwg.org",
    "w3.org",
    "web.dev",
    "developer.chrome.com",
    "webkit.org",
    "deno.com",
    "docs.deno.com",
    "bun.sh",
    // Python
    "docs.python.org",
    "pypi.org",
    "packaging.python.org",
    "peps.python.org",
    // Go
    "pkg.go.dev",
    "go.dev",
    // Platforms and other languages
    "learn.microsoft.com",
    "developer.apple.com",
    "developer.android.com",
    "kotlinlang.org",
    "docs.oracle.com",
    "docs.swift.org",
    "swift.org",
    "dart.dev",
    "api.dart.dev",
    "docs.flutter.dev",
    "php.net",
    "docs.ruby-lang.org",
    "ruby-lang.org",
    "guides.rubyonrails.org",
    "api.rubyonrails.org",
    "elixir-lang.org",
    "hexdocs.pm",
    "docs.scala-lang.org",
    "docs.julialang.org",
    "ziglang.org",
    "isocpp.org",
    "cmake.org",
    "gcc.gnu.org",
    "llvm.org",
    "clang.llvm.org",
    "docs.gradle.org",
    "maven.apache.org",
    "docs.spring.io",
    // Tools and services developers read every day
    "git-scm.com",
    "docs.github.com",
    "docs.gitlab.com",
    "docs.docker.com",
    "kubernetes.io",
    "postgresql.org",
    "sqlite.org",
    "dev.mysql.com",
    "mariadb.com",
    "redis.io",
    "nginx.org",
    "httpd.apache.org",
    "docs.aws.amazon.com",
    "cloud.google.com",
    "docs.unity3d.com",
    "docs.godotengine.org",
    "man7.org",
    "gnu.org",
    // Libraries and frameworks with a site of their own
    "react.dev",
    "vuejs.org",
    "angular.dev",
    "svelte.dev",
    "nextjs.org",
    "tailwindcss.com",
    "vite.dev",
    "tauri.app",
    "*.tauri.app",
    "docs.djangoproject.com",
    "fastapi.tiangolo.com",
    "flask.palletsprojects.com",
    "numpy.org",
    "pandas.pydata.org",
    "pytorch.org",
];

/// Where well-known libraries keep their documentation: package name (lower case), then
/// hosts, each optionally with a path.
pub const KNOWN_LIBRARIES: &[(&str, &[&str])] = &[
    // The languages themselves, as the manifests name them.
    ("node", &["nodejs.org"]),
    ("rust", &["doc.rust-lang.org", "rust-lang.org"]),
    ("python", &["docs.python.org"]),
    ("go", &["go.dev", "pkg.go.dev/std"]),
    // npm
    ("react", &["react.dev", "legacy.reactjs.org"]),
    ("react-dom", &["react.dev", "legacy.reactjs.org"]),
    ("vue", &["vuejs.org"]),
    ("@angular/core", &["angular.dev", "angular.io"]),
    ("svelte", &["svelte.dev"]),
    ("@sveltejs/kit", &["svelte.dev"]),
    ("next", &["nextjs.org"]),
    ("nuxt", &["nuxt.com"]),
    ("astro", &["docs.astro.build", "astro.build"]),
    ("tailwindcss", &["tailwindcss.com"]),
    ("typescript", &["typescriptlang.org"]),
    ("vite", &["vite.dev", "vitejs.dev"]),
    ("vitest", &["vitest.dev"]),
    ("jest", &["jestjs.io"]),
    ("eslint", &["eslint.org"]),
    ("prettier", &["prettier.io"]),
    ("webpack", &["webpack.js.org"]),
    ("rollup", &["rollupjs.org"]),
    ("express", &["expressjs.com"]),
    ("redux", &["redux.js.org"]),
    ("@reduxjs/toolkit", &["redux-toolkit.js.org"]),
    ("react-router", &["reactrouter.com"]),
    ("react-router-dom", &["reactrouter.com"]),
    ("axios", &["axios-http.com"]),
    ("lodash", &["lodash.com"]),
    ("playwright", &["playwright.dev"]),
    ("@playwright/test", &["playwright.dev"]),
    ("electron", &["electronjs.org"]),
    ("three", &["threejs.org"]),
    ("d3", &["d3js.org"]),
    ("zod", &["zod.dev"]),
    (
        "@tauri-apps/api",
        &["tauri.app", "v2.tauri.app", "v1.tauri.app"],
    ),
    (
        "@tauri-apps/cli",
        &["tauri.app", "v2.tauri.app", "v1.tauri.app"],
    ),
    // crates.io
    ("tauri", &["tauri.app", "v2.tauri.app", "v1.tauri.app"]),
    ("tokio", &["tokio.rs"]),
    ("serde", &["serde.rs"]),
    ("actix-web", &["actix.rs"]),
    ("rocket", &["rocket.rs"]),
    ("diesel", &["diesel.rs"]),
    ("bevy", &["bevyengine.org", "bevy.org"]),
    ("hyper", &["hyper.rs"]),
    ("leptos", &["leptos.dev", "book.leptos.dev"]),
    ("yew", &["yew.rs"]),
    // PyPI
    ("django", &["docs.djangoproject.com", "djangoproject.com"]),
    ("djangorestframework", &["django-rest-framework.org"]),
    ("fastapi", &["fastapi.tiangolo.com"]),
    ("flask", &["flask.palletsprojects.com"]),
    ("jinja2", &["jinja.palletsprojects.com"]),
    ("click", &["click.palletsprojects.com"]),
    ("numpy", &["numpy.org"]),
    ("pandas", &["pandas.pydata.org"]),
    ("scipy", &["docs.scipy.org", "scipy.org"]),
    ("matplotlib", &["matplotlib.org"]),
    ("scikit-learn", &["scikit-learn.org"]),
    ("torch", &["pytorch.org", "docs.pytorch.org"]),
    ("tensorflow", &["tensorflow.org"]),
    ("requests", &["requests.readthedocs.io"]),
    ("sqlalchemy", &["docs.sqlalchemy.org", "sqlalchemy.org"]),
    ("pydantic", &["docs.pydantic.dev"]),
    ("pytest", &["docs.pytest.org"]),
    ("celery", &["docs.celeryq.dev"]),
    ("httpx", &["python-httpx.org"]),
    ("aiohttp", &["docs.aiohttp.org"]),
    ("transformers", &["huggingface.co/docs/transformers"]),
    // Go
    ("github.com/gin-gonic/gin", &["gin-gonic.com"]),
    ("github.com/spf13/cobra", &["cobra.dev"]),
    ("github.com/labstack/echo", &["echo.labstack.com"]),
    ("github.com/gofiber/fiber", &["docs.gofiber.io"]),
    ("gorm.io/gorm", &["gorm.io"]),
];

/// Hosts many projects share: an address there stands for one project only together with
/// the start of its path. The number is how many path parts name the project.
const SHARED_HOSTS: &[(&str, usize)] = &[
    ("github.com", 2),
    ("gitlab.com", 2),
    ("bitbucket.org", 2),
    ("codeberg.org", 2),
    ("git.sr.ht", 2),
    ("sourceforge.net", 2),
    ("docs.rs", 1),
    ("crates.io", 2),
    ("npmjs.com", 2),
    ("pypi.org", 2),
    ("pkg.go.dev", usize::MAX),
    ("readthedocs.org", 2),
    ("huggingface.co", 2),
    ("hexdocs.pm", 1),
    ("rubygems.org", 2),
    ("jsr.io", 2),
];

/// Hosts that are never documentation, whatever a package index says: social networks,
/// chat, funding.
const NOT_DOCUMENTATION: &[&str] = &[
    "twitter.com",
    "x.com",
    "facebook.com",
    "linkedin.com",
    "youtube.com",
    "reddit.com",
    "discord.gg",
    "discord.com",
    "t.me",
    "gitter.im",
    "slack.com",
    "opencollective.com",
    "patreon.com",
    "ko-fi.com",
    "buymeacoffee.com",
    "liberapay.com",
    "paypal.com",
    "paypal.me",
    "tidelift.com",
];

/// The documentation hosts of an ecosystem: its language, its package index.
pub fn ecosystem_hosts(ecosystem: Ecosystem) -> &'static [&'static str] {
    match ecosystem {
        Ecosystem::Cargo => &["docs.rs", "doc.rust-lang.org", "crates.io", "rust-lang.org"],
        Ecosystem::Npm => &[
            "npmjs.com",
            "docs.npmjs.com",
            "nodejs.org",
            "developer.mozilla.org",
            "typescriptlang.org",
        ],
        Ecosystem::Pypi => &["docs.python.org", "pypi.org", "packaging.python.org"],
        Ecosystem::Go => &["pkg.go.dev", "go.dev"],
    }
}

/// The host of `url` as it is compared: lower case, without `www.`.
pub fn site_of(url: &Url) -> String {
    let host = url.host_str().unwrap_or("").to_ascii_lowercase();
    let host = host.trim_end_matches('.');
    host.strip_prefix("www.").unwrap_or(host).to_string()
}

/// Whether `site` is the listed `entry`, or below it when the entry starts with `*.`.
fn host_matches(site: &str, entry: &str) -> bool {
    match entry.strip_prefix("*.") {
        Some(parent) => site
            .strip_suffix(parent)
            .is_some_and(|below| below.ends_with('.')),
        None => site == entry,
    }
}

/// A `host` or `host/path` entry of the data as a source.
fn source_from_entry(entry: &str) -> OfficialSource {
    match entry.split_once('/') {
        Some((host, path)) => OfficialSource {
            host: host.to_string(),
            path: Some(format!("/{}", path.trim_end_matches('/'))),
        },
        None => OfficialSource {
            host: entry.to_string(),
            path: None,
        },
    }
}

/// Whether `url` is on one of the [`DOCUMENTATION_HOSTS`], or on the site of one of the
/// [`KNOWN_LIBRARIES`].
pub fn is_documentation_host(url: &Url) -> bool {
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    let site = site_of(url);
    DOCUMENTATION_HOSTS
        .iter()
        .any(|entry| host_matches(&site, entry))
        || KNOWN_LIBRARIES
            .iter()
            .flat_map(|(_, hosts)| hosts.iter())
            .any(|entry| within(url, &source_from_entry(entry)))
}

/// The official source an address stands for: the whole host, or the host and the start of
/// the path when many projects share the host (a repository on GitHub, a package's page).
/// `None` for addresses that are no documentation at all (social networks, funding pages)
/// and for the bare front page of a shared host.
pub fn source_from_url(url: &Url) -> Option<OfficialSource> {
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = site_of(url);
    if host.is_empty()
        || NOT_DOCUMENTATION
            .iter()
            .any(|no| host == *no || host.ends_with(&format!(".{no}")))
    {
        return None;
    }
    let Some((_, parts)) = SHARED_HOSTS.iter().find(|(shared, _)| host == *shared) else {
        return Some(OfficialSource { host, path: None });
    };
    let segments: Vec<&str> = url
        .path_segments()
        .map(|s| s.filter(|part| !part.is_empty()).collect())
        .unwrap_or_default();
    // A sponsorship page on a code host is not the project.
    if segments.first().is_some_and(|first| *first == "sponsors") {
        return None;
    }
    let parts = match *parts {
        usize::MAX => segments.len(),
        // npmjs.com/package/@scope/name: the scope alone is not the package.
        n if host == "npmjs.com" && segments.get(1).is_some_and(|s| s.starts_with('@')) => n + 1,
        n => n,
    };
    if parts == 0 || segments.len() < parts {
        return None;
    }
    Some(OfficialSource {
        host,
        path: Some(format!("/{}", segments[..parts].join("/"))),
    })
}

/// A package's own page on its index, and for a Go module on a code host its repository.
fn index_pages(library: &Library) -> Vec<OfficialSource> {
    let name = library.name.trim();
    let page = |host: &str, path: String| OfficialSource {
        host: host.to_string(),
        path: Some(path),
    };
    match library.ecosystem {
        Ecosystem::Cargo => vec![
            page("docs.rs", format!("/{name}")),
            page("crates.io", format!("/crates/{name}")),
        ],
        Ecosystem::Npm => vec![page("npmjs.com", format!("/package/{name}"))],
        Ecosystem::Pypi => vec![page(
            "pypi.org",
            format!("/project/{}", name.to_lowercase().replace(['_', '.'], "-")),
        )],
        Ecosystem::Go => {
            let mut pages = vec![page("pkg.go.dev", format!("/{name}"))];
            if let Some(repository) = Url::parse(&format!("https://{name}"))
                .ok()
                .as_ref()
                .and_then(source_from_url)
                .filter(|s| s.path.is_some())
            {
                pages.push(repository);
            }
            pages
        }
    }
}

/// Everything that is official for `library`: its page on the package index, its entry in
/// [`KNOWN_LIBRARIES`], and the addresses its package index gave (`from_registry`).
pub fn sources_for(library: &Library, from_registry: &[Url]) -> Vec<OfficialSource> {
    let mut sources: Vec<OfficialSource> = Vec::new();
    let mut add = |source: OfficialSource| {
        if !sources.contains(&source) {
            sources.push(source);
        }
    };
    if !library.is_runtime() {
        index_pages(library).into_iter().for_each(&mut add);
    }
    let name = library.name.trim().to_lowercase();
    for (known, hosts) in KNOWN_LIBRARIES {
        // A language's entry is only for the language, not for a package of that name.
        let is_language = matches!(*known, "node" | "rust" | "python" | "go");
        if *known == name && is_language == library.is_runtime() {
            hosts
                .iter()
                .map(|h| source_from_entry(h))
                .for_each(&mut add);
        }
    }
    from_registry
        .iter()
        .filter_map(source_from_url)
        .for_each(&mut add);
    sources
}

/// Whether `url` is inside `source`.
pub fn within(url: &Url, source: &OfficialSource) -> bool {
    if !matches!(url.scheme(), "http" | "https") || site_of(url) != source.host {
        return false;
    }
    let Some(prefix) = &source.path else {
        return true;
    };
    // Names on package indexes and code hosts do not depend on letter case, and "_" and
    // "-" are the same there.
    let fold = |s: &str| s.to_lowercase().replace('_', "-");
    let path = fold(url.path());
    let prefix = fold(prefix);
    path == prefix
        || path
            .strip_prefix(&prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// The group a result belongs to, given what is official for the library in question
/// (empty when the question is about no library).
pub fn tier(url: &Url, library_sources: &[OfficialSource]) -> Tier {
    if library_sources.iter().any(|source| within(url, source)) {
        Tier::Library
    } else if is_documentation_host(url) {
        Tier::Documentation
    } else {
        Tier::Other
    }
}

/// The three package indexes, in the order of [`Registry::with_test_bases`].
const INDEXES: [&str; 3] = [
    "https://registry.npmjs.org/",
    "https://pypi.org/pypi/",
    "https://crates.io/api/v1/crates/",
];

/// Whether `name` can be a package name of the ecosystem. Anything else is never put into
/// an address.
fn valid_package_name(name: &str, ecosystem: Ecosystem) -> bool {
    let plain = |s: &str| {
        !s.is_empty()
            && s.len() <= 214
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~'))
            && !s.starts_with('.')
            && !s.contains("..")
    };
    match ecosystem {
        Ecosystem::Npm => match name.strip_prefix('@') {
            Some(scoped) => scoped
                .split_once('/')
                .is_some_and(|(scope, package)| plain(scope) && plain(package)),
            None => plain(name),
        },
        Ecosystem::Pypi => plain(name) && !name.contains('~'),
        Ecosystem::Cargo => {
            !name.is_empty()
                && name.len() <= 64
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        }
        Ecosystem::Go => false,
    }
}

/// The address to ask about `library` below the three index addresses in `bases`.
fn registry_url_in(library: &Library, bases: &[Url; 3]) -> Option<Url> {
    let name = library.name.trim();
    if library.is_runtime() || !valid_package_name(name, library.ecosystem) {
        return None;
    }
    let (base, parts): (&Url, Vec<String>) = match library.ecosystem {
        Ecosystem::Npm => {
            let mut parts: Vec<String> = name.split('/').map(str::to_string).collect();
            parts.push("latest".into());
            (&bases[0], parts)
        }
        Ecosystem::Pypi => {
            let name = name.to_lowercase().replace(['_', '.'], "-");
            // An exact version: only that release is asked for (see the test).
            let exact = library
                .parsed()
                .filter(|v| v.minor.is_some() && v.patch.is_some())
                .filter(|_| {
                    library
                        .version
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '.')
                });
            match exact {
                Some(_) => (
                    &bases[1],
                    vec![name, library.version.clone(), "json".into()],
                ),
                None => (&bases[1], vec![name, "json".into()]),
            }
        }
        Ecosystem::Cargo => (&bases[2], vec![name.to_string()]),
        Ecosystem::Go => return None,
    };
    let mut url = base.clone();
    {
        let mut segments = url.path_segments_mut().ok()?;
        segments.pop_if_empty();
        segments.extend(parts.iter().map(String::as_str));
    }
    Some(url)
}

/// The address to ask a package index about `library`, or `None` where there is no index
/// to ask (Go, and the languages themselves).
pub fn registry_url(library: &Library) -> Option<Url> {
    let bases = INDEXES.map(|base| Url::parse(base).ok());
    let [Some(npm), Some(pypi), Some(crates)] = bases else {
        return None;
    };
    registry_url_in(library, &[npm, pypi, crates])
}

/// A repository or home page address as an https address: `git+https://…​.git`,
/// `git://`, `git@host:owner/repo.git` and npm's `github:owner/repo` are all understood.
/// Anything that is not a web address in the end is dropped.
fn web_address(raw: &str) -> Option<Url> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > 500 {
        return None;
    }
    let mut text = raw.strip_prefix("git+").unwrap_or(raw).to_string();
    for (short, host) in [
        ("github:", "github.com"),
        ("gitlab:", "gitlab.com"),
        ("bitbucket:", "bitbucket.org"),
    ] {
        if let Some(path) = text.strip_prefix(short) {
            text = format!("https://{host}/{path}");
        }
    }
    // git@github.com:owner/repo.git
    if let Some(rest) = text.strip_prefix("git@")
        && let Some((host, path)) = rest.split_once(':')
    {
        text = format!("https://{host}/{path}");
    }
    for scheme in ["git://", "ssh://git@", "ssh://"] {
        if let Some(rest) = text.strip_prefix(scheme) {
            text = format!("https://{rest}");
        }
    }
    let mut url = Url::parse(&text).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return None;
    }
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_fragment(None);
    if let Some(path) = url.path().strip_suffix(".git").map(str::to_string) {
        url.set_path(&path);
    }
    Some(url)
}

/// Collects addresses in order, each once.
fn collect(addresses: impl IntoIterator<Item = Option<String>>) -> Vec<Url> {
    let mut urls: Vec<Url> = Vec::new();
    for url in addresses
        .into_iter()
        .flatten()
        .filter_map(|a| web_address(&a))
    {
        if !urls.contains(&url) {
            urls.push(url);
        }
    }
    urls
}

fn text_at(value: &serde_json::Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(str::to_string)
}

/// The documentation, home page and repository addresses in npm's answer for one version
/// (`registry.npmjs.org/<name>/latest`).
pub fn parse_npm(json: &str) -> Vec<Url> {
    let Ok(root) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let repository = match root.get("repository") {
        Some(serde_json::Value::String(short)) => {
            // "owner/repo" alone means GitHub.
            let known = ["github:", "gitlab:", "bitbucket:"]
                .iter()
                .any(|p| short.starts_with(p));
            if known || short.contains("://") || short.starts_with("git@") {
                Some(short.clone())
            } else if short.split('/').count() == 2 {
                Some(format!("github:{short}"))
            } else {
                None
            }
        }
        Some(table) => text_at(table, "url"),
        None => None,
    };
    collect([text_at(&root, "homepage"), repository])
}

/// PyPI's answer for a whole project lists every file of every release and can be longer
/// than the fetcher reads. Its `info` object comes first, so it is read from the start of
/// an answer that was cut off.
fn pypi_info_of_cut_answer(json: &str) -> Option<serde_json::Value> {
    let start = json.trim_start().strip_prefix('{')?.trim_start();
    let rest = start
        .strip_prefix("\"info\"")?
        .trim_start()
        .strip_prefix(':')?;
    serde_json::Deserializer::from_str(rest)
        .into_iter::<serde_json::Value>()
        .next()?
        .ok()
}

/// The same from PyPI's answer (`pypi.org/pypi/<name>/json`).
pub fn parse_pypi(json: &str) -> Vec<Url> {
    let info = match serde_json::from_str::<serde_json::Value>(json) {
        Ok(root) => root.get("info").cloned(),
        Err(_) => pypi_info_of_cut_answer(json),
    };
    let Some(info) = &info else {
        return Vec::new();
    };
    let mut addresses = Vec::new();
    if let Some(serde_json::Value::Object(urls)) = info.get("project_urls") {
        // Documentation first, whatever order the project wrote them in.
        let rank = |label: &str| {
            let label = label.to_lowercase();
            if label.contains("doc") {
                0
            } else if label.contains("home") {
                1
            } else {
                2
            }
        };
        let mut labelled: Vec<(&String, &serde_json::Value)> = urls.iter().collect();
        labelled.sort_by_key(|(label, _)| rank(label));
        for (label, value) in labelled {
            let label = label.to_lowercase();
            let unwanted = [
                "fund", "sponsor", "donat", "tracker", "issue", "bug", "twitter", "mastodon",
                "chat", "discord", "slack", "forum", "download",
            ];
            if unwanted.iter().any(|word| label.contains(word)) {
                continue;
            }
            addresses.push(value.as_str().map(str::to_string));
        }
    }
    addresses.push(text_at(info, "docs_url"));
    addresses.push(text_at(info, "home_page"));
    collect(addresses)
}

/// The same from crates.io's answer (`crates.io/api/v1/crates/<name>`).
pub fn parse_crates(json: &str) -> Vec<Url> {
    let Ok(root) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    let Some(krate) = root.get("crate") else {
        return Vec::new();
    };
    collect([
        text_at(krate, "documentation"),
        text_at(krate, "homepage"),
        text_at(krate, "repository"),
    ])
}

/// What package indexes said about libraries, kept for as long as the app runs.
#[derive(Debug, Default)]
pub struct Registry {
    cache: Mutex<HashMap<(Ecosystem, String), Vec<Url>>>,
    /// Tests only: where the three indexes are asked instead (npm, PyPI, crates.io).
    #[cfg(feature = "test-hooks")]
    bases: Option<[Url; 3]>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Tests only: a registry that asks these addresses instead of npm, PyPI and
    /// crates.io.
    #[cfg(feature = "test-hooks")]
    pub fn with_test_bases(npm: Url, pypi: Url, crates: Url) -> Self {
        Self {
            cache: Mutex::default(),
            bases: Some([npm, pypi, crates]),
        }
    }

    fn address(&self, library: &Library) -> Option<Url> {
        #[cfg(feature = "test-hooks")]
        if let Some(bases) = &self.bases {
            return registry_url_in(library, bases);
        }
        registry_url(library)
    }

    /// The addresses the package index gives for `library`. One request, through the
    /// guarded fetcher; the answer (also an empty one) is remembered. A failure gives
    /// nothing and is asked again next time.
    pub async fn documentation_urls(&self, fetcher: &Fetcher, library: &Library) -> Vec<Url> {
        let key = (library.ecosystem, library.name.trim().to_lowercase());
        if let Ok(cache) = self.cache.lock()
            && let Some(known) = cache.get(&key)
        {
            return known.clone();
        }
        let Some(address) = self.address(library) else {
            return Vec::new();
        };
        let answer = match fetcher.get(&address, crate::fetch::Accept::Json).await {
            Ok(answer) => answer,
            Err(error) => {
                tracing::debug!(library = %library.name, "the package index gave no answer: {error}");
                return Vec::new();
            }
        };
        let urls = match library.ecosystem {
            Ecosystem::Npm => parse_npm(&answer.text),
            Ecosystem::Pypi => parse_pypi(&answer.text),
            Ecosystem::Cargo => parse_crates(&answer.text),
            Ecosystem::Go => Vec::new(),
        };
        if let Ok(mut cache) = self.cache.lock() {
            if cache.len() > 500 {
                cache.clear();
            }
            cache.insert(key, urls.clone());
        }
        urls
    }
}

/// A version named somewhere, as far as it was named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Named {
    major: u64,
    minor: Option<u64>,
}

impl Named {
    fn label(self) -> String {
        match self.minor {
            Some(minor) => format!("{}.{minor}", self.major),
            None => self.major.to_string(),
        }
    }

    /// The same version as the project's, as far as it was named.
    fn is(self, project: Version) -> bool {
        self.major == project.major
            && (self.minor.is_none() || project.minor.is_none() || self.minor == project.minor)
    }
}

/// A run of one to four digits at the start of `text`, and the rest.
fn number(text: &str) -> Option<(u64, &str)> {
    let end = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    if end == 0 || end > 4 {
        return None;
    }
    Some((text[..end].parse().ok()?, &text[end..]))
}

/// What follows an `x` or `*` that stands for "any" in a version (`17.x`).
fn after_wildcard(text: &str) -> Option<&str> {
    text.strip_prefix(['x', 'X', '*'])
}

/// A version written out: `1.38.0`, `4.2`, `v18`, `17.x`. A bare number (`18`, `2024`)
/// is not one: an address is full of bare numbers.
fn named_version(text: &str) -> Option<Named> {
    let (marked, rest) = match text.strip_prefix(['v', 'V']) {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (major, mut rest) = number(rest)?;
    let mut named = Named { major, minor: None };
    let mut dotted = false;
    if let Some(after) = rest.strip_prefix('.') {
        if let Some((minor, after)) = number(after) {
            named.minor = Some(minor);
            rest = after;
            // A third part belongs to the version; what it says is not needed.
            if let Some(after) = rest.strip_prefix('.') {
                rest = match number(after) {
                    Some((_, after)) => after,
                    None => after_wildcard(after)?,
                };
            }
        } else {
            rest = after_wildcard(after)?;
            if let Some(after) = rest.strip_prefix('.') {
                rest = after_wildcard(after)?;
            }
        }
        dotted = true;
    }
    if !dotted && !marked {
        return None;
    }
    // "1.2rc1" is a version; "v1beta" and "1.2.3.4" are something else.
    if rest.starts_with(|c: char| c.is_ascii_digit() || c == '.') {
        return None;
    }
    if !dotted && rest.starts_with(|c: char| c.is_alphanumeric()) {
        return None;
    }
    Some(named)
}

/// The version an address names, when it is the library's own documentation: a part of
/// the path (`/1.38.0/`, `/en/4.2/`, `/v18/`, `/latest-v20.x/`) or of the host
/// (`v2.tauri.app`).
fn version_in_address(url: &Url) -> Option<Named> {
    if let Some(first) = url.host_str().and_then(|h| h.split('.').next())
        && first.starts_with(['v', 'V'])
        && let Some(named) = named_version(first)
    {
        return Some(named);
    }
    url.path_segments()?.find_map(|segment| {
        // "latest-v20.x", "release-1.26": the version is what follows the last "-" or "_".
        let tail = segment.rsplit(['-', '_']).next().unwrap_or(segment);
        named_version(tail).or_else(|| named_version(segment))
    })
}

/// The version in a docs.rs address of exactly this crate: `docs.rs/<crate>/<version>/`.
fn docs_rs_version(url: &Url, library: &Library) -> Option<Option<Named>> {
    if library.ecosystem != Ecosystem::Cargo || site_of(url) != "docs.rs" {
        return None;
    }
    let mut segments = url.path_segments()?;
    let fold = |s: &str| s.to_lowercase().replace('_', "-");
    if fold(segments.next()?) != fold(library.name.trim()) {
        return None;
    }
    Some(segments.next().and_then(named_version))
}

/// Versions that follow the library's name in `text`: "Django 4.2", "tokio v1.38.0",
/// "NumPy version 2.1".
fn versions_after_name(text: &str, name: &str) -> Vec<Named> {
    let lower = text.to_lowercase();
    let name = name.to_lowercase();
    let mut found = Vec::new();
    // A name of one letter would be found in every other word.
    if name.chars().count() < 2 {
        return found;
    }
    let mut from = 0;
    while let Some(at) = lower[from..].find(&name) {
        let start = from + at;
        let end = start + name.len();
        from = end;
        let before = lower[..start].chars().next_back();
        if before.is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '/' | '@')) {
            continue;
        }
        if lower[end..].starts_with(|c: char| c.is_alphanumeric() || c == '_') {
            continue;
        }
        // "Vue.js 3.4", "Next.js 14".
        let after = lower[end..].strip_prefix(".js").unwrap_or(&lower[end..]);
        let mut rest = after.trim_start_matches([' ', '\t', ':', ',', '-']);
        for word in ["version", "release"] {
            if let Some(after) = rest.strip_prefix(word) {
                rest = after.trim_start();
            }
        }
        let candidate: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '.')
            .collect();
        let candidate = candidate.trim_end_matches('.');
        // After a name, a bare number is a version too ("React 18").
        let named = named_version(candidate).or_else(|| {
            // Four digits are a year ("Vue 2024 conference"), not a version.
            number(candidate)
                .filter(|(major, rest)| rest.is_empty() && *major < 1000)
                .map(|(major, _)| Named { major, minor: None })
        });
        if let Some(named) = named {
            found.push(named);
        }
    }
    found
}

/// How far into a page's text a version next to the library's name still counts as the
/// page's heading.
const HEADING_CHARS: usize = 600;

/// Whether a page is for the version of `library` the project uses: `matches tokio 1.38`
/// when its address or its heading names that version, `for tokio 0.2` when it names
/// another, `None` when it names none (or the project's version is not known).
///
/// A version in the address counts only on the library's own documentation
/// (`official_for_library`): `docs.rs/tokio/1.38.0/`, `/en/4.2/`, `/v18/`, `/4.x/`,
/// `v2.tauri.app`. Elsewhere a path like `/api/v1/` says nothing about the library. In the
/// title and the first lines of the page, the version has to follow the library's name
/// ("Django 4.2 documentation").
pub fn version_note(
    url: &Url,
    title: &str,
    text: &str,
    library: &Library,
    official_for_library: bool,
) -> Option<String> {
    let project = library.parsed()?;
    let name = library.short_name();
    let say = |named: Named| {
        if named.is(project) {
            format!("matches {name} {}", project.label())
        } else {
            format!("for {name} {}", named.label())
        }
    };
    // The address first: it is the most deliberate statement of a version.
    let in_address = match docs_rs_version(url, library) {
        Some(named) => named,
        None if official_for_library => version_in_address(url),
        None => None,
    };
    if let Some(named) = in_address {
        return Some(say(named));
    }
    let start: String = text.chars().take(HEADING_CHARS).collect();
    let mut named = versions_after_name(title, name);
    named.extend(versions_after_name(&start, name));
    let chosen = named
        .iter()
        .copied()
        .find(|n| n.is(project))
        .or_else(|| named.first().copied())?;
    Some(say(chosen))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    fn lib(name: &str, version: &str, ecosystem: Ecosystem) -> Library {
        Library {
            name: name.into(),
            version: version.into(),
            ecosystem,
        }
    }

    fn src(host: &str, path: Option<&str>) -> OfficialSource {
        OfficialSource {
            host: host.into(),
            path: path.map(str::to_string),
        }
    }

    /// The hosts the 0.0.4 plan names are all in the data.
    #[test]
    fn the_documentation_hosts_are_listed() {
        for host in [
            "docs.rs",
            "doc.rust-lang.org",
            "crates.io",
            "npmjs.com",
            "nodejs.org",
            "developer.mozilla.org",
            "docs.python.org",
            "pypi.org",
            "pkg.go.dev",
            "go.dev",
            "learn.microsoft.com",
            "react.dev",
            "vuejs.org",
            "angular.dev",
            "svelte.dev",
            "nextjs.org",
            "tailwindcss.com",
            "typescriptlang.org",
            "vite.dev",
            "tauri.app",
            "docs.djangoproject.com",
            "fastapi.tiangolo.com",
            "flask.palletsprojects.com",
            "numpy.org",
            "pandas.pydata.org",
            "pytorch.org",
            "kotlinlang.org",
            "docs.oracle.com",
            "developer.apple.com",
            "developer.android.com",
        ] {
            assert!(
                is_documentation_host(&url(&format!("https://{host}/some/page"))),
                "{host}"
            );
        }
        assert!(
            DOCUMENTATION_HOSTS.len() >= 60,
            "{}",
            DOCUMENTATION_HOSTS.len()
        );
        // The data is well formed: lower case, no scheme, no path, no duplicates.
        let mut seen = std::collections::BTreeSet::new();
        for host in DOCUMENTATION_HOSTS {
            let bare = host.trim_start_matches("*.");
            assert!(
                !bare.is_empty()
                    && bare.chars().all(|c| c.is_ascii_lowercase()
                        || c.is_ascii_digit()
                        || c == '.'
                        || c == '-'),
                "{host}"
            );
            assert!(!bare.starts_with("www."), "{host}");
            assert!(seen.insert(*host), "{host} is listed twice");
        }
        for (name, hosts) in KNOWN_LIBRARIES {
            assert_eq!(*name, name.to_lowercase(), "{name}");
            assert!(!hosts.is_empty(), "{name}");
            for h in *hosts {
                assert!(url(&format!("https://{h}")).host_str().is_some(), "{h}");
            }
        }
    }

    #[test]
    fn hosts_match_exactly_with_www_and_below_a_star() {
        assert!(is_documentation_host(&url("https://docs.rs/tokio")));
        assert!(is_documentation_host(&url(
            "https://www.npmjs.com/package/react"
        )));
        assert!(is_documentation_host(&url("http://DOCS.RS/x")));
        // A host that only ends the same way is another site.
        assert!(!is_documentation_host(&url("https://evildocs.rs/tokio")));
        assert!(!is_documentation_host(&url(
            "https://docs.rs.example.com/tokio"
        )));
        assert!(!is_documentation_host(&url("https://example.com/docs.rs")));
        assert!(!is_documentation_host(&url(
            "https://stackoverflow.com/q/1"
        )));
        assert!(!is_documentation_host(&url("https://medium.com/@x/post")));
        assert_eq!(
            site_of(&url("https://www.Example.com:8443/a")),
            "example.com"
        );
        assert_eq!(site_of(&url("https://docs.rs/")), "docs.rs");
    }

    #[test]
    fn every_ecosystem_has_its_hosts() {
        assert!(ecosystem_hosts(Ecosystem::Cargo).contains(&"docs.rs"));
        assert!(ecosystem_hosts(Ecosystem::Cargo).contains(&"doc.rust-lang.org"));
        assert!(ecosystem_hosts(Ecosystem::Npm).contains(&"npmjs.com"));
        assert!(ecosystem_hosts(Ecosystem::Npm).contains(&"developer.mozilla.org"));
        assert!(ecosystem_hosts(Ecosystem::Pypi).contains(&"docs.python.org"));
        assert!(ecosystem_hosts(Ecosystem::Go).contains(&"pkg.go.dev"));
        for e in [
            Ecosystem::Npm,
            Ecosystem::Cargo,
            Ecosystem::Pypi,
            Ecosystem::Go,
        ] {
            for host in ecosystem_hosts(e) {
                assert!(DOCUMENTATION_HOSTS.contains(host), "{host}");
            }
        }
    }

    #[test]
    fn an_address_stands_for_a_host_or_for_part_of_a_shared_one() {
        assert_eq!(
            source_from_url(&url("https://tokio.rs/tokio/tutorial")),
            Some(src("tokio.rs", None))
        );
        assert_eq!(
            source_from_url(&url("https://www.django-rest-framework.org/")),
            Some(src("django-rest-framework.org", None))
        );
        // A repository: only that repository, not all of GitHub.
        assert_eq!(
            source_from_url(&url("https://github.com/tokio-rs/tokio/tree/master/tokio")),
            Some(src("github.com", Some("/tokio-rs/tokio")))
        );
        assert_eq!(
            source_from_url(&url("https://gitlab.com/group/project")),
            Some(src("gitlab.com", Some("/group/project")))
        );
        assert_eq!(source_from_url(&url("https://github.com/tokio-rs")), None);
        assert_eq!(source_from_url(&url("https://github.com/")), None);
        // A package's page on a package index, a project on a hosting site.
        assert_eq!(
            source_from_url(&url("https://docs.rs/tokio/latest/tokio/")),
            Some(src("docs.rs", Some("/tokio")))
        );
        assert_eq!(
            source_from_url(&url("https://pypi.org/project/Django/")),
            Some(src("pypi.org", Some("/project/Django")))
        );
        assert_eq!(
            source_from_url(&url("https://requests.readthedocs.io/en/latest/")),
            Some(src("requests.readthedocs.io", None))
        );
        assert_eq!(
            source_from_url(&url(
                "https://www.npmjs.com/package/@tauri-apps/api?activeTab=readme"
            )),
            Some(src("npmjs.com", Some("/package/@tauri-apps/api")))
        );
        assert_eq!(
            source_from_url(&url("https://www.npmjs.com/package/@tauri-apps")),
            None
        );
        assert_eq!(
            source_from_url(&url("https://pkg.go.dev/github.com/gin-gonic/gin")),
            Some(src("pkg.go.dev", Some("/github.com/gin-gonic/gin")))
        );
        assert_eq!(
            source_from_url(&url("https://huggingface.co/docs/transformers/index")),
            Some(src("huggingface.co", Some("/docs/transformers")))
        );
        assert_eq!(
            source_from_url(&url("https://github.com/sponsors/someone")),
            None
        );
        // Not documentation.
        assert_eq!(source_from_url(&url("https://twitter.com/tokio_rs")), None);
        assert_eq!(source_from_url(&url("https://opencollective.com/x")), None);
        assert_eq!(source_from_url(&url("ftp://example.com/x")), None);
    }

    #[test]
    fn what_is_official_for_a_library() {
        let tokio = lib("tokio", "1.38.2", Ecosystem::Cargo);
        let sources = sources_for(
            &tokio,
            &[
                url("https://tokio.rs"),
                url("https://github.com/tokio-rs/tokio"),
                url("https://docs.rs/tokio"),
            ],
        );
        assert!(sources.contains(&src("docs.rs", Some("/tokio"))));
        assert!(sources.contains(&src("crates.io", Some("/crates/tokio"))));
        assert!(sources.contains(&src("tokio.rs", None)));
        assert!(sources.contains(&src("github.com", Some("/tokio-rs/tokio"))));
        // Each once.
        let mut unique = sources.clone();
        unique.dedup();
        assert_eq!(sources.len(), unique.len());

        let react = sources_for(&lib("react", "18.3.1", Ecosystem::Npm), &[]);
        assert!(react.contains(&src("react.dev", None)), "{react:?}");
        assert!(react.contains(&src("npmjs.com", Some("/package/react"))));
        let django = sources_for(&lib("Django", "4.2", Ecosystem::Pypi), &[]);
        assert!(django.contains(&src("docs.djangoproject.com", None)));
        assert!(django.contains(&src("pypi.org", Some("/project/django"))));
        let gin = sources_for(
            &lib("github.com/gin-gonic/gin", "1.9.1", Ecosystem::Go),
            &[],
        );
        assert!(gin.contains(&src("pkg.go.dev", Some("/github.com/gin-gonic/gin"))));
        assert!(gin.contains(&src("github.com", Some("/gin-gonic/gin"))));
        let python = sources_for(&lib("python", "3.11", Ecosystem::Pypi), &[]);
        assert!(python.contains(&src("docs.python.org", None)));
    }

    #[test]
    fn results_fall_into_three_groups() {
        let tokio = sources_for(
            &lib("tokio", "1.38.2", Ecosystem::Cargo),
            &[
                url("https://tokio.rs"),
                url("https://github.com/tokio-rs/tokio"),
                url("https://tokio-book.example/"),
            ],
        );
        let t = |u: &str| tier(&url(u), &tokio);
        assert_eq!(t("https://tokio-book.example/chapter/1"), Tier::Library);
        assert_eq!(t("https://docs.rs/tokio/1.38.0/tokio/"), Tier::Library);
        assert_eq!(t("https://docs.rs/TOKIO/latest"), Tier::Library);
        assert_eq!(t("https://tokio.rs/tokio/tutorial/spawning"), Tier::Library);
        assert_eq!(t("https://www.tokio.rs/blog"), Tier::Library);
        assert_eq!(
            t("https://github.com/tokio-rs/tokio/issues/1"),
            Tier::Library
        );
        assert_eq!(t("https://crates.io/crates/tokio"), Tier::Library);
        // Official documentation, but of something else.
        assert_eq!(
            t("https://docs.rs/serde/latest/serde/"),
            Tier::Documentation
        );
        assert_eq!(t("https://docs.rs/tokio-util/latest/"), Tier::Documentation);
        assert_eq!(
            t("https://doc.rust-lang.org/std/thread/"),
            Tier::Documentation
        );
        assert_eq!(
            t("https://developer.mozilla.org/en-US/docs/Web"),
            Tier::Documentation
        );
        // The rest, including other repositories on the same shared host.
        assert_eq!(t("https://github.com/someone/tokio-examples"), Tier::Other);
        assert_eq!(t("https://github.com/tokio-rs/tokio-evil"), Tier::Other);
        assert_eq!(t("https://stackoverflow.com/questions/1"), Tier::Other);
        assert_eq!(t("https://tokio.rs.example.com/"), Tier::Other);
        // No library in the question: only what the app knows by itself counts.
        assert_eq!(
            tier(&url("https://tokio-book.example/chapter/1"), &[]),
            Tier::Other
        );
        assert_eq!(
            tier(&url("https://docs.rs/tokio"), &[]),
            Tier::Documentation
        );
        // The site of a well-known library is official documentation whatever was asked.
        assert_eq!(tier(&url("https://tokio.rs/"), &[]), Tier::Documentation);
        assert_eq!(
            tier(&url("https://react.dev/learn"), &tokio),
            Tier::Documentation
        );
        assert!(Tier::Library < Tier::Documentation && Tier::Documentation < Tier::Other);
    }

    #[test]
    fn where_a_package_index_is_asked() {
        let ask = |l: &Library| registry_url(l).map(|u| u.to_string());
        assert_eq!(
            ask(&lib("react", "18.3.1", Ecosystem::Npm)).as_deref(),
            Some("https://registry.npmjs.org/react/latest")
        );
        assert_eq!(
            ask(&lib("@tauri-apps/api", "2.12.4", Ecosystem::Npm)).as_deref(),
            Some("https://registry.npmjs.org/@tauri-apps/api/latest")
        );
        // With an exact version, PyPI is asked about that release only: the answer for a
        // whole project lists every file of every release and can be several megabytes.
        assert_eq!(
            ask(&lib("Django", "4.2.11", Ecosystem::Pypi)).as_deref(),
            Some("https://pypi.org/pypi/django/4.2.11/json")
        );
        assert_eq!(
            ask(&lib("Typing_Extensions", "4.2", Ecosystem::Pypi)).as_deref(),
            Some("https://pypi.org/pypi/typing-extensions/json")
        );
        assert_eq!(
            ask(&lib("numpy", "", Ecosystem::Pypi)).as_deref(),
            Some("https://pypi.org/pypi/numpy/json")
        );
        assert_eq!(
            ask(&lib("serde_json", "1.0.117", Ecosystem::Cargo)).as_deref(),
            Some("https://crates.io/api/v1/crates/serde_json")
        );
        assert_eq!(
            ask(&lib("github.com/gin-gonic/gin", "1.9.1", Ecosystem::Go)),
            None
        );
        // The languages themselves are not packages.
        assert_eq!(ask(&lib("node", "20.11", Ecosystem::Npm)), None);
        assert_eq!(ask(&lib("python", "3.11", Ecosystem::Pypi)), None);
        assert_eq!(ask(&lib("rust", "1.75", Ecosystem::Cargo)), None);
        // A name that is no package name is never put into an address.
        for bad in [
            "../../etc",
            "a b",
            "x?y=1",
            "a/b/c",
            "",
            "@scope/",
            "name#frag",
            "%2e%2e",
        ] {
            assert_eq!(ask(&lib(bad, "1", Ecosystem::Npm)), None, "{bad}");
            assert_eq!(ask(&lib(bad, "1", Ecosystem::Pypi)), None, "{bad}");
            assert_eq!(ask(&lib(bad, "1", Ecosystem::Cargo)), None, "{bad}");
        }
    }

    #[test]
    fn package_index_answers() {
        let npm = include_str!("../tests/fixtures/npm_react_latest.json");
        assert_eq!(
            parse_npm(npm),
            [
                url("https://react.dev/"),
                url("https://github.com/facebook/react")
            ]
        );
        let pypi = include_str!("../tests/fixtures/pypi_django.json");
        let got = parse_pypi(pypi);
        assert!(
            got.contains(&url("https://docs.djangoproject.com/")),
            "{got:?}"
        );
        assert!(
            got.contains(&url("https://www.djangoproject.com/")),
            "{got:?}"
        );
        assert!(
            got.contains(&url("https://github.com/django/django")),
            "{got:?}"
        );
        assert!(
            !got.iter().any(|u| u.as_str().contains("fundraising")),
            "a funding page is not documentation: {got:?}"
        );
        // An answer that was cut off at the fetcher's size limit still gives its start.
        let cut = &pypi[..pypi.find("\"last_serial\"").unwrap() + 20];
        assert!(serde_json::from_str::<serde_json::Value>(cut).is_err());
        assert_eq!(parse_pypi(cut), got);
        assert!(parse_pypi(&pypi[..200]).is_empty());
        let crates = include_str!("../tests/fixtures/crates_tokio.json");
        assert_eq!(
            parse_crates(crates),
            [
                url("https://docs.rs/tokio"),
                url("https://tokio.rs/"),
                url("https://github.com/tokio-rs/tokio")
            ]
        );
        // Anything else is no answer, not a panic.
        for bad in [
            "",
            "not json",
            "[]",
            "{\"crate\": 3}",
            "{\"info\": {\"project_urls\": 7}}",
        ] {
            assert!(parse_npm(bad).is_empty(), "{bad}");
            assert!(parse_pypi(bad).is_empty(), "{bad}");
            assert!(parse_crates(bad).is_empty(), "{bad}");
        }
        // Repository shorthands and git addresses become https addresses; other schemes
        // are dropped.
        let odd = r#"{"homepage": "javascript:alert(1)",
                      "repository": "github:user/repo"}"#;
        assert_eq!(parse_npm(odd), [url("https://github.com/user/repo")]);
        let ssh = r#"{"repository": {"url": "git+ssh://git@github.com/user/repo.git"}}"#;
        assert_eq!(parse_npm(ssh), [url("https://github.com/user/repo")]);
    }

    fn note(u: &str, title: &str, text: &str, library: &Library) -> Option<String> {
        version_note(&url(u), title, text, library, true)
    }

    #[test]
    fn a_version_in_the_address() {
        let tokio = lib("tokio", "1.38.2", Ecosystem::Cargo);
        assert_eq!(
            note(
                "https://docs.rs/tokio/1.38.0/tokio/task/fn.spawn_blocking.html",
                "",
                "",
                &tokio
            )
            .as_deref(),
            Some("matches tokio 1.38")
        );
        assert_eq!(
            note("https://docs.rs/tokio/0.2.25/tokio/", "", "", &tokio).as_deref(),
            Some("for tokio 0.2")
        );
        assert_eq!(
            note("https://docs.rs/tokio/1.44.1/tokio/", "", "", &tokio).as_deref(),
            Some("for tokio 1.44")
        );
        assert_eq!(
            note("https://docs.rs/tokio/latest/tokio/", "", "", &tokio),
            None
        );

        let django = lib("django", "4.2.11", Ecosystem::Pypi);
        assert_eq!(
            note(
                "https://docs.djangoproject.com/en/4.2/topics/forms/",
                "",
                "",
                &django
            )
            .as_deref(),
            Some("matches django 4.2")
        );
        assert_eq!(
            note(
                "https://docs.djangoproject.com/en/5.0/topics/forms/",
                "",
                "",
                &django
            )
            .as_deref(),
            Some("for django 5.0")
        );
        assert_eq!(
            note(
                "https://docs.djangoproject.com/en/stable/topics/forms/",
                "",
                "",
                &django
            ),
            None
        );

        let react = lib("react", "18.3.1", Ecosystem::Npm);
        assert_eq!(
            note("https://react.dev/v18/reference", "", "", &react).as_deref(),
            Some("matches react 18.3")
        );
        assert_eq!(
            note("https://legacy.reactjs.org/docs/17.x/hooks", "", "", &react).as_deref(),
            Some("for react 17")
        );
        let node = lib("node", "20.11", Ecosystem::Npm);
        assert_eq!(
            note(
                "https://nodejs.org/docs/latest-v20.x/api/fs.html",
                "",
                "",
                &node
            )
            .as_deref(),
            Some("matches node 20.11")
        );
        let tauri = lib("@tauri-apps/api", "2.12.4", Ecosystem::Npm);
        assert_eq!(
            note("https://v2.tauri.app/start/", "", "", &tauri).as_deref(),
            Some("matches @tauri-apps/api 2.12")
        );
        assert_eq!(
            note("https://v1.tauri.app/v1/guides/", "", "", &tauri).as_deref(),
            Some("for @tauri-apps/api 1")
        );
        let python = lib("python", "3.12", Ecosystem::Pypi);
        assert_eq!(
            note(
                "https://docs.python.org/3.12/library/asyncio.html",
                "",
                "",
                &python
            )
            .as_deref(),
            Some("matches python 3.12")
        );
        // A bare number, a year and a date are not versions.
        assert_eq!(
            note(
                "https://docs.python.org/3/library/asyncio.html",
                "",
                "",
                &python
            ),
            None
        );
        assert_eq!(
            note("https://blog.example.com/2024/05/12/post", "", "", &python),
            None
        );
        assert_eq!(
            note("https://example.com/posts/18/comments", "", "", &react),
            None
        );
    }

    #[test]
    fn a_version_in_the_address_counts_only_on_the_librarys_own_documentation() {
        let react = lib("react", "18.3.1", Ecosystem::Npm);
        let elsewhere = |u: &str| version_note(&url(u), "", "", &react, false);
        assert_eq!(elsewhere("https://api.example.com/api/v1/users"), None);
        assert_eq!(elsewhere("https://example.com/v18/whatever"), None);
        // docs.rs names the crate in the address, so it counts wherever it is.
        let tokio = lib("tokio", "1.38.2", Ecosystem::Cargo);
        assert_eq!(
            version_note(
                &url("https://docs.rs/tokio/1.38.0/tokio/"),
                "",
                "",
                &tokio,
                false
            )
            .as_deref(),
            Some("matches tokio 1.38")
        );
        // Another crate's version says nothing about this one.
        assert_eq!(
            version_note(
                &url("https://docs.rs/serde/1.0.0/serde/"),
                "",
                "",
                &tokio,
                false
            ),
            None
        );
    }

    #[test]
    fn a_version_next_to_the_name_in_the_heading() {
        let django = lib("django", "4.2.11", Ecosystem::Pypi);
        let page = |title: &str, text: &str| {
            version_note(
                &url("https://example.com/page"),
                title,
                text,
                &django,
                false,
            )
        };
        assert_eq!(
            page("Working with forms | Django 4.2 documentation", "").as_deref(),
            Some("matches django 4.2")
        );
        assert_eq!(
            page("Django 5.1 release notes", "").as_deref(),
            Some("for django 5.1")
        );
        assert_eq!(
            page("Forms", "Django v4.2.3 documentation\n\nForms are...").as_deref(),
            Some("matches django 4.2")
        );
        assert_eq!(
            page("Forms", "Documentation for Django version 3.2").as_deref(),
            Some("for django 3.2")
        );
        // Both named: the page covers the project's version.
        assert_eq!(
            page("Django 5.0 and Django 4.2 compared", "").as_deref(),
            Some("matches django 4.2")
        );
        // A number that does not follow the name, or far down the page, says nothing.
        assert_eq!(page("Top 10 Django tips for 2024", ""), None);
        assert_eq!(
            page("Forms", "Django is a web framework. Python 3.12 is needed."),
            None
        );
        let far = format!("{}Django 1.11 was released long ago.", "word ".repeat(400));
        assert_eq!(page("Forms", &far), None);
        assert_eq!(page("Djangonauts 9.9", ""), None);
        // Names written with ".js", and a bare number right after the name.
        let vue = lib("vue", "3.4.21", Ecosystem::Npm);
        let heading =
            |title: &str| version_note(&url("https://example.com/x"), title, "", &vue, false);
        assert_eq!(
            heading("What is new in Vue.js 3.4").as_deref(),
            Some("matches vue 3.4")
        );
        assert_eq!(
            heading("Migrating from Vue 2").as_deref(),
            Some("for vue 2")
        );
        assert_eq!(
            heading("Vue 3 composition API").as_deref(),
            Some("matches vue 3.4")
        );
        assert_eq!(heading("Vue 2024 conference recap"), None);

        let tokio = lib("tokio", "1.38.2", Ecosystem::Cargo);
        assert_eq!(
            version_note(
                &url("https://docs.rs/tokio/latest/tokio/"),
                "tokio - Rust",
                "tokio 1.44.1\n\nA runtime",
                &tokio,
                true
            )
            .as_deref(),
            Some("for tokio 1.44")
        );
        // The address wins over the heading.
        assert_eq!(
            version_note(
                &url("https://docs.rs/tokio/1.38.0/tokio/"),
                "tokio 0.1 - Rust",
                "",
                &tokio,
                true
            )
            .as_deref(),
            Some("matches tokio 1.38")
        );
    }

    #[test]
    fn no_note_without_a_known_version() {
        let unknown = lib("numpy", "", Ecosystem::Pypi);
        assert_eq!(
            note(
                "https://numpy.org/doc/1.26/reference/",
                "NumPy v1.26 Manual",
                "",
                &unknown
            ),
            None
        );
        let numpy = lib("numpy", "1.26.4", Ecosystem::Pypi);
        assert_eq!(
            note("https://numpy.org/doc/1.26/reference/", "", "", &numpy).as_deref(),
            Some("matches numpy 1.26")
        );
        assert_eq!(
            note(
                "https://numpy.org/doc/2.1/",
                "NumPy v2.1 Manual",
                "",
                &numpy
            )
            .as_deref(),
            Some("for numpy 2.1")
        );
        // A Go module is called by its short name.
        let gin = lib("github.com/gin-gonic/gin", "1.9.1", Ecosystem::Go);
        assert_eq!(
            version_note(
                &url("https://example.com/x"),
                "Gin v1.9 middleware",
                "",
                &gin,
                false
            )
            .as_deref(),
            Some("matches gin 1.9")
        );
    }
}
