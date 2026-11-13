//! `reverse.lock.json`: the pinned rea, Node.js, Kuna, Ghidra and Java downloads
//! (item5-spec.md R1, R6, R15). Compiled into the app, like `llama.lock.json` and
//! `browser.lock.json`, so the app knows every URL, size and SHA-256 without asking the
//! network. `scripts/pin_reverse.py` writes it; `--check` and the tests below keep it honest.

use std::sync::OnceLock;

use runtime::PackFile;
use serde::{Deserialize, Serialize};

use crate::{Component, Platform};

/// The lock file as compiled into this build.
const EMBEDDED: &str = include_str!("../../../reverse.lock.json");

/// Schema version this build reads.
pub const SCHEMA: u32 = 2;

/// rea's npm registry.
pub const NPM_HOSTS: &[&str] = &["registry.npmjs.org"];
/// Node.js's download site.
pub const NODE_HOSTS: &[&str] = &["nodejs.org"];
/// GitHub release assets (Kuna, Ghidra, Temurin) and the hosts they redirect to.
pub const GITHUB_HOSTS: &[&str] = &[
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

/// The whole lock file. Unknown fields are ignored, so the file may grow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReverseLock {
    /// Schema version of the file ([`SCHEMA`]).
    pub schema: u32,
    /// `REA_MCP_TOOL_FAMILIES` (item5-spec.md section 5). They trim rea, they do not
    /// protect: the app's allow-list does (R10).
    pub families: String,
    /// `REA_GHIDRA_DISABLED_ANALYZERS` per platform key (R20).
    #[serde(default)]
    pub disabled_ghidra_analyzers: std::collections::BTreeMap<String, String>,
    /// rea, Node.js, Kuna, Ghidra and Java.
    pub parts: Vec<Part>,
}

/// One pinned part: a program and its folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Part {
    /// `rea`, `node`, `kuna`, `ghidra` or `java`.
    pub name: String,
    /// The Settings row it belongs to.
    pub component: Component,
    /// Pinned version.
    pub version: String,
    /// An installed copy below this is never started (R15 a).
    pub minimum_version: String,
    /// Why the minimum was raised, when it was.
    #[serde(default)]
    pub minimum_reason: Option<String>,
    /// Folder under the reverse root (`rea-6.9.0`).
    pub dir: String,
    /// Licence named before the download.
    pub license_name: String,
    /// Where the licence text lives.
    pub license_url: String,
    /// Hosts the download may come from (one of the compiled-in sets).
    pub hosts: Vec<String>,
    /// Vendor name (Java only).
    #[serde(default)]
    pub vendor: Option<String>,
    /// rea's entry script, relative to its folder (rea only).
    #[serde(default)]
    pub entry: Option<String>,
    /// rea versions the app carries a reviewed allow-list fixture for (rea only, R15 b).
    #[serde(default)]
    pub accepted_rea_versions: Vec<AcceptedRea>,
    /// One build per platform where the part is offered.
    pub builds: Vec<Build>,
}

/// A rea version the app may run, with its catalog fixture.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptedRea {
    /// rea version.
    pub version: String,
    /// File under `crates/reverse/tests/fixtures/`.
    pub schema_fixture: String,
}

/// What is kept when an archive is unpacked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Keep {
    /// The string `"all"`: every entry.
    All(KeepAll),
    /// Exactly these relative paths (after the strip prefix).
    Files(Vec<String>),
}

/// The literal `"all"` of [`Keep::All`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeepAll {
    /// Keep everything.
    All,
}

impl Keep {
    /// Whether every entry is kept.
    pub fn is_all(&self) -> bool {
        matches!(self, Keep::All(_))
    }
    /// Whether the relative path `rel` is kept.
    pub fn keeps(&self, rel: &str) -> bool {
        match self {
            Keep::All(_) => true,
            Keep::Files(files) => files.iter().any(|f| f == rel),
        }
    }
}

/// A file the app or rea runs or reads, checked after unpacking and on every status.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Program {
    /// Relative path inside the part's folder.
    pub path: String,
    /// It must be executable on macOS and Linux.
    #[serde(default)]
    pub exec: bool,
    /// Size in the archive, in bytes.
    pub size: u64,
}

/// One part's archive for one platform.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Build {
    /// Platform key (`windows-x86_64`).
    pub platform: String,
    /// The publisher's own asset name.
    pub upstream_asset: String,
    /// The archive, pinned by size and SHA-256.
    pub file: PackFile,
    /// Leading folder removed from every entry (`package/`).
    pub strip_prefix: String,
    /// What is kept.
    pub keep: Keep,
    /// Files checked after unpacking (the first is the one run, where there is one).
    pub programs: Vec<Program>,
    /// Longest kept relative path, in characters, after the prefix.
    pub max_path_chars: usize,
    /// Entries in the archive.
    pub entries: usize,
    /// Bytes kept on disk.
    pub unpacked_bytes: u64,
    /// `JAVA_HOME` relative to the folder (Java only; empty for the folder itself).
    #[serde(default)]
    pub java_home: Option<String>,
    /// Publisher checks that passed at pin time.
    pub verified_by: Vec<String>,
}

/// The publisher checks a part's build must have recorded at pin time for a release build of
/// the app to offer it (item5-spec.md WP9a): rea by npm's integrity, provenance and
/// signatures, Node.js and Temurin by their publishers' signatures, Ghidra by the SHA-256
/// digest GitHub publishes. Kuna's releases carry no signature, so its first download is all
/// there is (any recorded check). `None` for a part this build does not know.
/// `scripts/pin_reverse.py --check --release` applies the same rule.
pub fn release_checks(part: &str) -> Option<&'static [&'static str]> {
    Some(match part {
        "rea" => &["npm-integrity", "npm-provenance:", "npm-audit-signatures"],
        "node" => &["gpg:nodejs-release-keys"],
        "kuna" => &[],
        "ghidra" => &["github-digest"],
        "java" => &["gpg:adoptium", "adoptium-api-checksum"],
        _ => return None,
    })
}

/// Whether a pinned file's name is one plain file name and its SHA-256 64 hex digits, so its
/// resumable download (`.downloads/<name>.<16 hex>.part`) stays inside `.downloads/`.
pub fn safe_pinned_file(f: &PackFile) -> bool {
    crate::archive::safe_relative(&f.name).is_some_and(|parts| parts.len() == 1)
        && f.sha256.len() == 64
        && f.sha256.bytes().all(|b| b.is_ascii_hexdigit())
}

impl ReverseLock {
    /// Parses a lock file.
    pub fn parse(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// The part with this name.
    pub fn part(&self, name: &str) -> Option<&Part> {
        self.parts.iter().find(|p| p.name == name)
    }

    /// The parts of a Settings row, in install order (rea before Node; Ghidra before Java).
    pub fn parts_of(&self, component: Component) -> impl Iterator<Item = &Part> {
        self.parts.iter().filter(move |p| p.component == component)
    }

    /// The pinned rea version.
    pub fn rea_version(&self) -> Option<&str> {
        self.part("rea").map(|p| p.version.as_str())
    }

    /// `REA_GHIDRA_DISABLED_ANALYZERS` for this platform.
    pub fn disabled_analyzers(&self, platform: Platform) -> Option<&str> {
        self.disabled_ghidra_analyzers
            .get(platform.key())
            .map(String::as_str)
    }

    /// Every build a release build of the app would not offer, because a publisher check
    /// it needs ([`release_checks`]) was not recorded at pin time ("ghidra windows-x86_64").
    /// Empty when the lock is ready for a release.
    pub fn release_blockers(&self) -> Vec<String> {
        self.parts
            .iter()
            .flat_map(|p| {
                p.builds
                    .iter()
                    .filter(|b| !b.release_ready(&p.name))
                    .map(move |b| format!("{} {}", p.name, b.platform))
            })
            .collect()
    }
}

impl Build {
    /// Whether the check `kind` was recorded (`npm-provenance:` matches any commit).
    pub fn verified(&self, kind: &str) -> bool {
        self.verified_by.iter().any(|v| {
            if kind.ends_with(':') {
                v.starts_with(kind) && v.len() > kind.len()
            } else {
                v == kind
            }
        })
    }

    /// Whether a release build of the app offers this build of `part` ([`release_checks`]).
    pub fn release_ready(&self, part: &str) -> bool {
        !self.verified_by.is_empty()
            && release_checks(part).is_some_and(|need| need.iter().all(|k| self.verified(k)))
    }
}

impl Part {
    /// This part's build for `platform`, if it is offered there.
    pub fn build_for(&self, platform: Platform) -> Option<&Build> {
        self.builds.iter().find(|b| b.platform == platform.key())
    }

    /// The compiled-in host set that matches this part's `hosts`, or `None` when the lock
    /// names any other host (such a part is never downloaded).
    pub fn host_set(&self) -> Option<&'static [&'static str]> {
        [NPM_HOSTS, NODE_HOSTS, GITHUB_HOSTS]
            .into_iter()
            .find(|set| {
                set.len() == self.hosts.len()
                    && set.iter().all(|h| self.hosts.iter().any(|x| x == h))
            })
    }

    /// Whether `version` may still run: at or above the minimum and, for rea, one of the
    /// versions with a reviewed fixture.
    pub fn accepts(&self, version: &str) -> Acceptance {
        if !version_at_least(version, &self.minimum_version) {
            return Acceptance::BelowMinimum;
        }
        if self.name == "rea"
            && !self
                .accepted_rea_versions
                .iter()
                .any(|a| a.version == version)
        {
            return Acceptance::NotAccepted;
        }
        Acceptance::Yes
    }
}

/// Whether an installed version may run (R15).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Acceptance {
    /// It may.
    Yes,
    /// Below the lock's `minimum_version` (a known security problem).
    BelowMinimum,
    /// A rea version without a reviewed allow-list fixture in this app.
    NotAccepted,
}

/// A version such as `21.0.13+9` or `1.990` as numbers; `None` when it is not one.
pub fn version_key(version: &str) -> Option<Vec<u64>> {
    let parts: Option<Vec<u64>> = version
        .split(['.', '+', '_', '-'])
        .map(|p| p.parse().ok())
        .collect();
    parts.filter(|p| !p.is_empty() && p.len() <= 8)
}

/// Whether `version >= minimum`, comparing numbers (missing trailing numbers count as 0).
/// A version that does not parse is never enough.
pub fn version_at_least(version: &str, minimum: &str) -> bool {
    let (Some(mut v), Some(mut m)) = (version_key(version), version_key(minimum)) else {
        return false;
    };
    let n = v.len().max(m.len());
    v.resize(n, 0);
    m.resize(n, 0);
    v >= m
}

/// The lock file compiled into this build, parsed once.
///
/// # Panics
///
/// Panics if the embedded file does not parse. The file is part of the source tree and a
/// unit test parses it, so a broken lock file cannot pass CI.
pub fn embedded() -> &'static ReverseLock {
    static LOCK: OnceLock<ReverseLock> = OnceLock::new();
    LOCK.get_or_init(|| match ReverseLock::parse(EMBEDDED) {
        Ok(lock) => lock,
        // The input is a compile-time constant checked by `embedded_lock_parses`.
        Err(error) => panic!("embedded reverse.lock.json is invalid: {error}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::safe_relative;

    fn is_hex64(s: &str) -> bool {
        s.len() == 64
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }

    #[test]
    fn embedded_lock_parses() {
        let lock = ReverseLock::parse(EMBEDDED).expect("embedded lock parses");
        assert_eq!(&lock, embedded());
        assert_eq!(lock.schema, SCHEMA);
        assert_eq!(
            lock.families,
            "direct,enhanced,native,artifact,managed,session,electron,application"
        );
        let names: Vec<&str> = lock.parts.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["rea", "node", "kuna", "ghidra", "java"]);
    }

    #[test]
    fn every_url_is_https_on_its_parts_hosts() {
        let mut count = 0;
        for part in &embedded().parts {
            let hosts = part
                .host_set()
                .unwrap_or_else(|| panic!("{}: hosts {:?}", part.name, part.hosts));
            for b in &part.builds {
                let url = reqwest::Url::parse(&b.file.url).expect("url parses");
                assert_eq!(url.scheme(), "https", "{}", b.file.url);
                assert!(
                    url.host_str().is_some_and(|h| hosts.contains(&h)),
                    "{}: {} is not on {hosts:?}",
                    part.name,
                    b.file.url
                );
                assert!(url.username().is_empty() && url.password().is_none());
                assert!(b.file.url.ends_with(&b.file.name), "{}", b.file.url);
                assert_eq!(b.file.name, b.upstream_asset);
                assert!(safe_pinned_file(&b.file), "{}", b.file.name);
                assert!(is_hex64(&b.file.sha256), "{}", b.file.name);
                assert!(b.file.size > 0);
                count += 1;
            }
        }
        assert!(count >= 13, "{count} builds");
    }

    #[test]
    fn hosts_are_one_of_the_compiled_in_sets() {
        let lock = embedded();
        assert_eq!(lock.part("rea").unwrap().host_set(), Some(NPM_HOSTS));
        assert_eq!(lock.part("node").unwrap().host_set(), Some(NODE_HOSTS));
        for name in ["kuna", "ghidra", "java"] {
            assert_eq!(lock.part(name).unwrap().host_set(), Some(GITHUB_HOSTS));
        }
        let mut odd = lock.part("rea").unwrap().clone();
        odd.hosts.push("evil.example".into());
        assert_eq!(odd.host_set(), None);
        odd.hosts = vec!["registry.npmjs.org.evil.example".into()];
        assert_eq!(odd.host_set(), None);
    }

    #[test]
    fn every_shipped_platform_has_rea_and_node() {
        let lock = embedded();
        for platform in Platform::SHIPPED {
            for name in ["rea", "node"] {
                assert!(
                    lock.part(name).unwrap().build_for(platform).is_some(),
                    "{name} on {}",
                    platform.key()
                );
            }
        }
    }

    /// The owner's answers: Kuna is not offered on Windows (Q1 = A); on a Mac only Kuna,
    /// no Ghidra or Java (Q2 = B); Linux keeps both.
    #[test]
    fn decompilers_are_offered_where_the_owner_chose() {
        let lock = embedded();
        let on = |name: &str, p: Platform| lock.part(name).unwrap().build_for(p).is_some();
        assert!(!on("kuna", Platform::WindowsX64));
        assert!(on("ghidra", Platform::WindowsX64) && on("java", Platform::WindowsX64));
        for mac in [Platform::MacosArm64, Platform::MacosX64] {
            assert!(on("kuna", mac));
            assert!(!on("ghidra", mac) && !on("java", mac));
        }
        assert!(on("kuna", Platform::LinuxX64) && on("ghidra", Platform::LinuxX64));
    }

    #[test]
    fn every_part_has_its_checks_and_a_sane_minimum() {
        for part in &embedded().parts {
            assert!(version_key(&part.version).is_some(), "{}", part.name);
            assert!(
                version_at_least(&part.version, &part.minimum_version),
                "{}: minimum {} above {}",
                part.name,
                part.minimum_version,
                part.version
            );
            assert_eq!(part.dir, format!("{}-{}", part.name, part.version));
            assert!(safe_relative(&part.dir).is_some());
            assert!(part.license_url.starts_with("https://"));
            for b in &part.builds {
                assert!(!b.verified_by.is_empty(), "{} {}", part.name, b.platform);
                assert!(b.strip_prefix.ends_with('/'));
                assert!(!b.programs.is_empty());
                for p in &b.programs {
                    assert!(safe_relative(&p.path).is_some(), "{}", p.path);
                    assert!(b.keep.keeps(&p.path), "{} is not kept", p.path);
                }
                assert!(b.entries > 0 && b.unpacked_bytes > 0 && b.max_path_chars > 0);
            }
        }
    }

    #[test]
    fn pinned_file_names_stay_in_the_downloads_folder() {
        let ok = PackFile {
            name: "rea-agents-6.9.0.tgz".into(),
            url: String::new(),
            size: 1,
            sha256: "a".repeat(64),
        };
        assert!(safe_pinned_file(&ok));
        for name in ["../x.zip", "a/b.zip", "", "..", "con.zip", "x:y", "a\\b"] {
            let f = PackFile {
                name: name.into(),
                ..ok.clone()
            };
            assert!(!safe_pinned_file(&f), "{name:?}");
        }
        for sha in [
            "../../../../../../../../../../../../../../../../../../../../../../../.",
            "abc",
        ] {
            let f = PackFile {
                sha256: sha.into(),
                ..ok.clone()
            };
            assert!(!safe_pinned_file(&f), "{sha:?}");
        }
    }

    /// Only Ghidra may lack its publisher check in the committed lock, and only until the lead
    /// pins it where GitHub's API answers (the WP9a report); every other part already has
    /// what a release build needs.
    #[test]
    fn only_ghidra_may_wait_for_its_publisher_check() {
        for b in embedded().release_blockers() {
            assert!(b.starts_with("ghidra "), "{b} is not ready for a release");
        }
        let ghidra = embedded().part("ghidra").unwrap();
        for b in &ghidra.builds {
            assert!(
                b.verified("github-digest") || b.verified_by == ["first-download-only"],
                "{:?}",
                b.verified_by
            );
        }
    }

    #[test]
    fn release_checks_follow_each_publisher() {
        let lock = embedded();
        let mut b = lock.part("java").unwrap().builds[0].clone();
        assert!(b.release_ready("java"));
        b.verified_by = vec!["first-download-only".into()];
        assert!(!b.release_ready("java"), "Temurin needs its signature");
        assert!(b.release_ready("kuna"), "Kuna has nothing more to give");
        assert!(!b.release_ready("ghidra"), "Ghidra needs GitHub's digest");
        assert!(
            !b.release_ready("someday"),
            "an unknown part is never offered"
        );
        b.verified_by = vec!["github-digest".into()];
        assert!(b.release_ready("ghidra"));
        b.verified_by = vec!["npm-integrity".into(), "npm-audit-signatures".into()];
        assert!(!b.release_ready("rea"), "rea needs its provenance");
        b.verified_by.push("npm-provenance:".into());
        assert!(!b.release_ready("rea"), "a provenance names its commit");
        b.verified_by
            .push("npm-provenance:morluto/rea@d74549e".into());
        assert!(b.release_ready("rea"));
        b.verified_by.clear();
        assert!(!b.release_ready("kuna"));
        // A lock with a Ghidra build pinned on first download names it.
        let mut lock = lock.clone();
        for p in &mut lock.parts {
            for b in &mut p.builds {
                b.verified_by = release_checks(&p.name)
                    .unwrap()
                    .iter()
                    .map(|k| format!("{k}{}", if k.ends_with(':') { "x@y" } else { "" }))
                    .collect();
                if b.verified_by.is_empty() {
                    b.verified_by.push("first-download-only".into());
                }
            }
        }
        assert!(lock.release_blockers().is_empty());
        let g = lock.parts.iter_mut().find(|p| p.name == "ghidra").unwrap();
        g.builds[0].verified_by = vec!["first-download-only".into()];
        let platform = g.builds[0].platform.clone();
        assert_eq!(lock.release_blockers(), [format!("ghidra {platform}")]);
    }

    /// The previous rea stays usable after a bump: no accepted version is below rea's minimum
    /// unless the minimum was raised for a recorded reason (R15).
    #[test]
    fn accepted_rea_versions_are_not_below_the_minimum() {
        let rea = embedded().part("rea").unwrap();
        for a in &rea.accepted_rea_versions {
            assert!(
                version_at_least(&a.version, &rea.minimum_version) || rea.minimum_reason.is_some(),
                "accepted {} is below minimum {} with no reason",
                a.version,
                rea.minimum_version
            );
        }
    }

    #[test]
    fn each_accepted_rea_version_has_its_fixture() {
        let rea = embedded().part("rea").unwrap();
        assert_eq!(rea.entry.as_deref(), Some("scripts/rea.mjs"));
        assert!(!rea.accepted_rea_versions.is_empty() && rea.accepted_rea_versions.len() <= 2);
        assert_eq!(rea.accepted_rea_versions[0].version, rea.version);
        for a in &rea.accepted_rea_versions {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(&a.schema_fixture);
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let fixture: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(fixture["rea_version"], a.version.as_str());
            assert_eq!(fixture["tool_names"].as_array().unwrap().len(), 255);
            assert_eq!(fixture["allow_listed"].as_object().unwrap().len(), 37);
        }
    }

    #[test]
    fn ghidra_analyzers_are_named_for_every_ghidra_platform() {
        let lock = embedded();
        for b in &lock.part("ghidra").unwrap().builds {
            let p = Platform::from_key(&b.platform).unwrap();
            assert_eq!(lock.disabled_analyzers(p), Some("PDB Universal"));
        }
    }

    #[test]
    fn versions_compare_by_number() {
        assert!(version_at_least("21.0.13+9", "21"));
        assert!(version_at_least("12.1.4", "12.1"));
        assert!(!version_at_least("12.0.9", "12.1"));
        assert!(version_at_least("1.990", "1.990"));
        assert!(!version_at_least("1.99", "1.990"));
        assert!(version_at_least("24.22.3", "24.22.1"));
        assert!(!version_at_least("abc", "1"));
        assert!(!version_at_least("1", "x"));
    }

    #[test]
    fn acceptance_follows_minimum_and_fixtures() {
        let lock = embedded();
        let rea = lock.part("rea").unwrap();
        assert_eq!(rea.accepts(&rea.version), Acceptance::Yes);
        assert_eq!(rea.accepts("6.8.0"), Acceptance::BelowMinimum);
        let mut newer = rea.clone();
        newer.minimum_version = "6.0.0".into();
        assert_eq!(newer.accepts("6.8.0"), Acceptance::NotAccepted);
        let ghidra = lock.part("ghidra").unwrap();
        assert_eq!(ghidra.accepts("12.1.0"), Acceptance::Yes);
        assert_eq!(ghidra.accepts("11.4.2"), Acceptance::BelowMinimum);
    }
}
