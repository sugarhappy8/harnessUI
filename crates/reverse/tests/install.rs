//! Installing the reverse rows from local archives through `Fetch` (item5-spec.md WP9a):
//! success, checksum and size mismatches, resume, cancel, discarding a paused download, a
//! decompiler download that brings rea, the Windows path limit and file system, a full
//! disk, the marker written last, broken and blocked installs, versions that need an
//! update, removal, and an update that keeps the old version until the new one is
//! complete. The last test installs the real pinned archives when `REVERSE_PACK_CACHE`
//! names a cache filled by `scripts/pin_reverse.py --download-to`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use reverse::install::{Fetch, Installer, part_file_name};
use reverse::lock::{AcceptedRea, Build, Keep, KeepAll, Part, Program, ReverseLock};
use reverse::{
    BrokenWhy, Component, ComponentState, DirFetch, DownloadStage, Platform, Progress,
    ReverseError, ReverseLayout, UpdateWhy,
};
use runtime::PackFile;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

const ALL: Keep = Keep::All(KeepAll::All);

/// One file of a fake archive: path inside, content, executable.
type Entry<'a> = (&'a str, &'a [u8], bool);

fn sha256(path: &Path) -> String {
    let data = std::fs::read(path).unwrap();
    Sha256::digest(&data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn write_tar_gz(path: &Path, entries: &[Entry]) {
    let f = std::fs::File::create(path).unwrap();
    let gz = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
    let mut b = tar::Builder::new(gz);
    for (name, data, exec) in entries {
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(if *exec { 0o755 } else { 0o644 });
        h.set_entry_type(tar::EntryType::Regular);
        b.append_data(&mut h, name, *data).unwrap();
    }
    b.into_inner().unwrap().finish().unwrap();
}

fn write_zip(path: &Path, entries: &[Entry]) {
    let f = std::fs::File::create(path).unwrap();
    let mut w = zip::ZipWriter::new(f);
    for (name, data, exec) in entries {
        let o = zip::write::SimpleFileOptions::default().unix_permissions(if *exec {
            0o755
        } else {
            0o644
        });
        w.start_file(*name, o).unwrap();
        w.write_all(data).unwrap();
    }
    w.finish().unwrap();
}

/// A fake pinned part built from a local archive.
struct FakePart {
    name: &'static str,
    component: Component,
    version: &'static str,
    minimum: &'static str,
    asset: String,
    strip: &'static str,
    keep: Keep,
    entries: Vec<(String, Vec<u8>, bool)>,
    programs: Vec<(&'static str, bool)>,
    max_path_chars: usize,
}

fn fake_part(archives: &Path, p: &FakePart, platforms: &[Platform], hosts: &[&str]) -> Part {
    let path = archives.join(&p.asset);
    let entries: Vec<Entry> = p
        .entries
        .iter()
        .map(|(n, d, x)| (n.as_str(), d.as_slice(), *x))
        .collect();
    if p.asset.ends_with(".zip") {
        write_zip(&path, &entries);
    } else {
        write_tar_gz(&path, &entries);
    }
    let size = std::fs::metadata(&path).unwrap().len();
    let kept: Vec<&(String, Vec<u8>, bool)> = p
        .entries
        .iter()
        .filter(|(n, _, _)| p.keep.keeps(n.strip_prefix(p.strip).unwrap()))
        .collect();
    let programs = p
        .programs
        .iter()
        .map(|(rel, exec)| {
            let data = &p
                .entries
                .iter()
                .find(|(n, _, _)| n.strip_prefix(p.strip) == Some(*rel))
                .unwrap()
                .1;
            Program {
                path: rel.to_string(),
                exec: *exec,
                size: data.len() as u64,
            }
        })
        .collect::<Vec<_>>();
    let file = PackFile {
        name: p.asset.clone(),
        url: format!("https://{}/files/{}", hosts[0], p.asset),
        size,
        sha256: sha256(&path),
    };
    Part {
        name: p.name.into(),
        component: p.component,
        version: p.version.into(),
        minimum_version: p.minimum.into(),
        minimum_reason: None,
        dir: format!("{}-{}", p.name, p.version),
        license_name: "MIT".into(),
        license_url: "https://example.org/LICENSE".into(),
        hosts: hosts.iter().map(|h| h.to_string()).collect(),
        vendor: None,
        entry: (p.name == "rea").then(|| "scripts/rea.mjs".into()),
        accepted_rea_versions: if p.name == "rea" {
            vec![AcceptedRea {
                version: p.version.into(),
                schema_fixture: format!("rea-{}-catalog.json", p.version),
            }]
        } else {
            Vec::new()
        },
        builds: platforms
            .iter()
            .map(|pl| Build {
                platform: pl.key().into(),
                upstream_asset: p.asset.clone(),
                file: file.clone(),
                strip_prefix: p.strip.into(),
                keep: p.keep.clone(),
                programs: programs.clone(),
                max_path_chars: p.max_path_chars,
                entries: p.entries.len(),
                unpacked_bytes: kept.iter().map(|(_, d, _)| d.len() as u64).sum(),
                java_home: None,
                verified_by: vec!["first-download-only".into()],
            })
            .collect(),
    }
}

fn rea_part(version: &'static str, body: &[u8]) -> FakePart {
    FakePart {
        name: "rea",
        component: Component::Rea,
        version,
        minimum: version,
        asset: format!("rea-agents-{version}.tgz"),
        strip: "package/",
        keep: ALL,
        entries: vec![
            ("package/scripts/rea.mjs".into(), body.to_vec(), true),
            ("package/package.json".into(), b"{}".to_vec(), false),
            ("package/dist/a.js".into(), b"export {}".to_vec(), false),
        ],
        programs: vec![("scripts/rea.mjs", false), ("package.json", false)],
        max_path_chars: 15,
    }
}

const PLATFORMS: [Platform; 2] = [Platform::LinuxX64, Platform::WindowsX64];
const NPM: &[&str] = &["registry.npmjs.org"];
const NODE: &[&str] = &["nodejs.org"];
const GH: &[&str] = &[
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

struct Fixture {
    _dir: tempfile::TempDir,
    archives: PathBuf,
    root: PathBuf,
    lock: ReverseLock,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let archives = dir.path().join("archives");
    std::fs::create_dir_all(&archives).unwrap();
    let root = dir.path().join("reverse");
    let node = FakePart {
        name: "node",
        component: Component::Rea,
        version: "24.22.3",
        minimum: "24.22.1",
        asset: "node-v24.22.3-linux-x64.tar.gz".into(),
        strip: "node-v24.22.3-linux-x64/",
        keep: Keep::Files(vec!["bin/node".into(), "LICENSE".into()]),
        entries: vec![
            (
                "node-v24.22.3-linux-x64/bin/node".into(),
                b"ELF node".to_vec(),
                true,
            ),
            (
                "node-v24.22.3-linux-x64/bin/npm".into(),
                b"npm".to_vec(),
                true,
            ),
            (
                "node-v24.22.3-linux-x64/LICENSE".into(),
                b"MIT".to_vec(),
                false,
            ),
        ],
        programs: vec![("bin/node", true)],
        max_path_chars: 8,
    };
    let kuna = FakePart {
        name: "kuna",
        component: Component::Kuna,
        version: "1.990",
        minimum: "1.990",
        asset: "kuna-v1.990-linux-x86_64.tar.gz".into(),
        strip: "kuna-v1.990-linux-x86_64/",
        keep: ALL,
        entries: vec![
            (
                "kuna-v1.990-linux-x86_64/kuna".into(),
                b"ELF kuna".to_vec(),
                true,
            ),
            (
                "kuna-v1.990-linux-x86_64/LICENSE".into(),
                b"Apache".to_vec(),
                false,
            ),
        ],
        programs: vec![("kuna", true)],
        max_path_chars: 7,
    };
    let ghidra = FakePart {
        name: "ghidra",
        component: Component::Ghidra,
        version: "12.1.4",
        minimum: "12.1",
        asset: "ghidra_12.1.4_PUBLIC_20260921.zip".into(),
        strip: "ghidra_12.1.4_PUBLIC/",
        keep: ALL,
        entries: vec![
            (
                "ghidra_12.1.4_PUBLIC/support/analyzeHeadless".into(),
                b"#!/bin/sh".to_vec(),
                true,
            ),
            (
                "ghidra_12.1.4_PUBLIC/Ghidra/application.properties".into(),
                b"application.version=12.1.4".to_vec(),
                false,
            ),
        ],
        programs: vec![("support/analyzeHeadless", true)],
        max_path_chars: 100,
    };
    let java = FakePart {
        name: "java",
        component: Component::Ghidra,
        version: "21.0.13+9",
        minimum: "21",
        asset: "OpenJDK21U-jdk_x64_linux_hotspot_21.0.13_9.tar.gz".into(),
        strip: "jdk-21.0.13+9/",
        keep: ALL,
        entries: vec![
            ("jdk-21.0.13+9/bin/java".into(), b"ELF java".to_vec(), true),
            (
                "jdk-21.0.13+9/bin/javac".into(),
                b"ELF javac".to_vec(),
                true,
            ),
        ],
        programs: vec![("bin/java", true), ("bin/javac", true)],
        max_path_chars: 9,
    };
    let lock = ReverseLock {
        schema: 2,
        families: "direct".into(),
        disabled_ghidra_analyzers: Default::default(),
        parts: vec![
            fake_part(&archives, &rea_part("6.9.0", b"rea 6.9.0"), &PLATFORMS, NPM),
            fake_part(&archives, &node, &PLATFORMS, NODE),
            fake_part(&archives, &kuna, &[Platform::LinuxX64], GH),
            fake_part(&archives, &ghidra, &PLATFORMS, GH),
            fake_part(&archives, &java, &PLATFORMS, GH),
        ],
    };
    Fixture {
        _dir: dir,
        archives,
        root,
        lock,
    }
}

impl Fixture {
    fn installer(&self) -> Installer {
        self.installer_with(self.lock.clone(), Platform::LinuxX64)
    }
    fn installer_with(&self, lock: ReverseLock, platform: Platform) -> Installer {
        Installer::new(ReverseLayout::new(&self.root), platform)
            .with_lock(Arc::new(lock))
            .with_fetch(Arc::new(DirFetch::new(&self.archives)))
            .with_free_space(Arc::new(|_: &Path| Some(u64::MAX)))
            .with_fs_name(Arc::new(|_: &Path| Some("NTFS".to_string())))
    }
    fn part(&self, name: &str) -> &Part {
        self.lock.part(name).unwrap()
    }
    fn downloads(&self) -> PathBuf {
        self.root.join(".downloads")
    }
    fn part_file(&self, name: &str) -> PathBuf {
        self.downloads()
            .join(part_file_name(&self.part(name).builds[0].file))
    }
}

/// Records every progress report.
#[derive(Default)]
struct Seen(Mutex<Vec<Progress>>);

impl Seen {
    fn stages(&self) -> Vec<(String, DownloadStage)> {
        let mut out: Vec<(String, DownloadStage)> = Vec::new();
        for p in self.0.lock().unwrap().iter() {
            let item = (p.part.clone(), p.stage);
            if out.last() != Some(&item) {
                out.push(item);
            }
        }
        out
    }
    fn parts(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for p in self.0.lock().unwrap().iter() {
            if !out.contains(&p.part) {
                out.push(p.part.clone());
            }
        }
        out
    }
}

async fn download(
    i: &Installer,
    c: Component,
    seen: &Seen,
) -> Result<reverse::Installed, ReverseError> {
    i.download(c, &CancellationToken::new(), &|p| {
        seen.0.lock().unwrap().push(p)
    })
    .await
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.permissions().mode() & 0o100 != 0)
}

#[tokio::test]
async fn rea_installs_from_local_archives() {
    let f = fixture();
    let i = f.installer();
    assert_eq!(i.state(Component::Rea), ComponentState::NotInstalled);
    let seen = Seen::default();
    let installed = download(&i, Component::Rea, &seen).await.unwrap();
    assert_eq!(installed.version, "6.9.0");
    assert_eq!(
        installed
            .parts
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["rea", "node"]
    );
    assert_eq!(i.state(Component::Rea), ComponentState::Installed);
    let rea_dir = f.root.join("rea-6.9.0");
    assert_eq!(
        std::fs::read(rea_dir.join("scripts/rea.mjs")).unwrap(),
        b"rea 6.9.0"
    );
    assert!(rea_dir.join("dist/a.js").is_file());
    assert!(rea_dir.join(".complete").is_file());
    let node = f.root.join("node-24.22.3");
    assert!(node.join("bin/node").is_file() && node.join("LICENSE").is_file());
    assert!(!node.join("bin/npm").exists(), "only the kept files");
    #[cfg(unix)]
    assert!(is_executable(&node.join("bin/node")));
    // The parts are gone once installed, and no temporary folder is left.
    assert!(!f.part_file("rea").exists() && !f.part_file("node").exists());
    assert!(!f.root.join(".tmp-rea-6.9.0").exists());
    // Stages in order: every download, then every check, then every unpack.
    assert_eq!(
        seen.stages(),
        [
            ("rea".to_string(), DownloadStage::Downloading),
            ("node".into(), DownloadStage::Downloading),
            ("rea".into(), DownloadStage::Checking),
            ("node".into(), DownloadStage::Checking),
            ("rea".into(), DownloadStage::Unpacking),
            ("node".into(), DownloadStage::Unpacking),
        ]
    );
    let status = i.status(Component::Rea);
    assert_eq!(status.installed_version.as_deref(), Some("6.9.0"));
    assert_eq!(status.pinned_version, "6.9.0");
    assert_eq!(status.update, None);
    let unpacked: u64 = ["rea", "node"]
        .iter()
        .map(|n| f.part(n).builds[0].unpacked_bytes)
        .sum();
    assert_eq!(status.disk_bytes, unpacked);
    assert_eq!(status.licenses.len(), 2);
    // The installation hands WP9b the paths it needs.
    let inst = i.installation();
    assert_eq!(inst.node(), Some(node.join("bin/node")));
    assert_eq!(inst.rea_entry, Some(rea_dir.join("scripts/rea.mjs")));
    assert!(inst.decompilers().is_empty());
    // A second download is a no-op.
    let seen2 = Seen::default();
    download(&i, Component::Rea, &seen2).await.unwrap();
    assert!(seen2.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_decompiler_download_brings_rea() {
    let f = fixture();
    let i = f.installer();
    let seen = Seen::default();
    let kuna = download(&i, Component::Kuna, &seen).await.unwrap();
    assert_eq!(kuna.version, "1.990");
    assert_eq!(seen.parts(), ["rea", "node", "kuna"]);
    assert!(
        seen.0
            .lock()
            .unwrap()
            .iter()
            .all(|p| p.component == Component::Kuna)
    );
    assert_eq!(i.state(Component::Rea), ComponentState::Installed);
    assert_eq!(i.state(Component::Kuna), ComponentState::Installed);
    let inst = i.installation();
    assert_eq!(inst.decompilers(), [reverse::Decompiler::Kuna]);
    assert_eq!(inst.kuna_program(), Some(f.root.join("kuna-1.990/kuna")));
    // With rea installed, Ghidra's download fetches only Ghidra and Java.
    let seen = Seen::default();
    download(&i, Component::Ghidra, &seen).await.unwrap();
    assert_eq!(seen.parts(), ["ghidra", "java"]);
    let inst = i.installation();
    assert_eq!(inst.ghidra_dir(), Some(f.root.join("ghidra-12.1.4")));
    assert_eq!(inst.java_home, Some(f.root.join("java-21.0.13+9")));
}

#[tokio::test]
async fn a_checksum_mismatch_installs_nothing_and_deletes_the_part() {
    let f = fixture();
    let mut lock = f.lock.clone();
    lock.parts
        .iter_mut()
        .find(|p| p.name == "kuna")
        .unwrap()
        .builds[0]
        .file
        .sha256 = "0".repeat(64);
    let i = f.installer_with(lock.clone(), Platform::LinuxX64);
    let e = download(&i, Component::Kuna, &Seen::default())
        .await
        .unwrap_err();
    assert_eq!(
        e,
        ReverseError::Mismatch {
            part: "kuna".into()
        }
    );
    // Checked before anything was unpacked: neither rea nor Kuna is installed.
    for dir in ["rea-6.9.0", "node-24.22.3", "kuna-1.990", ".tmp-kuna-1.990"] {
        assert!(!f.root.join(dir).exists(), "{dir}");
    }
    let kuna_part = f
        .downloads()
        .join(part_file_name(&lock.part("kuna").unwrap().builds[0].file));
    assert!(!kuna_part.exists(), "the bad part is deleted");
    assert_eq!(i.state(Component::Kuna), ComponentState::NotInstalled);
    // rea's verified parts stay for the next attempt.
    assert!(matches!(
        i.state(Component::Rea),
        ComponentState::Partial { .. }
    ));
}

#[tokio::test]
async fn a_size_mismatch_is_refused() {
    for delta in [-1i64, 1] {
        let f = fixture();
        let mut lock = f.lock.clone();
        let b = &mut lock
            .parts
            .iter_mut()
            .find(|p| p.name == "rea")
            .unwrap()
            .builds[0];
        b.file.size = (b.file.size as i64 + delta) as u64;
        let i = f.installer_with(lock, Platform::LinuxX64);
        let e = download(&i, Component::Rea, &Seen::default())
            .await
            .unwrap_err();
        assert_eq!(e, ReverseError::Mismatch { part: "rea".into() }, "{delta}");
        assert!(!f.root.join("rea-6.9.0").exists());
    }
}

/// Counts fetches and records where each one started.
struct Counting {
    inner: DirFetch,
    calls: AtomicUsize,
    starts: Mutex<Vec<(String, u64)>>,
    before: Box<dyn Fn(&PackFile) + Send + Sync>,
}

impl Counting {
    fn new(dir: &Path) -> Self {
        Self {
            inner: DirFetch::new(dir),
            calls: AtomicUsize::new(0),
            starts: Mutex::new(Vec::new()),
            before: Box::new(|_| {}),
        }
    }
}

#[async_trait]
impl Fetch for Counting {
    async fn fetch(
        &self,
        file: &PackFile,
        dest: &Path,
        hosts: &'static [&'static str],
        cancel: &CancellationToken,
        on_bytes: &(dyn Fn(u64) + Send + Sync),
    ) -> Result<(), ReverseError> {
        (self.before)(file);
        self.calls.fetch_add(1, Ordering::SeqCst);
        let first = Mutex::new(None);
        let r = self
            .inner
            .fetch(file, dest, hosts, cancel, &|n| {
                first.lock().unwrap().get_or_insert(n);
                on_bytes(n);
            })
            .await;
        let start = first.lock().unwrap().unwrap_or(0);
        self.starts.lock().unwrap().push((file.name.clone(), start));
        r
    }
}

#[tokio::test]
async fn a_paused_download_resumes_where_it_stopped() {
    let f = fixture();
    let archive = f.archives.join(&f.part("kuna").builds[0].file.name);
    let bytes = std::fs::read(&archive).unwrap();
    let half = bytes.len() / 2;
    std::fs::create_dir_all(f.downloads()).unwrap();
    std::fs::write(f.part_file("kuna"), &bytes[..half]).unwrap();
    // rea first, so only Kuna's download is in play.
    let i = f.installer();
    download(&i, Component::Rea, &Seen::default())
        .await
        .unwrap();
    assert_eq!(
        i.state(Component::Kuna),
        ComponentState::Partial { bytes: half as u64 }
    );
    let counting = Arc::new(Counting::new(&f.archives));
    let i = f.installer().with_fetch(counting.clone() as Arc<dyn Fetch>);
    download(&i, Component::Kuna, &Seen::default())
        .await
        .unwrap();
    assert_eq!(i.state(Component::Kuna), ComponentState::Installed);
    let starts = counting.starts.lock().unwrap().clone();
    assert_eq!(
        starts,
        [(f.part("kuna").builds[0].file.name.clone(), half as u64)]
    );
}

/// Writes half of each file, then stops as if the user pressed Cancel.
struct HalfThenCancel(PathBuf);

#[async_trait]
impl Fetch for HalfThenCancel {
    async fn fetch(
        &self,
        file: &PackFile,
        dest: &Path,
        _hosts: &'static [&'static str],
        _cancel: &CancellationToken,
        on_bytes: &(dyn Fn(u64) + Send + Sync),
    ) -> Result<(), ReverseError> {
        let bytes = std::fs::read(self.0.join(&file.name)).unwrap();
        std::fs::write(dest, &bytes[..bytes.len() / 2]).unwrap();
        on_bytes((bytes.len() / 2) as u64);
        Err(ReverseError::Cancelled)
    }
}

#[tokio::test]
async fn cancel_keeps_a_partial_download_and_discard_frees_it() {
    let f = fixture();
    let i = f
        .installer()
        .with_fetch(Arc::new(HalfThenCancel(f.archives.clone())));
    let e = download(&i, Component::Rea, &Seen::default())
        .await
        .unwrap_err();
    assert_eq!(e, ReverseError::Cancelled);
    let half = f.part("rea").builds[0].file.size / 2;
    assert_eq!(
        i.state(Component::Rea),
        ComponentState::Partial { bytes: half }
    );
    assert!(!f.root.join("rea-6.9.0").exists());
    // The download slot is free again.
    i.discard_partial(Component::Rea).await.unwrap();
    assert_eq!(i.state(Component::Rea), ComponentState::NotInstalled);
    assert!(!f.part_file("rea").exists());
}

#[tokio::test]
async fn the_path_limit_counts_the_temporary_folder_on_windows() {
    let f = fixture();
    let max = f.part("ghidra").builds[0].max_path_chars;
    let tmp_name = ".tmp-ghidra-12.1.4";
    let base = f.root.parent().unwrap().to_string_lossy().chars().count();
    // root = <base>/<k letters>; tmp = <root>/<tmp_name>; longest = tmp + 1 + max.
    let k_ok = 250 - max - 1 - tmp_name.len() - 1 - base - 1;
    for (k, refused) in [(k_ok, false), (k_ok + 1, true)] {
        let root = f.root.parent().unwrap().join("x".repeat(k));
        let i = Installer::new(ReverseLayout::new(&root), Platform::WindowsX64)
            .with_lock(Arc::new(f.lock.clone()))
            .with_fetch(Arc::new(DirFetch::new(&f.archives)))
            .with_free_space(Arc::new(|_: &Path| Some(u64::MAX)))
            .with_fs_name(Arc::new(|_: &Path| Some("NTFS".to_string())));
        let r = i
            .download(Component::Ghidra, &CancellationToken::new(), &|_| {})
            .await;
        if refused {
            assert_eq!(
                r.unwrap_err(),
                ReverseError::PathTooLong {
                    chars: 251,
                    max: 250
                }
            );
            assert!(!root.join(".downloads").exists(), "nothing was downloaded");
        } else {
            assert!(r.is_ok(), "{r:?}");
        }
    }
    // The same root on Linux has no such limit.
    let root = f.root.parent().unwrap().join("x".repeat(k_ok + 1));
    let i = Installer::new(ReverseLayout::new(&root), Platform::LinuxX64)
        .with_lock(Arc::new(f.lock.clone()))
        .with_fetch(Arc::new(DirFetch::new(&f.archives)))
        .with_free_space(Arc::new(|_: &Path| Some(u64::MAX)));
    assert!(
        i.download(Component::Ghidra, &CancellationToken::new(), &|_| {})
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn ghidra_on_windows_needs_a_local_ntfs_or_refs_drive() {
    let f = fixture();
    let i = f
        .installer_with(f.lock.clone(), Platform::WindowsX64)
        .with_fs_name(Arc::new(|_: &Path| Some("FAT32".to_string())));
    assert_eq!(
        download(&i, Component::Ghidra, &Seen::default())
            .await
            .unwrap_err(),
        ReverseError::NotNtfs
    );
    // rea alone does not run Ghidra's runtimes from there.
    assert!(download(&i, Component::Rea, &Seen::default()).await.is_ok());
}

#[tokio::test]
async fn a_full_disk_is_refused_before_anything_is_downloaded() {
    let f = fixture();
    let counting = Arc::new(Counting::new(&f.archives));
    let i = f
        .installer()
        .with_fetch(counting.clone() as Arc<dyn Fetch>)
        .with_free_space(Arc::new(|_: &Path| Some(10)));
    let e = download(&i, Component::Rea, &Seen::default())
        .await
        .unwrap_err();
    let need: u64 = ["rea", "node"]
        .iter()
        .map(|n| {
            let b = &f.part(n).builds[0];
            b.file.size + b.unpacked_bytes
        })
        .sum();
    assert_eq!(
        e,
        ReverseError::DiskFull {
            need_bytes: need + need / 10
        }
    );
    assert_eq!(counting.calls.load(Ordering::SeqCst), 0);
    assert!(!f.downloads().exists());
}

#[tokio::test]
async fn the_marker_is_written_last() {
    let f = fixture();
    // A program whose size differs from the pin fails after unpacking: no folder, no
    // temporary folder, no marker anywhere.
    let mut lock = f.lock.clone();
    lock.parts
        .iter_mut()
        .find(|p| p.name == "rea")
        .unwrap()
        .builds[0]
        .programs[0]
        .size += 1;
    let i = f.installer_with(lock, Platform::LinuxX64);
    let e = download(&i, Component::Rea, &Seen::default())
        .await
        .unwrap_err();
    assert!(
        matches!(e, ReverseError::Archive { ref part, .. } if part == "rea"),
        "{e:?}"
    );
    assert!(!f.root.join("rea-6.9.0").exists());
    assert!(!f.root.join(".tmp-rea-6.9.0").exists());
    // A folder without its marker (a crash before the rename) is not installed, and the
    // next download replaces it.
    let half_done = f.root.join("rea-6.9.0");
    std::fs::create_dir_all(half_done.join("scripts")).unwrap();
    std::fs::write(half_done.join("scripts/rea.mjs"), b"half").unwrap();
    let i = f.installer();
    assert_ne!(i.state(Component::Rea), ComponentState::Installed);
    download(&i, Component::Rea, &Seen::default())
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(half_done.join("scripts/rea.mjs")).unwrap(),
        b"rea 6.9.0"
    );
    assert!(half_done.join(".complete").is_file());
}

#[tokio::test]
async fn a_missing_program_is_broken_and_a_download_repairs_it() {
    let f = fixture();
    let i = f.installer();
    download(&i, Component::Rea, &Seen::default())
        .await
        .unwrap();
    std::fs::remove_file(f.root.join("node-24.22.3/bin/node")).unwrap();
    assert_eq!(
        i.state(Component::Rea),
        ComponentState::Broken {
            why: BrokenWhy::Missing
        }
    );
    assert_eq!(
        i.usable(Component::Rea).unwrap_err(),
        ReverseError::Missing {
            part: "node".into()
        }
    );
    assert!(i.installation().rea.is_none());
    // A program with another size counts as missing too.
    std::fs::write(f.root.join("rea-6.9.0/scripts/rea.mjs"), b"changed!!!").unwrap();
    // Repair: the same Download brings back both.
    let seen = Seen::default();
    download(&i, Component::Rea, &seen).await.unwrap();
    assert_eq!(seen.parts(), ["rea", "node"]);
    assert_eq!(i.state(Component::Rea), ComponentState::Installed);
    // A whole part folder gone is broken, not "not installed".
    std::fs::remove_dir_all(f.root.join("node-24.22.3")).unwrap();
    assert_eq!(
        i.state(Component::Rea),
        ComponentState::Broken {
            why: BrokenWhy::Missing
        }
    );
}

#[tokio::test]
async fn a_program_the_system_refuses_to_open_is_blocked() {
    let f = fixture();
    download(&f.installer(), Component::Kuna, &Seen::default())
        .await
        .unwrap();
    let i = f.installer().with_opener(Arc::new(|p: &Path| {
        if p.ends_with("kuna") {
            Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
        } else {
            Ok(())
        }
    }));
    assert_eq!(
        i.state(Component::Kuna),
        ComponentState::Broken {
            why: BrokenWhy::Blocked
        }
    );
    assert_eq!(
        i.usable(Component::Kuna).unwrap_err(),
        ReverseError::Blocked {
            part: "kuna".into()
        }
    );
    assert_eq!(i.state(Component::Rea), ComponentState::Installed);
}

#[tokio::test]
async fn a_part_below_its_minimum_needs_an_update() {
    let f = fixture();
    download(&f.installer(), Component::Kuna, &Seen::default())
        .await
        .unwrap();
    // A later app pins Kuna 1.991 and raises the minimum for a security fix.
    let mut lock = f.lock.clone();
    let kuna = lock.parts.iter_mut().find(|p| p.name == "kuna").unwrap();
    kuna.version = "1.991".into();
    kuna.minimum_version = "1.991".into();
    kuna.dir = "kuna-1.991".into();
    let i = f.installer_with(lock, Platform::LinuxX64);
    assert_eq!(
        i.state(Component::Kuna),
        ComponentState::NeedsUpdate {
            why: UpdateWhy::SecurityFix {
                part: "kuna".into()
            }
        }
    );
    assert_eq!(
        i.usable(Component::Kuna).unwrap_err(),
        ReverseError::NeedsUpdate {
            part: "kuna".into()
        }
    );
    let status = i.status(Component::Kuna);
    assert_eq!(status.installed_version.as_deref(), Some("1.990"));
    assert_eq!(status.update.as_ref().map(|u| u.to.as_str()), Some("1.991"));
}

#[tokio::test]
async fn a_rea_outside_the_accepted_versions_needs_an_update() {
    let f = fixture();
    download(&f.installer(), Component::Rea, &Seen::default())
        .await
        .unwrap();
    let new = fake_part(
        &f.archives,
        &rea_part("6.10.0", b"rea 6.10.0"),
        &PLATFORMS,
        NPM,
    );
    let mut lock = f.lock.clone();
    let rea = lock.parts.iter_mut().find(|p| p.name == "rea").unwrap();
    *rea = Part {
        minimum_version: "6.0.0".into(),
        ..new.clone()
    };
    let i = f.installer_with(lock.clone(), Platform::LinuxX64);
    assert_eq!(
        i.state(Component::Rea),
        ComponentState::NeedsUpdate {
            why: UpdateWhy::NotAccepted {
                need: "6.10.0".into()
            }
        }
    );
    // With a reviewed fixture for 6.9.0 too, the old version keeps working with an update
    // offered.
    let rea = lock.parts.iter_mut().find(|p| p.name == "rea").unwrap();
    rea.accepted_rea_versions.push(AcceptedRea {
        version: "6.9.0".into(),
        schema_fixture: "rea-6.9.0-catalog.json".into(),
    });
    let i = f.installer_with(lock, Platform::LinuxX64);
    assert_eq!(i.state(Component::Rea), ComponentState::Installed);
    let update = i.status(Component::Rea).update.unwrap();
    assert_eq!(
        (update.from.as_str(), update.to.as_str()),
        ("6.9.0", "6.10.0")
    );
    assert_eq!(update.download_bytes, new.builds[0].file.size);
}

#[tokio::test]
async fn an_update_keeps_the_old_version_until_the_new_one_is_complete() {
    let f = fixture();
    download(&f.installer(), Component::Rea, &Seen::default())
        .await
        .unwrap();
    let new = fake_part(
        &f.archives,
        &rea_part("6.10.0", b"rea 6.10.0"),
        &PLATFORMS,
        NPM,
    );
    let mut lock = f.lock.clone();
    let rea = lock.parts.iter_mut().find(|p| p.name == "rea").unwrap();
    *rea = Part {
        minimum_version: "6.0.0".into(),
        accepted_rea_versions: vec![
            AcceptedRea {
                version: "6.10.0".into(),
                schema_fixture: "rea-6.10.0-catalog.json".into(),
            },
            AcceptedRea {
                version: "6.9.0".into(),
                schema_fixture: "rea-6.9.0-catalog.json".into(),
            },
        ],
        ..new
    };
    let old = f.root.join("rea-6.9.0");
    // (a) The new archive cannot be fetched: the old version stays and works.
    let missing = tempfile::tempdir().unwrap();
    let i = f
        .installer_with(lock.clone(), Platform::LinuxX64)
        .with_fetch(Arc::new(DirFetch::new(missing.path())));
    let e = i
        .update(Component::Rea, &CancellationToken::new(), &|_| {})
        .await;
    assert!(matches!(e, Err(ReverseError::CantReach { .. })), "{e:?}");
    assert!(old.join(".complete").is_file());
    assert_eq!(i.state(Component::Rea), ComponentState::Installed);
    // (b) A damaged new archive: the old version stays.
    let mut bad = lock.clone();
    bad.parts
        .iter_mut()
        .find(|p| p.name == "rea")
        .unwrap()
        .builds[0]
        .file
        .sha256 = "f".repeat(64);
    let i = f.installer_with(bad, Platform::LinuxX64);
    assert!(
        i.update(Component::Rea, &CancellationToken::new(), &|_| {})
            .await
            .is_err()
    );
    assert!(old.join(".complete").is_file());
    // (c) Success: the old folder still exists while the new one downloads, and is gone
    // afterwards; Node, unchanged, is not fetched again.
    let mut counting = Counting::new(&f.archives);
    let old_seen = Arc::new(Mutex::new(Vec::new()));
    let (o, s) = (old.clone(), old_seen.clone());
    counting.before =
        Box::new(move |file: &PackFile| s.lock().unwrap().push((file.name.clone(), o.exists())));
    let counting = Arc::new(counting);
    let i = f
        .installer_with(lock, Platform::LinuxX64)
        .with_fetch(counting.clone() as Arc<dyn Fetch>);
    let installed = i
        .update(Component::Rea, &CancellationToken::new(), &|_| {})
        .await
        .unwrap();
    assert_eq!(installed.version, "6.10.0");
    assert_eq!(
        old_seen.lock().unwrap().clone(),
        [("rea-agents-6.10.0.tgz".to_string(), true)]
    );
    assert!(!old.exists());
    assert_eq!(
        std::fs::read(f.root.join("rea-6.10.0/scripts/rea.mjs")).unwrap(),
        b"rea 6.10.0"
    );
    assert!(f.root.join("node-24.22.3/.complete").is_file());
    assert_eq!(i.status(Component::Rea).update, None);
}

#[tokio::test]
async fn remove_touches_only_its_folders() {
    let f = fixture();
    let i = f.installer();
    download(&i, Component::Kuna, &Seen::default())
        .await
        .unwrap();
    download(&i, Component::Ghidra, &Seen::default())
        .await
        .unwrap();
    for d in ["home", "tmp", "work", "cache/kuna", "rea-agents", "other"] {
        std::fs::create_dir_all(f.root.join(d)).unwrap();
    }
    std::fs::write(f.root.join("cache/kuna/project"), b"saved").unwrap();
    std::fs::write(f.root.join("notes.txt"), b"mine").unwrap();
    std::fs::write(f.root.join("other/keep"), b"mine").unwrap();
    assert_eq!(i.cache_bytes(), 5);

    i.remove(Component::Kuna).await.unwrap();
    assert!(!f.root.join("kuna-1.990").exists());
    assert!(!f.root.join("cache/kuna").exists());
    assert_eq!(i.state(Component::Kuna), ComponentState::NotInstalled);
    for d in [
        "rea-6.9.0",
        "node-24.22.3",
        "ghidra-12.1.4",
        "java-21.0.13+9",
    ] {
        assert!(f.root.join(d).join(".complete").is_file(), "{d}");
    }

    i.remove(Component::Ghidra).await.unwrap();
    assert!(!f.root.join("ghidra-12.1.4").exists() && !f.root.join("java-21.0.13+9").exists());
    assert_eq!(i.state(Component::Rea), ComponentState::Installed);

    i.remove(Component::Rea).await.unwrap();
    for d in [
        "rea-6.9.0",
        "node-24.22.3",
        "home",
        "tmp",
        "work",
        "cache",
        ".downloads",
    ] {
        assert!(!f.root.join(d).exists(), "{d}");
    }
    // Names that are not the reverse folder's own stay.
    assert!(f.root.join("notes.txt").is_file());
    assert!(f.root.join("other/keep").is_file());
    assert!(f.root.join("rea-agents").is_dir());
    // Removing again, or with no folder at all, is fine.
    i.remove(Component::Rea).await.unwrap();
}

#[tokio::test]
async fn a_second_download_of_the_same_row_is_busy() {
    let f = fixture();
    let i = Arc::new(f.installer().with_fetch(Arc::new(Slow(f.archives.clone()))));
    let first = {
        let i = i.clone();
        tokio::spawn(async move {
            i.download(Component::Rea, &CancellationToken::new(), &|_| {})
                .await
        })
    };
    // Wait until the first one shows as downloading.
    for _ in 0..200 {
        if matches!(i.state(Component::Rea), ComponentState::Downloading { .. }) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(matches!(
        i.state(Component::Rea),
        ComponentState::Downloading { .. }
    ));
    assert_eq!(
        i.download(Component::Rea, &CancellationToken::new(), &|_| {})
            .await
            .unwrap_err(),
        ReverseError::Busy
    );
    // A decompiler that would bring rea along is busy too.
    assert_eq!(
        i.download(Component::Kuna, &CancellationToken::new(), &|_| {})
            .await
            .unwrap_err(),
        ReverseError::Busy
    );
    // Removing a row while it downloads is refused too.
    assert_eq!(
        i.remove(Component::Rea).await.unwrap_err(),
        ReverseError::Busy
    );
    first.await.unwrap().unwrap();
    assert_eq!(i.state(Component::Rea), ComponentState::Installed);
    i.remove(Component::Rea).await.unwrap();
}

/// A fetch that waits a little before copying.
struct Slow(PathBuf);

#[async_trait]
impl Fetch for Slow {
    async fn fetch(
        &self,
        file: &PackFile,
        dest: &Path,
        hosts: &'static [&'static str],
        cancel: &CancellationToken,
        on_bytes: &(dyn Fn(u64) + Send + Sync),
    ) -> Result<(), ReverseError> {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        DirFetch::new(&self.0)
            .fetch(file, dest, hosts, cancel, on_bytes)
            .await
    }
}

#[tokio::test]
async fn a_row_not_offered_here_is_refused() {
    let f = fixture();
    let i = f.installer_with(f.lock.clone(), Platform::WindowsX64);
    assert_eq!(
        download(&i, Component::Kuna, &Seen::default())
            .await
            .unwrap_err(),
        ReverseError::NotAvailable {
            reason: "rea can't use Kuna on Windows yet.".into()
        }
    );
    let s = i.status(Component::Kuna);
    assert!(!s.available);
    assert_eq!(s.download_bytes, 0);
}

/// The real pinned archives through the real unpacker, from a cache filled by
/// `scripts/pin_reverse.py --download-to <dir>` (about 830 MB on Linux; 2.5 GB of disk while
/// it runs). Every shipped platform whose archives are all in the cache is installed (the
/// pin's own download folder holds all of them), so the Windows and Mac archives go through
/// the unpacker on any system.
#[tokio::test(flavor = "multi_thread")]
async fn the_pinned_archives_install_from_a_cache() {
    let Some(cache) = std::env::var_os("REVERSE_PACK_CACHE").map(PathBuf::from) else {
        eprintln!("REVERSE_PACK_CACHE is not set; skipped");
        return;
    };
    let lock = reverse::lock::embedded();
    let mut tried = 0;
    for platform in Platform::SHIPPED {
        let complete = lock
            .parts
            .iter()
            .filter_map(|p| p.build_for(platform))
            .all(|b| cache.join(&b.file.name).is_file());
        if !complete {
            eprintln!(
                "{}: not every archive is in the cache; skipped",
                platform.key()
            );
            continue;
        }
        tried += 1;
        let dir = tempfile::tempdir().unwrap();
        let i = Installer::new(ReverseLayout::new(dir.path().join("reverse")), platform)
            .with_fetch(Arc::new(DirFetch::new(&cache)));
        for c in Component::ALL {
            if !i.status(c).available {
                continue;
            }
            let started = std::time::Instant::now();
            let installed = i
                .download(c, &CancellationToken::new(), &|_| {})
                .await
                .unwrap_or_else(|e| panic!("{} {c:?}: {e}", platform.key()));
            eprintln!(
                "{} {c:?} installed in {:?}",
                platform.key(),
                started.elapsed()
            );
            assert_eq!(i.state(c), ComponentState::Installed, "{c:?}");
            for part in &installed.parts {
                let pinned = lock.part(&part.name).unwrap().build_for(platform).unwrap();
                assert_eq!(part.disk_bytes, pinned.unpacked_bytes, "{}", part.name);
                for p in &pinned.programs {
                    let path = part.dir.join(&p.path);
                    assert_eq!(
                        std::fs::metadata(&path).unwrap().len(),
                        p.size,
                        "{}",
                        p.path
                    );
                    #[cfg(unix)]
                    if p.exec {
                        assert!(is_executable(&path), "{} is not executable", p.path);
                    }
                }
            }
        }
        let inst = i.installation();
        assert!(inst.node().is_some_and(|n| n.is_file()));
        assert!(inst.rea_entry.as_ref().is_some_and(|e| e.is_file()));
    }
    assert!(tried > 0, "the cache holds no complete platform");
}

/// The real download path (`NetFetch`: the preflight, `runtime::packs::download_verified`,
/// GitHub's redirect to its asset host) against the pinned URLs, when
/// `REVERSE_NET_TEST=1`: rea, Node.js and, where offered, Kuna (about 125 MB on Linux).
#[tokio::test(flavor = "multi_thread")]
async fn the_pinned_downloads_come_from_their_hosts() {
    if std::env::var_os("REVERSE_NET_TEST").is_none() {
        eprintln!("REVERSE_NET_TEST is not set; skipped");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let i = Installer::new(
        ReverseLayout::new(dir.path().join("reverse")),
        Platform::current(),
    );
    let row = if i.status(Component::Kuna).available {
        Component::Kuna
    } else {
        Component::Rea
    };
    let seen = Seen::default();
    let started = std::time::Instant::now();
    download(&i, row, &seen)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    eprintln!(
        "{row:?} with rea downloaded and installed in {:?}",
        started.elapsed()
    );
    assert_eq!(i.state(Component::Rea), ComponentState::Installed);
    assert_eq!(i.state(row), ComponentState::Installed);
    let last = seen.0.lock().unwrap().last().cloned().unwrap();
    assert_eq!(last.stage, DownloadStage::Unpacking);
}
