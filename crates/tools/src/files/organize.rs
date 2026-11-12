//! The file tools that organise the computer (app 0.0.5): `make_folder`, `move_path` and
//! `delete_folder`.
//!
//! - They work only while the chat is in Full access (`ChatContext::full_access`, read for
//!   every call). The app offers them to the model only then; a call in any other mode is
//!   answered with [`ONLY_IN_FULL_ACCESS`] and nothing is touched.
//! - Every path goes through the same sandbox as the other file tools, so the app's own
//!   folders (its data, which holds the database, its logs and its install folder) are never
//!   reached. A folder that *contains* one of them is never moved or deleted either: moving
//!   or deleting the home folder would take the app's data with it. The same holds for the
//!   folders skills are loaded from.
//! - Nothing is replaced: a move to a name that exists is refused. A move into an existing
//!   folder puts the item inside it, under its own name.
//! - A filesystem or drive root is never moved or deleted with these tools.
//! - What runs is what was described: both paths are checked again right before the change
//!   (sandbox step 5), and so are the folders around them.

use super::*;
use policy::path::{CaseRule, Fs, is_within, lexical};

/// The tools of this module, in the order they are listed.
pub(super) const TOOLS: [&str; 3] = ["make_folder", "move_path", "delete_folder"];

/// The answer to a call while the chat is not in Full access.
pub const ONLY_IN_FULL_ACCESS: &str = "This tool works only when Full access is on, and it is off in this chat. Nothing was changed. The user can turn on Full access in the chat's header.";

/// Entries counted at most for the preview of a folder that is deleted.
const MAX_COUNTED: usize = 20_000;

/// Whether `tool` is one of the tools of this module.
pub fn is_tool(tool: &str) -> bool {
    TOOLS.contains(&tool)
}

/// The tools' definitions, added to the files server's list.
pub(super) fn defs() -> Vec<ToolDef> {
    let def = |name: &str, description: &str, schema: Value, destructive: bool| ToolDef {
        name: name.into(),
        description: description.into(),
        input_schema: schema,
        destructive: Some(destructive),
        read_only: Some(false),
    };
    vec![
        def(
            "make_folder",
            "Create a folder, and any missing folders above it. Works only in Full access.",
            json!({"type":"object","properties":{
                "path": path_schema("Folder to create")
            },"required":["path"],"additionalProperties":false}),
            false,
        ),
        def(
            "move_path",
            "Move or rename a file or folder. If `to` is an existing folder, the item goes inside it under its own name; an existing file or folder is never replaced. Works only in Full access.",
            json!({"type":"object","properties":{
                "from": path_schema("File or folder to move"),
                "to": path_schema("New path, or an existing folder to move it into")
            },"required":["from","to"],"additionalProperties":false}),
            false,
        ),
        def(
            "delete_folder",
            "Delete a folder and everything in it. Works only in Full access.",
            json!({"type":"object","properties":{
                "path": path_schema("Folder to delete")
            },"required":["path"],"additionalProperties":false}),
            true,
        ),
    ]
}

/// Whether `path` is a filesystem or drive root (`/`, `C:\`).
fn is_root(path: &Path) -> bool {
    path.parent().is_none()
}

/// The text for a root that would be moved or deleted.
fn root_refusal(shown: &str) -> String {
    format!(
        "{shown} is the top of a drive, so these tools do not move or delete it. Nothing was changed."
    )
}

/// What a deleted folder holds: files, folders and bytes, counted up to [`MAX_COUNTED`]
/// entries. Links are counted as files and not followed.
struct Contents {
    files: usize,
    folders: usize,
    bytes: u64,
    more: bool,
}

fn count(folder: &Path) -> Contents {
    let mut c = Contents {
        files: 0,
        folders: 0,
        bytes: 0,
        more: false,
    };
    let mut stack = vec![folder.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.filter_map(Result::ok) {
            if c.files + c.folders >= MAX_COUNTED {
                c.more = true;
                return c;
            }
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                c.folders += 1;
                stack.push(e.path());
            } else {
                c.files += 1;
                c.bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    c
}

fn plural(n: usize, one: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {one}s")
    }
}

impl FilesServer {
    /// The app folder (or skill folder) that is `target` or lies inside it, if any: such a
    /// folder is never moved or deleted. The comparison ignores letter case, so it refuses
    /// at least as much as the file system would match.
    fn protected_inside(&self, target: &str, chat: &ChatContext) -> Option<String> {
        let t = lexical(target, None, self.flavor).ok()?;
        for f in &chat.forbidden_folders {
            let mut forms = vec![f.clone()];
            if let Ok(Some(c)) = RealFs.canonicalize(f) {
                forms.push(c);
            }
            for form in forms {
                if let Ok(fl) = lexical(&form, None, self.flavor)
                    && is_within(&fl, &t, CaseRule::Insensitive)
                {
                    return Some(format!(
                        "{} holds the app's own folders, which are never moved or deleted. Nothing was changed.",
                        policy::sanitize::visible(target)
                    ));
                }
            }
        }
        skill_guard::check(
            target,
            &chat.skill_roots,
            self.flavor,
            Reach::InsideOrAround,
        )
        .err()
    }

    /// Describes a call to one of this module's tools.
    pub(super) async fn describe_organize(
        &self,
        tool: &str,
        args: &Map<String, Value>,
        chat: &ChatContext,
    ) -> Described {
        if !chat.full_access {
            return Described::Invalid(ONLY_IN_FULL_ACCESS.into());
        }
        match tool {
            "make_folder" => self.describe_make_folder(tool, args, chat),
            "move_path" => self.describe_move(tool, args, chat),
            "delete_folder" => self.describe_delete_folder(tool, args, chat),
            other => Described::Invalid(format!("the files server has no tool named {other}")),
        }
    }

    /// An action that creates, moves or deletes something at `c`.
    fn organize_action(
        tool: &str,
        c: &CheckedPath,
        root: &str,
        summary: String,
        preview: String,
        destroys: bool,
        extra: Value,
    ) -> Described {
        Described::Action(Action {
            server: "files".into(),
            tool: tool.into(),
            summary,
            preview,
            scope: Scope::FileWrite {
                folder: root.to_string(),
            },
            // Only offered in Full access, where nothing asks. Described as what it is, so
            // the audit log and any other mode see a delete or a move as one.
            risk: if destroys {
                Risk::AlwaysAsk
            } else {
                Risk::Normal
            },
            read_only: false,
            outbound: false,
            keep_asking: destroys.then_some(policy::KeepAsking::Delete),
            network: false,
            hard_limit: None,
            payload: Self::payload(c, extra),
        })
    }

    fn describe_make_folder(
        &self,
        tool: &str,
        args: &Map<String, Value>,
        chat: &ChatContext,
    ) -> Described {
        let Some(path) = s(args, "path") else {
            return Described::Invalid("path is required".into());
        };
        let (c, root, _) = match self.check(chat, &path) {
            Ok(v) => v,
            Err(why) => return Self::hard_limit(tool, &path, why),
        };
        if let Err(why) =
            skill_guard::check(&c.canonical, &chat.skill_roots, self.flavor, Reach::Inside)
        {
            return Self::hard_limit(tool, &path, why);
        }
        let shown = policy::sanitize::visible(&c.canonical);
        let p = Path::new(&c.canonical);
        if p.is_dir() {
            return Described::Invalid(format!("{shown} already exists. Nothing was changed."));
        }
        if p.symlink_metadata().is_ok() {
            return Described::Invalid(format!(
                "{shown} already exists and is not a folder. Pick another name."
            ));
        }
        Self::organize_action(
            tool,
            &c,
            &root,
            format!("Create folder {shown}"),
            format!("Create the folder {shown}, and any missing folders above it."),
            false,
            Value::Null,
        )
    }

    fn describe_move(
        &self,
        tool: &str,
        args: &Map<String, Value>,
        chat: &ChatContext,
    ) -> Described {
        let (Some(from), Some(to)) = (s(args, "from"), s(args, "to")) else {
            return Described::Invalid("from and to are both required".into());
        };
        let (src, root, _) = match self.check(chat, &from) {
            Ok(v) => v,
            Err(why) => return Self::hard_limit(tool, &from, why),
        };
        let shown_from = policy::sanitize::visible(&src.canonical);
        let src_path = PathBuf::from(&src.canonical);
        // The path is resolved through links, so moving one would move what it points to.
        if src.was_symlink {
            return Described::Invalid(format!(
                "{} is a link. move_path moves real files and folders only, and would move what the link points to. Nothing was moved; a command (run_command) can move the link itself.",
                policy::sanitize::visible(&from)
            ));
        }
        if src_path.symlink_metadata().is_err() {
            return Described::Invalid(format!(
                "{shown_from} does not exist, so there is nothing to move."
            ));
        }
        if is_root(&src_path) {
            return Described::Invalid(root_refusal(&shown_from));
        }
        if let Some(why) = self.protected_inside(&src.canonical, chat) {
            return Self::hard_limit(tool, &from, why);
        }
        let (mut dst, _, _) = match self.check(chat, &to) {
            Ok(v) => v,
            Err(why) => return Self::hard_limit(tool, &to, why),
        };
        // Into an existing folder, under the item's own name.
        if Path::new(&dst.canonical).is_dir() {
            let Some(name) = src_path.file_name() else {
                return Described::Invalid(root_refusal(&shown_from));
            };
            let inside = Path::new(&dst.canonical).join(name).display().to_string();
            dst = match self.check(chat, &inside) {
                Ok((c, _, _)) => c,
                Err(why) => return Self::hard_limit(tool, &inside, why),
            };
        }
        let shown_to = policy::sanitize::visible(&dst.canonical);
        if let Err(why) = skill_guard::check(
            &dst.canonical,
            &chat.skill_roots,
            self.flavor,
            Reach::Inside,
        ) {
            return Self::hard_limit(tool, &to, why);
        }
        let dst_path = Path::new(&dst.canonical);
        if dst_path == src_path {
            return Described::Invalid(format!(
                "{shown_from} is already there, so nothing was moved."
            ));
        }
        if dst_path.symlink_metadata().is_ok() {
            return Described::Invalid(format!(
                "{shown_to} already exists, and move_path never replaces anything. Pick another name, or move or delete what is there first."
            ));
        }
        if dst_path.starts_with(&src_path) {
            return Described::Invalid(format!(
                "{shown_to} is inside {shown_from}, and a folder cannot be moved into itself."
            ));
        }
        let what = if src_path.is_dir() { "folder" } else { "file" };
        Self::organize_action(
            tool,
            &src,
            &root,
            format!("Move {shown_from} to {shown_to}"),
            format!("Move the {what} {shown_from} to {shown_to}."),
            false,
            json!({
                "to": {
                    "canonical": dst.canonical,
                    "root": dst.root,
                    "exists": dst.exists,
                    "wasSymlink": dst.was_symlink,
                }
            }),
        )
    }

    fn describe_delete_folder(
        &self,
        tool: &str,
        args: &Map<String, Value>,
        chat: &ChatContext,
    ) -> Described {
        let Some(path) = s(args, "path") else {
            return Described::Invalid("path is required".into());
        };
        let (c, root, _) = match self.check(chat, &path) {
            Ok(v) => v,
            Err(why) => return Self::hard_limit(tool, &path, why),
        };
        let shown = policy::sanitize::visible(&c.canonical);
        let p = PathBuf::from(&c.canonical);
        if c.was_symlink {
            return Described::Invalid(format!(
                "{} is a link, not a folder. delete_folder removes real folders only, so nothing was deleted; a command (run_command) can remove the link itself.",
                policy::sanitize::visible(&path)
            ));
        }
        let Ok(meta) = p.symlink_metadata() else {
            return Described::Invalid(format!(
                "{shown} does not exist, so there is nothing to delete."
            ));
        };
        if !meta.is_dir() {
            return Described::Invalid(format!(
                "{shown} is not a folder. delete_file deletes a file."
            ));
        }
        if is_root(&p) {
            return Described::Invalid(root_refusal(&shown));
        }
        if let Some(why) = self.protected_inside(&c.canonical, chat) {
            return Self::hard_limit(tool, &path, why);
        }
        let held = count(&p);
        let what = if held.files + held.folders == 0 {
            "It is empty.".to_string()
        } else {
            format!(
                "It holds {}{}, {} ({} bytes).",
                if held.more { "more than " } else { "" },
                plural(held.files, "file"),
                plural(held.folders, "folder"),
                held.bytes
            )
        };
        Self::organize_action(
            tool,
            &c,
            &root,
            format!("Delete folder {shown}"),
            format!("Delete the folder {shown} and everything in it. {what}"),
            true,
            Value::Null,
        )
    }

    /// Runs a call to one of this module's tools that the gate let through.
    pub(super) async fn call_organize(
        &self,
        tool: &str,
        action: &Action,
        ctx: &CallContext,
    ) -> ToolOutput {
        // Read again: the user may have switched Full access off since the call was
        // described.
        if !ctx.chat.full_access {
            return ToolOutput::error(ONLY_IN_FULL_ACCESS);
        }
        let path = match self.recheck(&ctx.chat, action) {
            Ok(p) => p,
            Err(why) => return ToolOutput::error(why),
        };
        let shown = policy::sanitize::visible(&path.display().to_string());
        match tool {
            "make_folder" => {
                if let Err(why) = skill_guard::check(
                    &path.display().to_string(),
                    &ctx.chat.skill_roots,
                    self.flavor,
                    Reach::Inside,
                ) {
                    return ToolOutput::error(why);
                }
                match std::fs::create_dir_all(&path) {
                    Ok(()) => ToolOutput::text(format!("Created the folder {shown}.")),
                    Err(e) => {
                        ToolOutput::error(format!("could not create {shown}: {}", io_reason(&e)))
                    }
                }
            }
            "move_path" => self.run_move(&path, action, &ctx.chat),
            "delete_folder" => {
                if let Some(why) = self.protected_inside(&path.display().to_string(), &ctx.chat) {
                    return ToolOutput::error(why);
                }
                if is_root(&path) {
                    return ToolOutput::error(root_refusal(&shown));
                }
                if !path.is_dir() || path.symlink_metadata().is_ok_and(|m| !m.is_dir()) {
                    return ToolOutput::error(format!(
                        "{shown} is no longer a folder, so nothing was deleted."
                    ));
                }
                let target = path.clone();
                match tokio::task::spawn_blocking(move || std::fs::remove_dir_all(&target)).await {
                    Ok(Ok(())) => ToolOutput::text(format!(
                        "Deleted the folder {shown} and everything in it."
                    )),
                    Ok(Err(e)) => ToolOutput::error(format!(
                        "could not delete all of {shown}: {}. Some of it may be gone already; list_dir shows what is left.",
                        io_reason(&e)
                    )),
                    Err(e) => ToolOutput::error(e.to_string()),
                }
            }
            other => ToolOutput::error(format!("the files server has no tool named {other}")),
        }
    }

    fn run_move(&self, from: &Path, action: &Action, chat: &ChatContext) -> ToolOutput {
        let shown_from = policy::sanitize::visible(&from.display().to_string());
        let Some(to) = action.payload.get("to").and_then(|t| {
            Some(CheckedPath {
                canonical: t.get("canonical")?.as_str()?.to_string(),
                root: t.get("root")?.as_u64()? as usize,
                exists: t.get("exists")?.as_bool()?,
                was_symlink: t.get("wasSymlink")?.as_bool()?,
            })
        }) else {
            return ToolOutput::error("internal error: the approved move has no destination");
        };
        let sb = self.sandbox(chat);
        if let Err(e) = sb.recheck(&to, &RealFs) {
            return ToolOutput::error(self.refusal(&e, &sb));
        }
        let to = PathBuf::from(&to.canonical);
        let shown_to = policy::sanitize::visible(&to.display().to_string());
        if let Some(why) = self.protected_inside(&from.display().to_string(), chat) {
            return ToolOutput::error(why);
        }
        if let Err(why) = skill_guard::check(
            &to.display().to_string(),
            &chat.skill_roots,
            self.flavor,
            Reach::Inside,
        ) {
            return ToolOutput::error(why);
        }
        if is_root(from) {
            return ToolOutput::error(root_refusal(&shown_from));
        }
        if to.symlink_metadata().is_ok() {
            return ToolOutput::error(format!(
                "{shown_to} exists now, and move_path never replaces anything. Nothing was moved."
            ));
        }
        if let Some(parent) = to.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            return ToolOutput::error(format!(
                "could not create the folder for {shown_to}: {}",
                io_reason(&e)
            ));
        }
        match std::fs::rename(from, &to) {
            Ok(()) => {
                if let Ok(mut seen) = self.seen.lock() {
                    seen.files
                        .retain(|(_, p), _| !Path::new(p).starts_with(from));
                }
                ToolOutput::text(format!("Moved {shown_from} to {shown_to}."))
            }
            Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices && from.is_file() => {
                // Another drive: a file is copied, checked and then removed.
                match std::fs::copy(from, &to) {
                    Ok(n) if Some(n) == std::fs::metadata(from).ok().map(|m| m.len()) => {
                        match std::fs::remove_file(from) {
                            Ok(()) => ToolOutput::text(format!(
                                "Moved {shown_from} to {shown_to} (copied to the other drive, then removed)."
                            )),
                            Err(e) => ToolOutput::error(format!(
                                "copied {shown_from} to {shown_to}, but could not remove the original: {}",
                                io_reason(&e)
                            )),
                        }
                    }
                    Ok(_) => {
                        let _ = std::fs::remove_file(&to);
                        ToolOutput::error(format!(
                            "the copy of {shown_from} on the other drive came out a different size, so it was removed and the original kept."
                        ))
                    }
                    Err(e) => ToolOutput::error(format!(
                        "could not copy {shown_from} to the other drive: {}",
                        io_reason(&e)
                    )),
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => ToolOutput::error(format!(
                "{shown_to} is on another drive, and move_path moves a folder only within a drive. Nothing was moved. A command (run_command) can copy it across, and delete_folder can remove the original afterwards."
            )),
            Err(e) => ToolOutput::error(format!(
                "could not move {shown_from} to {shown_to}: {}",
                io_reason(&e)
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_util::sync::CancellationToken;

    struct Setup {
        _dir: tempfile::TempDir,
        root: PathBuf,
        app: PathBuf,
        chat: ChatContext,
    }

    /// A chat in Full access whose only folder is a temporary one, with the app's own
    /// folder inside it (as the app's data folder is inside the home folder).
    fn setup() -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let app = root.join("app-data");
        std::fs::create_dir_all(app.join("logs")).unwrap();
        std::fs::write(app.join("app.db"), "database").unwrap();
        let chat = ChatContext {
            chat_id: 1,
            approved_folders: vec![root.display().to_string()],
            working_folder: Some(root.display().to_string()),
            forbidden_folders: vec![app.display().to_string()],
            skill_folders: vec![],
            skill_roots: vec![root.join("skills").display().to_string()],
            result_cap_tokens: 2000,
            full_access: true,
        };
        Setup {
            _dir: dir,
            root,
            app,
            chat,
        }
    }

    async fn run(
        fs: &FilesServer,
        chat: &ChatContext,
        tool: &str,
        a: Value,
    ) -> Result<(Action, ToolOutput), String> {
        let a = a.as_object().cloned().unwrap();
        match fs.describe(tool, &a, chat).await {
            Described::Invalid(why) => Err(why),
            Described::Action(action) => {
                if let Some(h) = &action.hard_limit {
                    return Err(h.clone());
                }
                let ctx = CallContext {
                    chat: chat.clone(),
                    cancel: CancellationToken::new(),
                    progress: None,
                };
                let out = fs.call(tool, &a, &action, ctx).await;
                Ok((action, out))
            }
        }
    }

    fn ok(r: Result<(Action, ToolOutput), String>) -> (Action, String) {
        let (action, out) = r.expect("the call was refused");
        assert!(!out.is_error, "{}", out.joined_text());
        (action, out.joined_text())
    }

    /// Outside Full access the tools refuse before anything is looked at, and again when
    /// the mode is switched off between the description and the run.
    #[tokio::test]
    async fn the_tools_work_only_in_full_access() {
        let s = setup();
        let fs = FilesServer::default();
        std::fs::create_dir_all(s.root.join("old")).unwrap();
        let off = ChatContext {
            full_access: false,
            ..s.chat.clone()
        };
        for (tool, a) in [
            ("make_folder", json!({"path": "new"})),
            ("move_path", json!({"from": "old", "to": "renamed"})),
            ("delete_folder", json!({"path": "old"})),
        ] {
            let why = run(&fs, &off, tool, a.clone()).await.unwrap_err();
            assert_eq!(why, ONLY_IN_FULL_ACCESS, "{tool}");

            let a = a.as_object().cloned().unwrap();
            let Described::Action(action) = fs.describe(tool, &a, &s.chat).await else {
                panic!("{tool} was not described in Full access");
            };
            let ctx = CallContext {
                chat: off.clone(),
                cancel: CancellationToken::new(),
                progress: None,
            };
            let out = fs.call(tool, &a, &action, ctx).await;
            assert!(
                out.is_error && out.joined_text() == ONLY_IN_FULL_ACCESS,
                "{tool}"
            );
        }
        assert!(!s.root.join("new").exists());
        assert!(s.root.join("old").is_dir());
        assert!(!s.root.join("renamed").exists());
        // The other file tools are not affected by the mode.
        assert!(is_tool("delete_folder") && !is_tool("delete_file"));
        let names: Vec<String> = fs.tools().into_iter().map(|t| t.name).collect();
        for t in TOOLS {
            assert!(names.contains(&t.to_string()), "{t} is listed");
        }
    }

    /// Create, move, rename and delete: the organising a model does in Full access.
    #[tokio::test]
    async fn folders_are_created_moved_and_deleted() {
        let s = setup();
        let fs = FilesServer::default();

        let (action, text) = ok(run(&fs, &s.chat, "make_folder", json!({"path": "a/b/c"})).await);
        assert!(s.root.join("a/b/c").is_dir(), "{text}");
        assert_eq!(action.keep_asking, None);
        assert!(!action.read_only);
        let why = run(&fs, &s.chat, "make_folder", json!({"path": "a/b"}))
            .await
            .unwrap_err();
        assert!(why.contains("already exists"), "{why}");

        std::fs::write(s.root.join("a/report.txt"), "text").unwrap();
        std::fs::create_dir_all(s.root.join("docs")).unwrap();
        // Into an existing folder, under its own name.
        ok(run(
            &fs,
            &s.chat,
            "move_path",
            json!({"from": "a/report.txt", "to": "docs"}),
        )
        .await);
        assert_eq!(
            std::fs::read_to_string(s.root.join("docs/report.txt")).unwrap(),
            "text"
        );
        // A rename, into a folder that does not exist yet.
        ok(run(
            &fs,
            &s.chat,
            "move_path",
            json!({"from": "docs/report.txt", "to": "archive/2026/report-old.txt"}),
        )
        .await);
        assert!(s.root.join("archive/2026/report-old.txt").is_file());
        // A folder.
        ok(run(&fs, &s.chat, "move_path", json!({"from": "a", "to": "z"})).await);
        assert!(s.root.join("z/b/c").is_dir() && !s.root.join("a").exists());

        // Nothing is replaced, and a folder does not go into itself.
        std::fs::write(s.root.join("one.txt"), "1").unwrap();
        std::fs::write(s.root.join("two.txt"), "2").unwrap();
        let why = run(
            &fs,
            &s.chat,
            "move_path",
            json!({"from": "one.txt", "to": "two.txt"}),
        )
        .await
        .unwrap_err();
        assert!(why.contains("never replaces"), "{why}");
        assert_eq!(
            std::fs::read_to_string(s.root.join("two.txt")).unwrap(),
            "2"
        );
        let why = run(
            &fs,
            &s.chat,
            "move_path",
            json!({"from": "z", "to": "z/b/in"}),
        )
        .await
        .unwrap_err();
        assert!(why.contains("into itself"), "{why}");
        let why = run(
            &fs,
            &s.chat,
            "move_path",
            json!({"from": "gone", "to": "x"}),
        )
        .await
        .unwrap_err();
        assert!(why.contains("does not exist"), "{why}");
        // A link is not followed: moving it would move what it points to.
        #[cfg(unix)]
        {
            std::fs::create_dir_all(s.root.join("real")).unwrap();
            std::os::unix::fs::symlink(s.root.join("real"), s.root.join("link")).unwrap();
            let why = run(
                &fs,
                &s.chat,
                "move_path",
                json!({"from": "link", "to": "renamed"}),
            )
            .await
            .unwrap_err();
            assert!(why.contains("is a link"), "{why}");
            assert!(s.root.join("real").is_dir() && !s.root.join("renamed").exists());
            let why = run(&fs, &s.chat, "delete_folder", json!({"path": "link"}))
                .await
                .unwrap_err();
            assert!(
                why.contains("is a link") && !why.contains("delete_file"),
                "{why}"
            );
        }

        // A delete says what goes with it, and is described as a delete.
        std::fs::write(s.root.join("z/b/c/f.txt"), "12345").unwrap();
        let (action, text) = ok(run(&fs, &s.chat, "delete_folder", json!({"path": "z"})).await);
        assert!(!s.root.join("z").exists(), "{text}");
        assert!(
            action.preview.contains("1 file, 2 folders (5 bytes)"),
            "{}",
            action.preview
        );
        assert_eq!(action.keep_asking, Some(policy::KeepAsking::Delete));
        assert_eq!(action.risk, Risk::AlwaysAsk);
        let why = run(&fs, &s.chat, "delete_folder", json!({"path": "one.txt"}))
            .await
            .unwrap_err();
        assert!(why.contains("not a folder"), "{why}");
        let why = run(&fs, &s.chat, "delete_folder", json!({"path": "gone"}))
            .await
            .unwrap_err();
        assert!(why.contains("does not exist"), "{why}");
    }

    /// The app's own folders: never reached, and never moved or deleted with a folder
    /// around them. The same for the folders skills are loaded from.
    #[tokio::test]
    async fn the_app_folders_are_never_moved_or_deleted() {
        let s = setup();
        let fs = FilesServer::default();
        let app = s.app.display().to_string();
        let inside = s.app.join("logs").display().to_string();
        let root = s.root.display().to_string();
        std::fs::create_dir_all(s.root.join("skills/x")).unwrap();
        std::fs::create_dir_all(s.root.join("box")).unwrap();
        for (tool, a) in [
            (
                "make_folder",
                json!({"path": s.app.join("new").display().to_string()}),
            ),
            ("delete_folder", json!({"path": app})),
            ("delete_folder", json!({"path": inside})),
            // The folder that holds the app's folder.
            ("delete_folder", json!({"path": root})),
            ("move_path", json!({"from": app, "to": "elsewhere"})),
            ("move_path", json!({"from": root, "to": "elsewhere"})),
            ("move_path", json!({"from": "box", "to": app})),
            ("move_path", json!({"from": "box", "to": inside})),
            ("delete_folder", json!({"path": "skills"})),
            ("delete_folder", json!({"path": "skills/x"})),
            ("move_path", json!({"from": "skills", "to": "s2"})),
            ("move_path", json!({"from": "box", "to": "skills/x"})),
            ("make_folder", json!({"path": "skills/y"})),
        ] {
            let r = run(&fs, &s.chat, tool, a.clone()).await;
            assert!(r.is_err(), "{tool} {a} was let through");
        }
        // The app's text upper-cased on a file system that ignores case is still refused.
        let upper = s.app.display().to_string().to_uppercase();
        if Path::new(&upper).exists() {
            assert!(
                run(&fs, &s.chat, "delete_folder", json!({"path": upper}))
                    .await
                    .is_err()
            );
        }
        assert!(s.app.join("app.db").is_file() && s.app.join("logs").is_dir());
        assert!(s.root.join("skills/x").is_dir() && s.root.join("box").is_dir());
        // A drive's top is never moved or deleted here.
        let top = s.root.ancestors().last().unwrap().display().to_string();
        let wide = ChatContext {
            approved_folders: vec![top.clone()],
            forbidden_folders: vec![],
            skill_roots: vec![],
            ..s.chat.clone()
        };
        let why = run(&fs, &wide, "delete_folder", json!({"path": top}))
            .await
            .unwrap_err();
        assert!(why.contains("top of a drive"), "{why}");
    }

    /// A listing or a search of a folder that holds the app's folder names it, and goes no
    /// further into it.
    #[tokio::test]
    async fn listings_and_searches_stay_out_of_the_app_folders() {
        let s = setup();
        let fs = FilesServer::default();
        std::fs::write(s.app.join("logs/run.log"), "secret-token here").unwrap();
        std::fs::write(s.root.join("notes.txt"), "secret-token too").unwrap();
        let (_, text) = ok(run(&fs, &s.chat, "list_dir", json!({"path": ".", "depth": 3})).await);
        assert!(
            text.contains("app-data/  (the app's own folder: not reachable)"),
            "{text}"
        );
        assert!(
            !text.contains("app.db") && !text.contains("run.log"),
            "{text}"
        );
        assert!(text.contains("notes.txt"), "{text}");
        let (_, text) = ok(run(
            &fs,
            &s.chat,
            "search_files",
            json!({"path": ".", "query": "secret-token"}),
        )
        .await);
        assert!(text.contains("notes.txt"), "{text}");
        assert!(!text.contains("run.log"), "{text}");
        // In every mode, not only in Full access.
        let off = ChatContext {
            full_access: false,
            ..s.chat.clone()
        };
        let (_, text) = ok(run(
            &fs,
            &off,
            "search_files",
            json!({"path": ".", "query": "secret-token"}),
        )
        .await);
        assert!(!text.contains("run.log"), "{text}");
    }

    /// A pipe (or a device) is never read: reading one blocks for ever or never ends. With
    /// the whole computer reachable in Full access, a search must still finish.
    #[cfg(unix)]
    #[tokio::test]
    async fn pipes_and_devices_are_left_alone() {
        let s = setup();
        let fs = FilesServer::default();
        let pipe = s.root.join("pipe");
        let made = std::process::Command::new("mkfifo").arg(&pipe).status();
        if !made.is_ok_and(|st| st.success()) {
            return; // no mkfifo here
        }
        std::fs::write(s.root.join("plain.txt"), "needle\n").unwrap();
        for (tool, a) in [
            ("read_file", json!({"path": "pipe"})),
            ("write_file", json!({"path": "pipe", "content": "x"})),
            (
                "edit_file",
                json!({"path": "pipe", "old_text": "a", "new_text": "b"}),
            ),
            ("delete_file", json!({"path": "pipe"})),
        ] {
            let why = run(&fs, &s.chat, tool, a).await.unwrap_err();
            assert!(why.contains("not a regular file"), "{tool}: {why}");
        }
        let search = run(
            &fs,
            &s.chat,
            "search_files",
            json!({"path": ".", "query": "needle"}),
        );
        let (_, text) = ok(
            tokio::time::timeout(std::time::Duration::from_secs(10), search)
                .await
                .expect("the search hung on a pipe"),
        );
        assert!(text.contains("plain.txt"), "{text}");
        let wide = ChatContext {
            approved_folders: vec!["/".into()],
            ..s.chat.clone()
        };
        let why = run(&fs, &wide, "read_file", json!({"path": "/dev/zero"}))
            .await
            .unwrap_err();
        assert!(why.contains("not a regular file"), "{why}");
    }
}
