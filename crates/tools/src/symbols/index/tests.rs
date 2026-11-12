use super::*;

struct Tree {
    _dir: tempfile::TempDir,
    root: PathBuf,
    outside: PathBuf,
}

fn write(path: &Path, text: impl AsRef<[u8]>) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A project with code, notes, and everything the walk has to leave out.
fn tree() -> Tree {
    let dir = tempfile::tempdir().unwrap();
    let base = dunce::canonicalize(dir.path()).unwrap();
    let root = base.join("project");
    let outside = base.join("elsewhere");
    write(&root.join("notes.txt"), "plain notes\n");
    write(&root.join("calc.py"), "def add(a, b):\n    return a + b\n");
    write(
        &root.join("src/lib.rs"),
        "pub fn parse_args() {}\nstruct Point;\n",
    );
    write(
        &root.join("src/deep/more/far.ts"),
        "export function far() {}\n",
    );
    for skipped in [
        ".git/config",
        "node_modules/x/index.js",
        "target/debug/a.rs",
        "dist/bundle.js",
        "build/out.py",
        "__pycache__/c.py",
        ".ssh/id_rsa",
    ] {
        write(&root.join(skipped), "def hidden_def():\n    pass\n");
    }
    write(&root.join("big.py"), "x = 1\n".repeat(200_000));
    write(&root.join("blob.dat"), b"abc\0def");
    write(&root.join("image.png"), b"not really an image");
    write(&root.join(".bashrc"), "export A=1\n");
    write(&outside.join("secret.py"), "def secret():\n    pass\n");
    Tree {
        _dir: dir,
        root,
        outside,
    }
}

fn built(root: &Path, forbidden: &[String], skills: &[String]) -> Arc<FolderIndex> {
    let idx = FolderIndex::new(&root.display().to_string(), forbidden, skills);
    idx.build_now();
    idx
}

fn rels(idx: &FolderIndex) -> Vec<String> {
    idx.snapshot()
        .files
        .iter()
        .filter(|f| !f.hidden)
        .map(|f| f.rel.to_string())
        .collect()
}

#[test]
fn the_walk_leaves_out_what_mentions_cannot_read() {
    let t = tree();
    let idx = built(&t.root, &[], &[]);
    let snap = idx.snapshot();
    assert!(snap.complete && !snap.truncated);
    // Breadth first: shallow files before deep ones.
    assert_eq!(
        rels(&idx),
        vec!["calc.py", "notes.txt", "src/lib.rs", "src/deep/more/far.ts"]
    );
}

#[cfg(unix)]
#[test]
fn links_are_never_followed_out_of_the_folder() {
    let t = tree();
    std::os::unix::fs::symlink(&t.outside, t.root.join("away")).unwrap();
    std::os::unix::fs::symlink(t.outside.join("secret.py"), t.root.join("secret.py")).unwrap();
    let idx = built(&t.root, &[], &[]);
    assert!(!rels(&idx).iter().any(|r| r.contains("secret")));
    let found = search(&[idx], "secret", 20);
    assert!(found.hits.is_empty(), "{:?}", found.hits);
}

#[test]
fn forbidden_and_skill_folders_are_not_entered() {
    let t = tree();
    write(&t.root.join("appdata/keys.py"), "def keys():\n    pass\n");
    write(&t.root.join("skills/x/run.py"), "def skill():\n    pass\n");
    let idx = built(
        &t.root,
        &[t.root.join("appdata").display().to_string()],
        &[t.root.join("skills").display().to_string()],
    );
    let all = rels(&idx);
    assert!(
        !all.iter()
            .any(|r| r.starts_with("appdata") || r.starts_with("skills")),
        "{all:?}"
    );
    // An approved folder that is itself forbidden gives nothing.
    let inner = built(
        &t.root.join("appdata"),
        &[t.root.join("appdata").display().to_string()],
        &[],
    );
    assert!(rels(&inner).is_empty());
}

#[test]
fn symbols_and_files_are_found_by_name() {
    let t = tree();
    let idx = built(&t.root, &[], &[]);
    let found = search(std::slice::from_ref(&idx), "add", 20);
    assert!(!found.indexing);
    let first = &found.hits[0];
    assert_eq!(first.rel, "calc.py");
    let sym = first.symbol.as_ref().unwrap();
    assert_eq!(
        (sym.name.as_str(), sym.kind, sym.line, sym.end_line),
        ("add", SymbolKind::Function, 1, 2)
    );
    assert_eq!(first.path, t.root.join("calc.py"));

    // Files by a fuzzy match on the path.
    let found = search(std::slice::from_ref(&idx), "srclib", 20);
    assert_eq!(found.hits[0].rel, "src/lib.rs");
    assert!(found.hits[0].symbol.is_none());
    let found = search(std::slice::from_ref(&idx), "far", 20);
    assert_eq!(found.hits[0].rel, "src/deep/more/far.ts");

    // `path#name`: definitions of matching files only.
    let found = search(std::slice::from_ref(&idx), "lib#Po", 20);
    assert_eq!(found.hits.len(), 1);
    assert_eq!(found.hits[0].symbol.as_ref().unwrap().name, "Point");
    let found = search(std::slice::from_ref(&idx), "calc#", 20);
    assert_eq!(found.hits.len(), 1);

    // Names from skipped places are never found.
    assert!(
        search(std::slice::from_ref(&idx), "hidden_def", 20)
            .hits
            .is_empty()
    );
    assert!(
        search(std::slice::from_ref(&idx), "bashrc", 20)
            .hits
            .is_empty()
    );
    // The limit holds.
    assert_eq!(search(&[idx], "a", 2).hits.len(), 2);
}

#[test]
fn an_empty_query_lists_recently_changed_files() {
    let t = tree();
    let old = SystemTime::now() - Duration::from_secs(3600);
    for f in ["calc.py", "src/lib.rs", "src/deep/more/far.ts"] {
        std::fs::File::options()
            .write(true)
            .open(t.root.join(f))
            .unwrap()
            .set_modified(old)
            .unwrap();
    }
    let idx = built(&t.root, &[], &[]);
    let found = search(&[idx], "", 20);
    assert_eq!(found.hits[0].rel, "notes.txt");
    assert!(found.hits.iter().all(|h| h.symbol.is_none()));
}

#[test]
fn a_rebuild_reads_again_only_files_that_changed() {
    let t = tree();
    let idx = built(&t.root, &[], &[]);
    let calc = t.root.join("calc.py");
    let lib = t.root.join("src/lib.rs");
    let lib_time = std::fs::metadata(&lib).unwrap().modified().unwrap();
    // Same size and time: the cached definitions are kept (the file is not read again).
    std::fs::write(&lib, "pub fn parse_argz() {}\nstruct Point;\n").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&lib)
        .unwrap()
        .set_modified(lib_time)
        .unwrap();
    // A changed size: read again.
    std::fs::write(
        &calc,
        "def add(a, b):\n    return a + b\n\n\ndef mul(a, b):\n    return a * b\n",
    )
    .unwrap();
    idx.build_now();
    let names = |q: &str| -> Vec<String> {
        search(std::slice::from_ref(&idx), q, 20)
            .hits
            .iter()
            .filter_map(|h| h.symbol.as_ref().map(|s| s.name.clone()))
            .collect()
    };
    assert_eq!(names("mul"), vec!["mul"]);
    assert_eq!(names("parse_args"), vec!["parse_args"]);
    assert!(names("parse_argz").is_empty());
}

#[test]
fn the_file_cap_holds() {
    let t = tree();
    for i in 0..30 {
        write(&t.root.join(format!("many/f{i:02}.txt")), "x\n");
    }
    let idx = FolderIndex::with_max_files(&t.root.display().to_string(), 10);
    idx.build_now();
    let snap = idx.snapshot();
    assert_eq!(snap.files.len(), 10);
    assert!(snap.truncated);
}

#[test]
fn indexes_are_shared_per_folder_nested_folders_are_covered_and_few_are_kept() {
    let t = tree();
    let indexes = Indexes::new();
    let root = t.root.display().to_string();
    let src = t.root.join("src").display().to_string();
    let a = indexes.for_roots(&[root.clone(), src], &[], &[]);
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].root(), t.root.as_path());
    let b = indexes.for_roots(std::slice::from_ref(&root), &[], &[]);
    assert!(Arc::ptr_eq(&a[0], &b[0]));
    a[0].wait(Duration::from_secs(10));
    // A fifth folder drops the least recently used one.
    let mut others = Vec::new();
    for i in 0..MAX_FOLDERS {
        let p = t.outside.join(format!("o{i}"));
        std::fs::create_dir_all(&p).unwrap();
        others.push(
            indexes
                .for_roots(&[p.display().to_string()], &[], &[])
                .remove(0),
        );
    }
    for o in &others {
        o.wait(Duration::from_secs(10));
    }
    let c = indexes.for_roots(std::slice::from_ref(&root), &[], &[]);
    assert!(
        !Arc::ptr_eq(&a[0], &c[0]),
        "the first folder was dropped and built anew"
    );
    assert!(a[0].cancelled.load(Ordering::Acquire));
    c[0].wait(Duration::from_secs(10));
}

#[test]
fn a_search_answers_while_the_index_is_building() {
    let t = tree();
    let indexes = Indexes::new();
    let idx = indexes.for_roots(&[t.root.display().to_string()], &[], &[]);
    // Whatever is there now, at once; then everything once the build is done.
    let _ = search(&idx, "add", 20);
    idx[0].wait(Duration::from_secs(10));
    let found = search(&idx, "add", 20);
    assert!(!found.indexing);
    assert_eq!(found.hits[0].rel, "calc.py");
}

/// Timing on this repository (run with `--ignored --nocapture`; the numbers are reported).
#[test]
#[ignore = "timing measurement, run by hand"]
fn timing_on_this_repository() {
    // Another folder can be measured with MENTION_TIMING_DIR.
    let dir = std::env::var("MENTION_TIMING_DIR")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../..").to_string());
    let repo = dunce::canonicalize(dir).unwrap();
    let started = Instant::now();
    let idx = FolderIndex::new(&repo.display().to_string(), &[], &[]);
    let (names, _) = idx.list();
    let listed = started.elapsed();
    idx.build_now();
    let built = started.elapsed();
    let snap = idx.snapshot();
    let symbols: usize = snap.files.iter().map(|f| f.symbols().len()).sum();
    println!(
        "repository: {} files listed in {listed:?}; {} files and {symbols} definitions indexed in {built:?}",
        names.len(),
        snap.files.len()
    );
    for q in [
        "a",
        "add",
        "parse",
        "src/lib",
        "Composer",
        "mentions#res",
        "",
    ] {
        let t = Instant::now();
        let found = search(std::slice::from_ref(&idx), q, 30);
        println!(
            "search {q:?}: {} hits in {:?}",
            found.hits.len(),
            t.elapsed()
        );
    }
    // A rebuild with nothing changed.
    let t = Instant::now();
    idx.build_now();
    println!("rebuild with nothing changed: {:?}", t.elapsed());
}
