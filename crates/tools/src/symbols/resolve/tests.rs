use super::*;
use crate::FilesServer;
use mcp_host::{BuiltinServer, Described};
use std::path::PathBuf;

/// An approved folder holding `calc.py` and `notes.txt`, and a folder next to it that is
/// not approved, holding `secret.txt`.
struct Setup {
    _dir: tempfile::TempDir,
    root: PathBuf,
    outside: PathBuf,
}

const CALC: &str = "\"\"\"Small sums.\"\"\"\n\n\ndef add(a, b):\n    return a + b\n\n\ndef sub(a, b):\n    return a - b\n";

fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let base = dunce::canonicalize(dir.path()).unwrap();
    let root = base.join("project");
    let outside = base.join("elsewhere");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(root.join("calc.py"), CALC).unwrap();
    std::fs::write(root.join("notes.txt"), "first\nsecond\nthird\n").unwrap();
    std::fs::write(outside.join("secret.txt"), "do not read\n").unwrap();
    Setup {
        _dir: dir,
        root,
        outside,
    }
}

fn chat(s: &Setup) -> ChatContext {
    ChatContext {
        chat_id: 1,
        approved_folders: vec![s.root.display().to_string()],
        working_folder: Some(s.root.display().to_string()),
        result_cap_tokens: 2000,
        ..Default::default()
    }
}

fn file(path: impl Into<String>) -> MentionRequest {
    MentionRequest {
        path: path.into(),
        ..Default::default()
    }
}

fn refused(s: &Setup, c: &ChatContext, req: &MentionRequest) -> String {
    match resolve(req, c, 10_000) {
        Ok(r) => panic!("{} must be refused; got {:?}", req.path, r.text),
        Err(why) => {
            assert!(!why.contains("do not read"), "{why}");
            let _ = s;
            why
        }
    }
}

#[test]
fn a_path_outside_the_approved_folders_is_refused() {
    let s = setup();
    let c = chat(&s);
    let why = refused(
        &s,
        &c,
        &file(s.outside.join("secret.txt").display().to_string()),
    );
    assert!(why.contains("outside the approved folders"), "{why}");
}

#[test]
fn a_path_that_climbs_out_with_dot_dot_is_refused() {
    let s = setup();
    let c = chat(&s);
    for p in [
        "../elsewhere/secret.txt",
        "./sub/../../elsewhere/secret.txt",
    ] {
        let why = refused(&s, &c, &file(p));
        assert!(why.contains("outside the approved folders"), "{p}: {why}");
    }
}

#[cfg(unix)]
#[test]
fn a_symlink_that_leaves_the_folder_is_refused() {
    let s = setup();
    let c = chat(&s);
    std::os::unix::fs::symlink(s.outside.join("secret.txt"), s.root.join("link.txt")).unwrap();
    std::os::unix::fs::symlink(&s.outside, s.root.join("away")).unwrap();
    for p in ["link.txt", "away/secret.txt"] {
        let why = refused(&s, &c, &file(p));
        assert!(why.contains("outside the approved folders"), "{p}: {why}");
    }
    // A link that stays inside is followed like read_file follows it.
    std::os::unix::fs::symlink(s.root.join("notes.txt"), s.root.join("inner.txt")).unwrap();
    let r = resolve(&file("inner.txt"), &c, 10_000).unwrap();
    assert_eq!(r.canonical, s.root.join("notes.txt").display().to_string());
}

#[test]
fn a_forbidden_folder_is_refused_even_inside_an_approved_one() {
    let s = setup();
    let app_data = s.root.join("appdata");
    std::fs::create_dir_all(&app_data).unwrap();
    std::fs::write(app_data.join("keys.txt"), "do not read\n").unwrap();
    let c = ChatContext {
        forbidden_folders: vec![app_data.display().to_string()],
        ..chat(&s)
    };
    let why = refused(&s, &c, &file("appdata/keys.txt"));
    assert!(why.contains("app's own folders"), "{why}");
}

#[test]
fn sensitive_places_and_skill_folders_are_refused() {
    let s = setup();
    for (dir, name) in [(".git", "config"), (".ssh", "id_ed25519"), ("", ".bashrc")] {
        let folder = s.root.join(dir);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join(name), "do not read\n").unwrap();
    }
    let skills = s.root.join("skills");
    std::fs::create_dir_all(skills.join("x")).unwrap();
    std::fs::write(skills.join("x/SKILL.md"), "do not read\n").unwrap();
    let c = ChatContext {
        skill_roots: vec![skills.display().to_string()],
        ..chat(&s)
    };
    for p in [".git/config", ".ssh/id_ed25519", ".bashrc"] {
        let why = refused(&s, &c, &file(p));
        assert!(why.contains("protected place"), "{p}: {why}");
    }
    let why = refused(&s, &c, &file("skills/x/SKILL.md"));
    assert!(why.contains("skills folder"), "{why}");
}

#[test]
fn a_file_that_is_too_large_is_refused() {
    let s = setup();
    let c = chat(&s);
    let big = "a".repeat(RAW_LIMIT as usize + 1);
    std::fs::write(s.root.join("big.txt"), big).unwrap();
    let why = refused(&s, &c, &file("big.txt"));
    assert!(why.contains("too large"), "{why}");
}

#[test]
fn a_binary_file_is_refused() {
    let s = setup();
    let c = chat(&s);
    std::fs::write(s.root.join("image.png"), b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR").unwrap();
    let why = refused(&s, &c, &file("image.png"));
    assert!(why.contains("binary"), "{why}");
}

#[test]
fn a_folder_a_missing_file_and_no_approved_folder_are_refused() {
    let s = setup();
    let c = chat(&s);
    std::fs::create_dir_all(s.root.join("src")).unwrap();
    assert!(refused(&s, &c, &file("src")).contains("is a folder"));
    assert!(refused(&s, &c, &file("gone.txt")).contains("was not found"));
    let none = ChatContext {
        approved_folders: vec![],
        working_folder: None,
        ..chat(&s)
    };
    let why = refused(
        &s,
        &none,
        &file(s.root.join("notes.txt").display().to_string()),
    );
    assert!(why.contains("outside the approved folders"), "{why}");
}

#[test]
fn a_whole_file_comes_with_its_path_and_numbered_lines() {
    let s = setup();
    let c = chat(&s);
    let r = resolve(&file("notes.txt"), &c, 10_000).unwrap();
    let abs = s.root.join("notes.txt").display().to_string();
    assert_eq!(r.label, "notes.txt");
    assert_eq!(r.kind, "file");
    assert_eq!((r.start_line, r.end_line, r.total_lines), (1, 3, 3));
    assert!(!r.truncated);
    assert_eq!(
        r.text,
        format!("File: {abs} (3 lines)\n     1\tfirst\n     2\tsecond\n     3\tthird\n")
    );
}

#[test]
fn a_long_file_sends_its_start_and_says_what_was_left_out() {
    let s = setup();
    let c = chat(&s);
    let text: String = (1..=500).map(|i| format!("line number {i}\n")).collect();
    std::fs::write(s.root.join("long.txt"), text).unwrap();
    let r = resolve(&file("long.txt"), &c, 2_000).unwrap();
    assert!(r.truncated);
    assert!(r.text.len() <= 2_000, "{}", r.text.len());
    assert!(r.text.contains("     1\tline number 1\n"));
    let last = r
        .text
        .lines()
        .filter(|l| l.contains("\tline number"))
        .count();
    let note = format!(
        "[Lines {}-500 of the file were left out ({} lines). The file tools can read more: call read_file with offset={}.]",
        last + 1,
        500 - last,
        last + 1
    );
    assert!(r.text.contains(&note), "{}", r.text);
}

#[test]
fn a_symbol_comes_with_its_definition_lines_path_and_range() {
    let s = setup();
    let c = chat(&s);
    let req = MentionRequest {
        path: "calc.py".into(),
        symbol: Some("add".into()),
        kind: Some("function".into()),
        line: Some(4),
        ..Default::default()
    };
    let r = resolve(&req, &c, 10_000).unwrap();
    let abs = s.root.join("calc.py").display().to_string();
    assert_eq!(r.label, "calc.py#add");
    assert_eq!((r.start_line, r.end_line, r.total_lines), (4, 5, 9));
    assert_eq!(
        r.text,
        format!(
            "File: {abs}, lines 4-5 (function add)\n     4\tdef add(a, b):\n     5\t    return a + b\n"
        )
    );
    // Looked up again in the file as it is now: it moved down two lines.
    std::fs::write(s.root.join("calc.py"), format!("import os\n\n{CALC}")).unwrap();
    let r = resolve(&req, &c, 10_000).unwrap();
    assert_eq!((r.start_line, r.end_line), (6, 7));
    // Gone: refused, not some other text.
    std::fs::write(s.root.join("calc.py"), "def other():\n    pass\n").unwrap();
    assert!(
        resolve(&req, &c, 10_000)
            .unwrap_err()
            .contains("was not found")
    );
}

#[test]
fn a_method_is_found_by_its_qualified_name_and_the_nearest_line() {
    let s = setup();
    let c = chat(&s);
    let src = "struct A;\nimpl A {\n    fn new() -> Self { A }\n}\nstruct B;\nimpl B {\n    fn new() -> Self { B }\n}\n";
    std::fs::write(s.root.join("lib.rs"), src).unwrap();
    let req = MentionRequest {
        path: "lib.rs".into(),
        symbol: Some("B::new".into()),
        ..Default::default()
    };
    let r = resolve(&req, &c, 10_000).unwrap();
    assert_eq!((r.start_line, r.label.as_str()), (7, "lib.rs#B::new"));
    let near = MentionRequest {
        symbol: Some("new".into()),
        line: Some(3),
        ..req
    };
    assert_eq!(resolve(&near, &c, 10_000).unwrap().start_line, 3);
}

#[test]
fn names_are_relative_to_the_working_folder_or_start_with_the_folder_name() {
    let roots = vec!["/w/app".to_string(), "/w/lib".to_string()];
    assert_eq!(
        display_path("/w/app/src/a.rs", &roots, Some("/w/app")),
        "src/a.rs"
    );
    assert_eq!(
        display_path("/w/lib/b.rs", &roots, Some("/w/app")),
        "lib/b.rs"
    );
    assert_eq!(display_path("/w/app/c.rs", &roots, None), "app/c.rs");
}

#[test]
fn the_share_of_the_context_is_capped() {
    // 8,000 tokens at most.
    assert_eq!(cap_chars(200_000, 1), 24_000);
    // A fifth of the context.
    assert_eq!(cap_chars(8_192, 1), 1_638 * 3);
    // 40% shared by the mentions.
    assert_eq!(cap_chars(8_192, 4), 819 * 3);
    // Never nothing.
    assert_eq!(cap_chars(100, 8), 600);
}

/// What a mention can read, `read_file` can read too (no hard limit at its gate).
#[tokio::test]
async fn whatever_a_mention_reads_read_file_may_read_too() {
    let s = setup();
    let c = chat(&s);
    let files = FilesServer::default();
    for p in ["notes.txt", "calc.py"] {
        let r = resolve(&file(p), &c, 10_000).unwrap();
        let args = serde_json::json!({ "path": r.canonical });
        match files
            .describe("read_file", args.as_object().unwrap(), &c)
            .await
        {
            Described::Action(a) => assert!(a.hard_limit.is_none(), "{p}: {:?}", a.hard_limit),
            other => panic!("{p}: {other:?}"),
        }
    }
}

/// A file of one line longer than the cap: its start is sent, the mention is marked cut,
/// and the note says so.
#[test]
fn a_single_line_longer_than_the_cap_is_marked_cut_with_a_note() {
    let s = setup();
    let c = chat(&s);
    std::fs::write(
        s.root.join("big.rs"),
        format!("fn big() {{ {} }}\n", "x".repeat(50_000)),
    )
    .unwrap();
    let r = resolve(&file("big.rs"), &c, 2_000).unwrap();
    assert!(r.truncated);
    assert!(r.text.len() <= 2_000, "{}", r.text.len());
    assert!(r.text.contains("     1\tfn big() { xxx"), "{}", r.text);
    assert!(
        r.text.ends_with(
            "…\n[Line 1 is longer than a mention can hold, so only its start is shown. The file tools can read the file: call read_file with offset=1.]\n"
        ),
        "{}",
        &r.text[r.text.len() - 300..]
    );
}

/// The open itself refuses a link as the last part of the path, even when the earlier
/// check passed (a swap between the check and the open).
#[cfg(unix)]
#[test]
fn the_open_refuses_a_link_swapped_in_after_the_check() {
    let s = setup();
    let real = s.root.join("notes.txt");
    let before = std::fs::symlink_metadata(&real).unwrap();
    // Kept open, so the file system cannot give its inode number to the file swapped in.
    let _keep = std::fs::File::open(&real).unwrap();
    // Swapped: notes.txt is now a link to a file outside.
    std::fs::remove_file(&real).unwrap();
    std::os::unix::fs::symlink(s.outside.join("secret.txt"), &real).unwrap();
    let err = open_checked(&real, &before).unwrap_err();
    assert!(!err.contains("do not read"), "{err}");
    // Swapped for another regular file: not the file that was checked.
    std::fs::remove_file(&real).unwrap();
    std::fs::write(&real, "another file\n").unwrap();
    assert!(open_checked(&real, &before).is_err());
    // The file that was checked opens, and its size and kind come from the handle.
    let now = std::fs::symlink_metadata(&real).unwrap();
    let (_, meta) = open_checked(&real, &now).unwrap();
    assert!(meta.is_file());
    assert_eq!(meta.len(), "another file\n".len() as u64);
}
