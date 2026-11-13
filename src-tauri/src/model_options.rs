//! The chat's model list, "On this computer" (0.0.6 item 8): every installed model (the
//! same rows as Models → Installed) and every file used recently, each file once.
//!
//! Before 0.0.6 the list held only the files used recently (`recentLocalModels`), so a
//! finished download appeared only after it had been loaded once. Now the files used
//! recently come first, newest first, then the other installed models by name; a file that
//! is no longer there is left out. One file reached by two spellings of its path (a link,
//! `..`, or letters in another case on Windows and macOS) is one entry, keyed by its
//! canonical path. Two different files with the same name each carry the end of their
//! folder (`hint`), so the list can tell them apart.
//!
//! Only the app's own model table and its own setting are read; nothing a model or a tool
//! writes reaches this list, and it is reachable only through the main window's command.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::Serialize;

use crate::dto::ModelOptionDto;
use crate::settings::RECENT_LOCAL_MODELS;

/// How many recently used files are remembered.
pub const RECENT_LIMIT: usize = 20;

/// How a system compares paths. Tests name the rule, so they never depend on the build
/// machine's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathRule {
    /// Linux and the rest: the case of a letter matters, `/` separates.
    Unix,
    /// macOS: the default file system ignores case; `/` separates.
    Mac,
    /// Windows: case is ignored, and `\` and `/` both separate.
    Windows,
}

impl PathRule {
    /// The rule of the system this build runs on.
    pub const fn this_system() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Mac
        } else {
            Self::Unix
        }
    }

    fn is_separator(self, c: char) -> bool {
        c == '/' || (self == Self::Windows && c == '\\')
    }

    fn separator(self) -> &'static str {
        if self == Self::Windows { "\\" } else { "/" }
    }

    /// What two spellings of one file share, given each one's canonical path: the path
    /// itself, with letters compared without case on Windows and macOS (and either slash on
    /// Windows). A case-sensitive volume on a Mac is not told apart (two files whose names
    /// differ only in case would show as one there).
    pub fn key(self, canonical: &str) -> String {
        match self {
            Self::Unix => canonical.to_owned(),
            Self::Mac => fold_case(canonical),
            Self::Windows => fold_case(&canonical.replace('/', "\\")),
        }
    }

    /// The file name: the part after the last separator.
    fn file_name(self, path: &str) -> &str {
        path.rsplit(|c| self.is_separator(c))
            .next()
            .filter(|n| !n.is_empty())
            .unwrap_or(path)
    }

    /// The names of the folders the file is in, outermost first.
    fn folders(self, path: &str) -> Vec<&str> {
        let mut parts: Vec<&str> = path
            .split(|c| self.is_separator(c))
            .filter(|p| !p.is_empty())
            .collect();
        parts.pop();
        parts
    }
}

/// Letters in one case, one for one (as Windows' own table does: "ß" stays "ß"), so a
/// path never changes length or folds into another letter's spelling.
fn fold_case(text: &str) -> String {
    text.chars()
        .map(|c| {
            let mut upper = c.to_uppercase();
            match (upper.next(), upper.next()) {
                (Some(u), None) => u,
                _ => c,
            }
        })
        .collect()
}

/// One entry of the chat's model list: the option, and for a local file whose name another
/// file in the list shares, the end of its folder ("Downloads\models"), shown in small text.
#[derive(Debug, Clone, Serialize)]
pub struct ModelOptionEntry {
    #[serde(flatten)]
    pub option: ModelOptionDto,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl ModelOptionEntry {
    /// An entry without a folder hint (an endpoint's model).
    pub fn plain(option: ModelOptionDto) -> Self {
        Self { option, hint: None }
    }
}

/// Where an existing file really is: its canonical path (links and `..` resolved, on
/// Windows the real spelling without the `\\?\` prefix), or its path as given when that
/// cannot be worked out. `None` when there is no file there.
pub fn locate(path: &str) -> Option<String> {
    let p = Path::new(path);
    if !p.is_file() {
        return None;
    }
    Some(canonical(path).unwrap_or_else(|| path.to_owned()))
}

/// The canonical spelling of a path, if the file is there.
fn canonical(path: &str) -> Option<String> {
    dunce::canonicalize(path)
        .ok()
        .map(|c| c.to_string_lossy().into_owned())
}

/// The local entries of the list. `recent` (newest first) and `installed` (the model
/// table's paths) as stored; `locate` gives an existing file's canonical path and `None`
/// for a file that is gone. Files used recently come first, then the other installed
/// models by name (ignoring case), then by path. One file is one entry; an installed one
/// keeps the spelling of a row of Models → Installed, so picking it here or there loads
/// the same path: of two rows for one file, the one spelled as its real path, else the
/// first by path. Same-looking names (ignoring case) carry folder hints.
pub fn local_entries(
    recent: &[String],
    installed: &[String],
    rule: PathRule,
    locate: impl Fn(&str) -> Option<String>,
) -> Vec<ModelOptionEntry> {
    // One row per file: (is it spelled as the real path, its path), the better one kept.
    let mut rows: HashMap<String, (bool, String)> = HashMap::new();
    for p in installed {
        let Some(c) = locate(p) else { continue };
        let row = (*p == c, p.clone());
        let better = |old: &(bool, String)| row.0 > old.0 || (row.0 == old.0 && row.1 < old.1);
        match rows.get(&rule.key(&c)) {
            Some(old) if !better(old) => {}
            _ => {
                rows.insert(rule.key(&c), row);
            }
        }
    }
    let mut installed: Vec<(String, String)> =
        rows.into_iter().map(|(key, (_, p))| (key, p)).collect();
    installed.sort_by_cached_key(|(_, p)| (rule.file_name(p).to_lowercase(), p.clone()));
    let row_path: HashMap<&str, &str> = installed
        .iter()
        .map(|(key, path)| (key.as_str(), path.as_str()))
        .collect();

    let mut seen: HashSet<String> = HashSet::new();
    let mut paths: Vec<String> = Vec::new();
    for path in recent {
        let Some(c) = locate(path) else { continue };
        let key = rule.key(&c);
        if seen.contains(&key) {
            continue;
        }
        paths.push(
            row_path
                .get(key.as_str())
                .map_or_else(|| path.clone(), |p| (*p).to_owned()),
        );
        seen.insert(key);
    }
    for (key, path) in &installed {
        if seen.insert(key.clone()) {
            paths.push(path.clone());
        }
    }

    let hints = folder_hints(&paths, rule);
    paths
        .into_iter()
        .zip(hints)
        .map(|(path, hint)| ModelOptionEntry {
            option: ModelOptionDto {
                label: rule.file_name(&path).to_owned(),
                reference: format!("local:{path}"),
                source: "local".into(),
                context_length: None,
            },
            hint,
        })
        .collect()
}

/// For each path, the end of its folder when another path has a file of the same name
/// (ignoring case): the last two folders, or more until every one of those names differs,
/// at most the whole folder.
fn folder_hints(paths: &[String], rule: PathRule) -> Vec<Option<String>> {
    let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, p) in paths.iter().enumerate() {
        by_name
            .entry(rule.file_name(p).to_lowercase())
            .or_default()
            .push(i);
    }
    let mut hints = vec![None; paths.len()];
    for group in by_name.values().filter(|g| g.len() > 1) {
        let folders: Vec<Vec<&str>> = group.iter().map(|&i| rule.folders(&paths[i])).collect();
        let deepest = folders.iter().map(Vec::len).max().unwrap_or(0);
        let mut n = 2.min(deepest);
        loop {
            let tails: Vec<String> = folders
                .iter()
                .map(|f| {
                    let tail = f[f.len().saturating_sub(n)..].join(rule.separator());
                    if tail.is_empty() {
                        rule.separator().to_owned()
                    } else {
                        tail
                    }
                })
                .collect();
            let keys: HashSet<String> = tails.iter().map(|t| rule.key(t)).collect();
            if keys.len() == tails.len() || n >= deepest {
                for (&i, tail) in group.iter().zip(tails) {
                    hints[i] = Some(tail);
                }
                break;
            }
            n += 1;
        }
    }
    hints
}

/// The recent list after `path` was used: its canonical spelling first, every other
/// spelling of the same file dropped, at most [`RECENT_LIMIT`]. `canonical` gives a path's
/// canonical spelling when the file is there; a file that is gone is compared as written
/// and kept (a drive that is not plugged in comes back).
pub fn remembered(
    mut recent: Vec<String>,
    path: &str,
    rule: PathRule,
    canonical: impl Fn(&str) -> Option<String>,
) -> Vec<String> {
    let stored = canonical(path).unwrap_or_else(|| path.to_owned());
    let key = rule.key(&stored);
    recent.retain(|p| rule.key(&canonical(p).unwrap_or_else(|| p.clone())) != key);
    recent.insert(0, stored);
    recent.truncate(RECENT_LIMIT);
    recent
}

/// The stored recent list; one that cannot be read counts as empty, so it never hides the
/// installed models.
async fn read_recent(store: &store::Store) -> Vec<String> {
    match store.get_setting::<Vec<String>>(RECENT_LOCAL_MODELS).await {
        Ok(v) => v.unwrap_or_default(),
        Err(e) => {
            tracing::warn!("could not read recent models: {e}");
            Vec::new()
        }
    }
}

/// "On this computer", read from the store and this computer's files.
pub async fn local_options(
    store: &store::Store,
) -> Result<Vec<ModelOptionEntry>, store::StoreError> {
    let recent = read_recent(store).await;
    let installed: Vec<String> = store
        .list_models()
        .await?
        .into_iter()
        .map(|m| m.path)
        .collect();
    Ok(local_entries(
        &recent,
        &installed,
        PathRule::this_system(),
        locate,
    ))
}

/// Remembers a model file as the one used most recently (see [`remembered`]).
pub async fn remember(store: &store::Store, path: &str) {
    let recent = remembered(
        read_recent(store).await,
        path,
        PathRule::this_system(),
        canonical,
    );
    if let Err(e) = store.set_setting(RECENT_LOCAL_MODELS, &recent).await {
        tracing::warn!("could not save recent models: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    /// Every path exists and is already canonical (a file system that keeps the spelling it
    /// is given, as macOS may).
    fn all_there(p: &str) -> Option<String> {
        Some(p.to_owned())
    }

    fn refs(entries: &[ModelOptionEntry]) -> Vec<String> {
        entries.iter().map(|e| e.option.reference.clone()).collect()
    }

    fn shown(entries: &[ModelOptionEntry]) -> Vec<(String, Option<String>)> {
        entries
            .iter()
            .map(|e| (e.option.label.clone(), e.hint.clone()))
            .collect()
    }

    #[test]
    fn a_model_installed_but_never_loaded_is_listed() {
        let out = local_entries(&[], &strings(&["/m/fresh.gguf"]), PathRule::Unix, all_there);
        assert_eq!(refs(&out), ["local:/m/fresh.gguf"]);
        let e = &out[0];
        assert_eq!(
            (
                e.option.label.as_str(),
                e.option.source.as_str(),
                e.hint.as_deref()
            ),
            ("fresh.gguf", "local", None)
        );
        assert_eq!(e.option.context_length, None);
    }

    #[test]
    fn files_that_are_gone_are_left_out() {
        let gone = |p: &str| (!p.contains("gone")).then(|| p.to_owned());
        let out = local_entries(
            &strings(&["/m/gone-recent.gguf", "/m/kept.gguf"]),
            &strings(&["/m/gone-installed.gguf", "/m/other.gguf"]),
            PathRule::Unix,
            gone,
        );
        assert_eq!(refs(&out), ["local:/m/kept.gguf", "local:/m/other.gguf"]);
    }

    /// Recently used first (newest first, installed or not), then the rest of the installed
    /// models by name, ignoring case.
    #[test]
    fn recently_used_first_then_the_rest_by_name() {
        let out = local_entries(
            &strings(&["/m/c.gguf", "/elsewhere/opened.gguf", "/m/a.gguf"]),
            &strings(&[
                "/m/b.gguf",
                "/m/Zeta.gguf",
                "/m/a.gguf",
                "/m/alpha.gguf",
                "/m/c.gguf",
                "/m/Beta.gguf",
            ]),
            PathRule::Unix,
            all_there,
        );
        assert_eq!(
            refs(&out),
            [
                "local:/m/c.gguf",
                "local:/elsewhere/opened.gguf",
                "local:/m/a.gguf",
                "local:/m/alpha.gguf",
                "local:/m/b.gguf",
                "local:/m/Beta.gguf",
                "local:/m/Zeta.gguf",
            ]
        );
        assert!(out.iter().all(|e| e.hint.is_none()));
    }

    /// The owner's screenshot, first reading: one file under two spellings (the drive letter
    /// and folders in another case, the other slash). One entry on Windows and macOS, in the
    /// spelling Installed uses; the rule is named, so this holds on any build machine.
    #[test]
    fn two_spellings_of_one_file_give_one_entry_on_windows_and_macos() {
        let recent = strings(&[r"c:/users/me/downloads/MODELS/ornith-1.5-9b-uncensored.q8_0.GGUF"]);
        let installed =
            strings(&[r"C:\Users\Me\Downloads\models\Ornith-1.5-9B-uncensored.Q8_0.gguf"]);
        let out = local_entries(&recent, &installed, PathRule::Windows, all_there);
        assert_eq!(
            shown(&out),
            [("Ornith-1.5-9B-uncensored.Q8_0.gguf".to_owned(), None)]
        );
        assert_eq!(
            refs(&out),
            [r"local:C:\Users\Me\Downloads\models\Ornith-1.5-9B-uncensored.Q8_0.gguf"]
        );

        let out = local_entries(
            &strings(&["/Users/me/Models/Ornith.Q8_0.gguf"]),
            &strings(&["/users/ME/models/ornith.q8_0.gguf"]),
            PathRule::Mac,
            all_there,
        );
        assert_eq!(refs(&out), ["local:/users/ME/models/ornith.q8_0.gguf"]);

        // On Linux the case matters: those are two files, told apart by their folders.
        let out = local_entries(
            &strings(&["/home/me/Models/Ornith.Q8_0.gguf"]),
            &strings(&["/home/me/models/ornith.q8_0.gguf"]),
            PathRule::Unix,
            all_there,
        );
        assert_eq!(
            shown(&out),
            [
                ("Ornith.Q8_0.gguf".to_owned(), Some("me/Models".to_owned())),
                ("ornith.q8_0.gguf".to_owned(), Some("me/models".to_owned())),
            ]
        );
    }

    /// One file in two rows of Installed (a link and the file itself, or one path in two
    /// cases on Windows): one entry, the row spelled as the file's real path, whatever the
    /// link is called; without such a row, the first by path, the same every time.
    #[test]
    fn one_file_in_two_rows_is_listed_by_its_real_path() {
        let canonical = |p: &str| Some(p.replace("/m/alias.gguf", "/m/real/z.gguf"));
        let out = local_entries(
            &[],
            &strings(&["/m/alias.gguf", "/m/real/z.gguf"]),
            PathRule::Unix,
            canonical,
        );
        assert_eq!(shown(&out), [("z.gguf".to_owned(), None)]);
        assert_eq!(refs(&out), ["local:/m/real/z.gguf"]);
        // Used recently under the link's name: still the real path, first.
        let out = local_entries(
            &strings(&["/m/alias.gguf"]),
            &strings(&["/m/other.gguf", "/m/real/z.gguf", "/m/alias.gguf"]),
            PathRule::Unix,
            canonical,
        );
        assert_eq!(refs(&out), ["local:/m/real/z.gguf", "local:/m/other.gguf"]);

        let real = |_: &str| Some(r"C:\m\A.gguf".to_owned());
        let out = local_entries(
            &[],
            &strings(&[r"c:\m\a.gguf", r"C:\m\A.gguf", r"C:\M\A.GGUF"]),
            PathRule::Windows,
            real,
        );
        assert_eq!(refs(&out), [r"local:C:\m\A.gguf"]);
        for rows in [
            [r"c:\m\a.gguf", r"C:\M\A.GGUF"],
            [r"C:\M\A.GGUF", r"c:\m\a.gguf"],
        ] {
            let out = local_entries(&[], &strings(&rows), PathRule::Windows, real);
            assert_eq!(refs(&out), [r"local:C:\M\A.GGUF"]);
        }
    }

    /// Two spellings that only the file system can tell are one file (a link, `..`): the
    /// canonical path is the key.
    #[test]
    fn two_spellings_resolved_by_the_file_system_give_one_entry() {
        let canonical = |p: &str| Some(p.replace("/link/", "/models/").replace("/x/../", "/"));
        let out = local_entries(
            &strings(&["/home/me/link/a.gguf", "/home/me/x/../models/a.gguf"]),
            &strings(&["/home/me/models/a.gguf"]),
            PathRule::Unix,
            canonical,
        );
        assert_eq!(refs(&out), ["local:/home/me/models/a.gguf"]);
        // Not installed: the spelling used most recently stays.
        let out = local_entries(
            &strings(&["/home/me/link/a.gguf", "/home/me/models/a.gguf"]),
            &[],
            PathRule::Unix,
            canonical,
        );
        assert_eq!(refs(&out), ["local:/home/me/link/a.gguf"]);
    }

    /// The owner's screenshot, second reading: two copies in different folders. Two entries,
    /// each with the end of its folder.
    #[test]
    fn two_copies_give_two_entries_with_folder_hints() {
        let name = "Ornith-1.5-9B-uncensored.Q8_0.gguf";
        let out = local_entries(
            &strings(&[&format!(r"C:\Users\Me\Downloads\models\{name}")]),
            &strings(&[
                &format!(r"C:\Users\Me\AppData\Roaming\harnessUI\models\{name}"),
                r"C:\Users\Me\Downloads\models\other.gguf",
            ]),
            PathRule::Windows,
            all_there,
        );
        assert_eq!(
            shown(&out),
            [
                (name.to_owned(), Some(r"Downloads\models".to_owned())),
                (name.to_owned(), Some(r"harnessUI\models".to_owned())),
                ("other.gguf".to_owned(), None),
            ]
        );

        let out = local_entries(
            &[],
            &strings(&[
                "/home/me/Downloads/models/Q8_0.gguf",
                "/home/me/.local/share/harnessUI/models/q8_0.gguf",
            ]),
            PathRule::Unix,
            all_there,
        );
        assert_eq!(
            shown(&out),
            [
                ("q8_0.gguf".to_owned(), Some("harnessUI/models".to_owned())),
                ("Q8_0.gguf".to_owned(), Some("Downloads/models".to_owned())),
            ]
        );
    }

    /// Hints grow until they differ, and stop at the whole folder.
    #[test]
    fn folder_hints_show_enough_to_tell_the_copies_apart() {
        let out = local_entries(
            &[],
            &strings(&[
                "/a/x/models/m.gguf",
                "/b/x/models/m.gguf",
                "/m.gguf",
                "/srv/m.gguf",
            ]),
            PathRule::Unix,
            all_there,
        );
        let mut hints: Vec<String> = out.iter().filter_map(|e| e.hint.clone()).collect();
        hints.sort();
        assert_eq!(hints, ["/", "a/x/models", "b/x/models", "srv"]);

        let out = local_entries(
            &[],
            &strings(&[r"D:\models\m.gguf", r"E:\models\m.gguf", r"C:\m.gguf"]),
            PathRule::Windows,
            all_there,
        );
        let mut hints: Vec<String> = out.iter().filter_map(|e| e.hint.clone()).collect();
        hints.sort();
        assert_eq!(hints, ["C:", r"D:\models", r"E:\models"]);
    }

    /// Case folding is one letter for one letter, so it never merges different names.
    #[test]
    fn case_folding_keeps_every_letter() {
        assert_eq!(
            PathRule::Windows.key(r"c:/Straße/model.gguf"),
            r"C:\STRAßE\MODEL.GGUF"
        );
        assert_ne!(
            PathRule::Mac.key("/m/strasse.gguf"),
            PathRule::Mac.key("/m/straße.gguf")
        );
        assert_eq!(PathRule::Mac.key("/Ä/b"), PathRule::Mac.key("/ä/B"));
        assert_ne!(PathRule::Unix.key("/A/b"), PathRule::Unix.key("/a/b"));
        // A backslash is part of a name on Linux and macOS.
        assert_ne!(PathRule::Unix.key(r"/m/a\b"), PathRule::Unix.key("/m/a/b"));
        assert_eq!(PathRule::Unix.file_name(r"/m/a\b.gguf"), r"a\b.gguf");
        assert_eq!(PathRule::Windows.file_name(r"C:\m\a/b.gguf"), "b.gguf");
    }

    #[test]
    fn remembering_keeps_one_spelling_of_each_file_newest_first() {
        let recent = strings(&[r"c:\models\x.gguf", r"C:\other\y.gguf"]);
        let out = remembered(recent, r"C:/Models/X.gguf", PathRule::Windows, all_there);
        assert_eq!(out, strings(&[r"C:/Models/X.gguf", r"C:\other\y.gguf"]));

        // The canonical spelling is what is stored.
        let canonical = |p: &str| Some(p.replace("/link/", "/models/"));
        let out = remembered(
            strings(&["/m/models/a.gguf", "/m/b.gguf"]),
            "/m/link/a.gguf",
            PathRule::Unix,
            canonical,
        );
        assert_eq!(out, strings(&["/m/models/a.gguf", "/m/b.gguf"]));

        // A file that is gone stays remembered under its own spelling.
        let out = remembered(
            strings(&["/unplugged/a.gguf"]),
            "/m/b.gguf",
            PathRule::Unix,
            |p: &str| (!p.starts_with("/unplugged")).then(|| p.to_owned()),
        );
        assert_eq!(out, strings(&["/m/b.gguf", "/unplugged/a.gguf"]));

        let many: Vec<String> = (0..30).map(|i| format!("/m/{i}.gguf")).collect();
        let out = remembered(many, "/m/new.gguf", PathRule::Unix, all_there);
        assert_eq!(out.len(), RECENT_LIMIT);
        assert_eq!(out[0], "/m/new.gguf");
        assert_eq!(out[1], "/m/0.gguf");
    }

    /// The JSON the picker reads: the 0.0.4 fields as before, and `hint` only when there is one.
    #[test]
    fn entries_serialise_as_model_options_with_an_optional_hint() {
        let out = local_entries(
            &[],
            &strings(&["/a/b/m.gguf", "/c/d/m.gguf", "/c/d/n.gguf"]),
            PathRule::Unix,
            all_there,
        );
        let json = serde_json::to_value(&out).unwrap();
        assert_eq!(
            json,
            serde_json::json!([
                {"ref": "local:/a/b/m.gguf", "label": "m.gguf", "source": "local", "contextLength": null, "hint": "a/b"},
                {"ref": "local:/c/d/m.gguf", "label": "m.gguf", "source": "local", "contextLength": null, "hint": "c/d"},
                {"ref": "local:/c/d/n.gguf", "label": "n.gguf", "source": "local", "contextLength": null},
            ])
        );
    }

    // ---- With the real store and real files ----

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("model-options-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        // Its real path (on macOS the temporary folder is reached through a link).
        dunce::canonicalize(&d).unwrap()
    }

    async fn register(store: &store::Store, path: &std::path::Path) {
        store
            .upsert_model(store::ModelInput {
                path: path.to_string_lossy().into_owned(),
                repo: Some("someone/Model-GGUF".into()),
                file_name: path.file_name().unwrap().to_string_lossy().into_owned(),
                size_bytes: 4,
                sha256: None,
                gguf_summary: serde_json::json!({}),
                source: store::ModelSource::Download,
            })
            .await
            .unwrap();
    }

    /// A finished download (a registered model that was never loaded) is listed; one whose
    /// file was deleted is not; old `recentLocalModels` data (a plain list of paths, with a
    /// file that is gone) still works and comes first.
    #[tokio::test]
    async fn the_store_lists_installed_models_and_recent_files() {
        let dir = temp_dir("store");
        let downloaded = dir.join("Downloaded.Q4_K_M.gguf");
        let opened = dir.join("opened.gguf");
        let deleted = dir.join("deleted.gguf");
        for f in [&downloaded, &opened, &deleted] {
            std::fs::write(f, b"GGUF").unwrap();
        }
        let store = store::Store::open_in_memory().unwrap();
        register(&store, &downloaded).await;
        register(&store, &deleted).await;
        std::fs::remove_file(&deleted).unwrap();
        store
            .set_setting(
                RECENT_LOCAL_MODELS,
                &serde_json::json!([opened, dir.join("gone.gguf")]),
            )
            .await
            .unwrap();

        let out = local_options(&store).await.unwrap();
        assert_eq!(
            refs(&out),
            [
                format!("local:{}", opened.display()),
                format!("local:{}", downloaded.display()),
            ]
        );
        assert_eq!(out[1].option.label, "Downloaded.Q4_K_M.gguf");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Two spellings of one file through the real file system (`..`, and on Unix a link) are
    /// one entry, and remembering stores the canonical path once.
    #[tokio::test]
    async fn the_store_keys_files_by_their_canonical_path() {
        let dir = temp_dir("canon");
        std::fs::create_dir_all(dir.join("models")).unwrap();
        std::fs::create_dir_all(dir.join("other")).unwrap();
        let file = dir.join("models").join("m.gguf");
        std::fs::write(&file, b"GGUF").unwrap();
        let dotted = dir.join("other").join("..").join("models").join("m.gguf");
        let store = store::Store::open_in_memory().unwrap();
        register(&store, &file).await;

        remember(&store, &dotted.to_string_lossy()).await;
        remember(&store, &file.to_string_lossy()).await;
        let recent: Vec<String> = store
            .get_setting(RECENT_LOCAL_MODELS)
            .await
            .unwrap()
            .unwrap();
        let canonical = dunce::canonicalize(&file)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(recent, [canonical.clone()]);

        #[cfg(unix)]
        {
            let link = dir.join("link.gguf");
            std::os::unix::fs::symlink(&file, &link).unwrap();
            register(&store, &link).await;
            remember(&store, &link.to_string_lossy()).await;
            let recent: Vec<String> = store
                .get_setting(RECENT_LOCAL_MODELS)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(recent, [canonical.clone()]);
        }
        let out = local_options(&store).await.unwrap();
        assert_eq!(out.len(), 1, "{:?}", refs(&out));
        assert_eq!(out[0].hint, None);
        // The row spelled as the file's real path, not the link's.
        assert_eq!(refs(&out), [format!("local:{}", file.display())]);
        assert_eq!(out[0].option.label, "m.gguf");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A stored list that cannot be read never hides the installed models.
    #[tokio::test]
    async fn an_unreadable_recent_list_still_lists_installed_models() {
        let dir = temp_dir("bad");
        let file = dir.join("m.gguf");
        std::fs::write(&file, b"GGUF").unwrap();
        let store = store::Store::open_in_memory().unwrap();
        register(&store, &file).await;
        store
            .set_setting(RECENT_LOCAL_MODELS, &serde_json::json!({"not": "a list"}))
            .await
            .unwrap();
        let out = local_options(&store).await.unwrap();
        assert_eq!(refs(&out), [format!("local:{}", file.display())]);
        remember(&store, &file.to_string_lossy()).await;
        let recent: Vec<String> = store
            .get_setting(RECENT_LOCAL_MODELS)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(recent.len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
