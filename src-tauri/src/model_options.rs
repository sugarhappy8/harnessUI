//! The chat's model list, "On this computer" (0.0.6 item 8): every installed model (the
//! same rows as Models → Installed) and every file used recently, each file once.
//!
//! Before 0.0.6 the list held only the files used recently (`recentLocalModels`), so a
//! finished download appeared only after it had been loaded once. Now the files used
//! recently come first, newest first, then the other installed models by name; a file that
//! is no longer there is left out. One file reached by two spellings of its path (a link,
//! `..`, or letters in another case on Windows and macOS) is one entry, keyed by its
//! canonical path; a file used recently is listed under the spelling it was used with, so
//! a chat's model reference stays as it is. Two different files with the same name each
//! carry the end of their folder (`hint`), so the list can tell them apart.
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
/// `recent` marks a local file used recently (sent only when true), so a view that offers
/// "where you left off" can leave out installed models that were never used.
#[derive(Debug, Clone, Serialize)]
pub struct ModelOptionEntry {
    #[serde(flatten)]
    pub option: ModelOptionDto,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub recent: bool,
}

impl ModelOptionEntry {
    /// An entry without a folder hint (an endpoint's model).
    pub fn plain(option: ModelOptionDto) -> Self {
        Self {
            option,
            hint: None,
            recent: false,
        }
    }
}

/// The whole list in the order every view reads it: the local files used recently, then
/// the endpoints' models, then the installed models that were never used. The picker shows
/// local and endpoint models in sections of their own, so it reads as "On this computer"
/// (recent first, then by name) and "Endpoints"; the views that take the first entries as
/// the ones used last (the Welcome card, a new chat with nothing loaded, the model selects)
/// get the same first entries as before installed models were listed.
pub fn arrange(
    local: Vec<ModelOptionEntry>,
    endpoints: impl IntoIterator<Item = ModelOptionEntry>,
) -> Vec<ModelOptionEntry> {
    let (mut out, never_used): (Vec<_>, Vec<_>) = local.into_iter().partition(|e| e.recent);
    out.extend(endpoints);
    out.extend(never_used);
    out
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

/// Whether a character changes how text is displayed without being visible: control
/// characters (new lines and tabs too: a name is one line), bidirectional embeddings,
/// overrides and isolates, zero-width characters, separators, fillers, variation selectors
/// and tag characters. The same set as the webview's `visiblePath` (`FolderChip.tsx`).
fn is_invisible(c: char) -> bool {
    matches!(c as u32,
        0x00..=0x1F | 0x7F..=0x9F | 0xAD | 0x34F | 0x61C | 0x115F | 0x1160 | 0x17B4 | 0x17B5
        | 0x180B..=0x180F | 0x200B..=0x200F | 0x2028..=0x202E | 0x2060..=0x206F | 0x3164
        | 0xFE00..=0xFE0F | 0xFEFF | 0xFFA0 | 0xFFF9..=0xFFFB | 0x1BCA0..=0x1BCA3
        | 0x1D173..=0x1D17A | 0xE0000..=0xE0FFF)
}

/// Text as the list shows it: every invisible or direction-changing character as
/// `\u{XXXX}`, exactly as the webview's `visiblePath` writes it, so a file name from a
/// download or another program cannot pass for another name in any view of the list (the
/// picker, the Welcome card, the model selects).
fn visible(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if is_invisible(c) {
            out.push_str(&format!("\\u{{{:04X}}}", c as u32));
        } else {
            out.push(c);
        }
    }
    out
}

/// The local entries of the list. `recent` (newest first) and `installed` (the model
/// table's paths) as stored; `loaded` is the path of the model loaded or loading now, as
/// it was loaded; `locate` gives an existing file's canonical path and `None` for a file
/// that is gone. Files used recently come first, then the other installed models by name
/// (ignoring case), then by path. One file is one entry; the canonical path only tells
/// which spellings are one file. A file used recently keeps the spelling it was used with
/// (a link, a mapped drive), installed under another or not, so picking it here leaves the
/// chat that used it on the same reference; an installed model never used keeps the
/// spelling of a row of Models → Installed: of two rows for one file, the one spelled as
/// its real path, else the first by path. The loaded model keeps the spelling it was
/// loaded with, so picking it here in another chat is the model already loaded, not a
/// restart of the same file under another spelling (which would leave the chat that
/// loaded it with "A different model is loaded"). Same-looking names (ignoring case) carry
/// folder hints.
pub fn local_entries(
    recent: &[String],
    installed: &[String],
    loaded: Option<&str>,
    rule: PathRule,
    locate: impl Fn(&str) -> Option<String>,
) -> Vec<ModelOptionEntry> {
    // One row per file: (is it spelled as the real path, its path), the better one kept.
    let mut rows: HashMap<String, (bool, String)> = HashMap::new();
    for p in installed {
        let Some(c) = locate(p) else { continue };
        let row = (*p == c, p.clone());
        let better = |old: &(bool, String)| (row.0 && !old.0) || (row.0 == old.0 && row.1 < old.1);
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

    let loaded = loaded.and_then(|l| locate(l).map(|c| (rule.key(&c), l.to_owned())));
    let spelled = |key: &str, path: String| match &loaded {
        Some((k, l)) if k == key => l.clone(),
        _ => path,
    };

    let mut seen: HashSet<String> = HashSet::new();
    // (the path shown and picked, whether it was used recently)
    let mut paths: Vec<(String, bool)> = Vec::new();
    for path in recent {
        let Some(c) = locate(path) else { continue };
        let key = rule.key(&c);
        if seen.contains(&key) {
            continue;
        }
        paths.push((spelled(&key, path.clone()), true));
        seen.insert(key);
    }
    for (key, path) in &installed {
        if seen.insert(key.clone()) {
            paths.push((spelled(key, path.clone()), false));
        }
    }

    let shown: Vec<String> = paths.iter().map(|(p, _)| p.clone()).collect();
    let hints = folder_hints(&shown, rule);
    paths
        .into_iter()
        .zip(hints)
        .map(|((path, recent), hint)| ModelOptionEntry {
            option: ModelOptionDto {
                label: visible(rule.file_name(&path)),
                reference: format!("local:{path}"),
                source: "local".into(),
                context_length: None,
            },
            hint: hint.map(|h| visible(&h)),
            recent,
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

/// What a remembered path is compared by: its canonical spelling under `rule`, or the path
/// as written when the file is gone.
fn file_key(rule: PathRule, canonical: &impl Fn(&str) -> Option<String>, path: &str) -> String {
    rule.key(&canonical(path).unwrap_or_else(|| path.to_owned()))
}

/// The recent list after `path` was used: that spelling first, as the user opened it (a
/// link, a mapped drive: the reference of the chat that opened it), every other spelling
/// of the same file dropped, at most [`RECENT_LIMIT`]. `canonical` gives a path's
/// canonical spelling when the file is there, and serves only to tell which spellings are
/// one file; a file that is gone is compared as written and kept (a drive that is not
/// plugged in comes back). A Hugging Face cache link (`snapshots/<rev>/x.gguf`, pointing at
/// `blobs/<sha>`, a name loading refuses) is kept as the link the same way.
pub fn remembered(
    mut recent: Vec<String>,
    path: &str,
    rule: PathRule,
    canonical: impl Fn(&str) -> Option<String>,
) -> Vec<String> {
    let key = file_key(rule, &canonical, path);
    recent.retain(|p| file_key(rule, &canonical, p) != key);
    recent.insert(0, path.to_owned());
    recent.truncate(RECENT_LIMIT);
    recent
}

/// The recent list after `path` was removed from Models → Installed: every spelling of the
/// file dropped, the rest kept in order.
pub fn forgotten(
    mut recent: Vec<String>,
    path: &str,
    rule: PathRule,
    canonical: impl Fn(&str) -> Option<String>,
) -> Vec<String> {
    let key = file_key(rule, &canonical, path);
    recent.retain(|p| file_key(rule, &canonical, p) != key);
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

/// "On this computer", read from the store and this computer's files; `loaded` is the
/// path of the model loaded or loading now (see [`local_entries`]).
pub async fn local_options(
    store: &store::Store,
    loaded: Option<&str>,
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
        loaded,
        PathRule::this_system(),
        locate,
    ))
}

async fn save_recent(store: &store::Store, recent: &[String]) {
    if let Err(e) = store.set_setting(RECENT_LOCAL_MODELS, &recent).await {
        tracing::warn!("could not save recent models: {e}");
    }
}

/// Remembers a model file as the one used most recently (see [`remembered`]).
pub async fn remember(store: &store::Store, path: &str) {
    let recent = remembered(
        read_recent(store).await,
        path,
        PathRule::this_system(),
        canonical,
    );
    save_recent(store, &recent).await;
}

/// Forgets a model file removed from Models → Installed (see [`forgotten`]), so "Remove
/// from the list" takes it off the chat's model list too. The file itself is not touched.
pub async fn forget(store: &store::Store, path: &str) {
    let before = read_recent(store).await;
    let recent = forgotten(before.clone(), path, PathRule::this_system(), canonical);
    if recent != before {
        save_recent(store, &recent).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    /// The local entries with no model loaded.
    fn entries(
        recent: &[String],
        installed: &[String],
        rule: PathRule,
        locate: impl Fn(&str) -> Option<String>,
    ) -> Vec<ModelOptionEntry> {
        local_entries(recent, installed, None, rule, locate)
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
        let out = entries(&[], &strings(&["/m/fresh.gguf"]), PathRule::Unix, all_there);
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
        let out = entries(
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
        let out = entries(
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
    /// spelling used last (the chat's reference), else the one Installed uses; the rule is
    /// named, so this holds on any build machine.
    #[test]
    fn two_spellings_of_one_file_give_one_entry_on_windows_and_macos() {
        let recent = strings(&[r"c:/users/me/downloads/MODELS/ornith-1.5-9b-uncensored.q8_0.GGUF"]);
        let installed =
            strings(&[r"C:\Users\Me\Downloads\models\Ornith-1.5-9B-uncensored.Q8_0.gguf"]);
        let out = entries(&recent, &installed, PathRule::Windows, all_there);
        assert_eq!(
            shown(&out),
            [("ornith-1.5-9b-uncensored.q8_0.GGUF".to_owned(), None)]
        );
        assert_eq!(
            refs(&out),
            [r"local:c:/users/me/downloads/MODELS/ornith-1.5-9b-uncensored.q8_0.GGUF"]
        );
        assert!(out[0].recent);
        let out = entries(&[], &installed, PathRule::Windows, all_there);
        assert_eq!(
            refs(&out),
            [r"local:C:\Users\Me\Downloads\models\Ornith-1.5-9B-uncensored.Q8_0.gguf"]
        );

        let out = entries(
            &strings(&["/Users/me/Models/Ornith.Q8_0.gguf"]),
            &strings(&["/users/ME/models/ornith.q8_0.gguf"]),
            PathRule::Mac,
            all_there,
        );
        assert_eq!(refs(&out), ["local:/Users/me/Models/Ornith.Q8_0.gguf"]);

        // On Linux the case matters: those are two files, told apart by their folders.
        let out = entries(
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
    /// cases on Windows), never used: one entry, the row spelled as the file's real path,
    /// whatever the link is called; without such a row, the first by path, the same every
    /// time. Used recently: the spelling it was used with.
    #[test]
    fn one_file_in_two_rows_is_listed_by_its_real_path() {
        let canonical = |p: &str| Some(p.replace("/m/alias.gguf", "/m/real/z.gguf"));
        let out = entries(
            &[],
            &strings(&["/m/alias.gguf", "/m/real/z.gguf"]),
            PathRule::Unix,
            canonical,
        );
        assert_eq!(shown(&out), [("z.gguf".to_owned(), None)]);
        assert_eq!(refs(&out), ["local:/m/real/z.gguf"]);
        // Used recently under the link's name: the link (the chat's reference), first.
        let out = entries(
            &strings(&["/m/alias.gguf"]),
            &strings(&["/m/other.gguf", "/m/real/z.gguf", "/m/alias.gguf"]),
            PathRule::Unix,
            canonical,
        );
        assert_eq!(refs(&out), ["local:/m/alias.gguf", "local:/m/other.gguf"]);

        let real = |_: &str| Some(r"C:\m\A.gguf".to_owned());
        let out = entries(
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
            let out = entries(&[], &strings(&rows), PathRule::Windows, real);
            assert_eq!(refs(&out), [r"local:C:\M\A.GGUF"]);
        }
    }

    /// Two spellings that only the file system can tell are one file (a link, `..`): the
    /// canonical path is the key; the spelling used most recently is the one listed.
    #[test]
    fn two_spellings_resolved_by_the_file_system_give_one_entry() {
        let canonical = |p: &str| Some(p.replace("/link/", "/models/").replace("/x/../", "/"));
        let out = entries(
            &strings(&["/home/me/link/a.gguf", "/home/me/x/../models/a.gguf"]),
            &strings(&["/home/me/models/a.gguf"]),
            PathRule::Unix,
            canonical,
        );
        assert_eq!(refs(&out), ["local:/home/me/link/a.gguf"]);
        // Not installed: the same.
        let out = entries(
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
        let out = entries(
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

        let out = entries(
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
        let out = entries(
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

        let out = entries(
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

        // The spelling used is what is stored; the canonical one only finds the others.
        let canonical = |p: &str| Some(p.replace("/link/", "/models/"));
        let out = remembered(
            strings(&["/m/models/a.gguf", "/m/b.gguf"]),
            "/m/link/a.gguf",
            PathRule::Unix,
            canonical,
        );
        assert_eq!(out, strings(&["/m/link/a.gguf", "/m/b.gguf"]));

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

    /// The model loaded now is listed under the spelling it was loaded with (a link, a mapped
    /// drive), whether it is installed or not, so picking it in another chat is the loaded
    /// model. Other files keep their spelling; a loaded file that is in neither list is not
    /// added; with nothing loaded the list is as before.
    #[test]
    fn the_loaded_model_keeps_the_spelling_it_was_loaded_with() {
        let canonical = |p: &str| Some(p.replace("/x/link/", "/x/real/"));
        let recent = strings(&["/x/real/M.gguf", "/m/b.gguf"]);
        let out = local_entries(
            &recent,
            &[],
            Some("/x/link/M.gguf"),
            PathRule::Unix,
            canonical,
        );
        assert_eq!(refs(&out), ["local:/x/link/M.gguf", "local:/m/b.gguf"]);
        assert!(out[0].recent && out[1].recent);
        let out = local_entries(&recent, &[], None, PathRule::Unix, canonical);
        assert_eq!(refs(&out), ["local:/x/real/M.gguf", "local:/m/b.gguf"]);

        // Installed under its real path, loaded through the link: the loaded spelling.
        let installed = strings(&["/x/real/M.gguf", "/m/c.gguf"]);
        let out = local_entries(
            &[],
            &installed,
            Some("/x/link/M.gguf"),
            PathRule::Unix,
            canonical,
        );
        assert_eq!(refs(&out), ["local:/m/c.gguf", "local:/x/link/M.gguf"]);
        assert!(!out[1].recent);

        // A Windows mapped drive and its network path, and letters in another case.
        let unc = |p: &str| Some(p.replace(r"Z:\", r"\\server\share\"));
        let out = local_entries(
            &strings(&[r"\\server\share\models\M.gguf"]),
            &[],
            Some(r"z:\MODELS\m.gguf"),
            PathRule::Windows,
            |p: &str| unc(&p.replace("z:", "Z:")),
        );
        assert_eq!(shown(&out), [("m.gguf".to_owned(), None)]);
        assert_eq!(refs(&out), [r"local:z:\MODELS\m.gguf"]);

        // A loaded file that is not listed is not added, and one that is gone changes nothing.
        let out = local_entries(
            &recent,
            &[],
            Some("/other/n.gguf"),
            PathRule::Unix,
            canonical,
        );
        assert_eq!(refs(&out), ["local:/x/real/M.gguf", "local:/m/b.gguf"]);
        let gone = |p: &str| (!p.contains("link")).then(|| p.to_owned());
        let out = local_entries(&recent, &[], Some("/x/link/M.gguf"), PathRule::Unix, gone);
        assert_eq!(refs(&out), ["local:/x/real/M.gguf", "local:/m/b.gguf"]);
    }

    /// A model opened through a link or a mapped drive keeps that spelling in the list when
    /// it is not loaded, installed under its real path or not, so picking it there leaves the
    /// chat that opened it on the same reference. The canonical path only tells that two
    /// spellings are one file.
    #[test]
    fn a_file_opened_through_a_link_keeps_that_spelling_when_not_loaded() {
        let canonical = |p: &str| Some(p.replace("/x/link/", "/x/real/"));
        let recent = remembered(
            strings(&["/x/real/M.gguf", "/m/b.gguf"]),
            "/x/link/M.gguf",
            PathRule::Unix,
            canonical,
        );
        assert_eq!(recent, strings(&["/x/link/M.gguf", "/m/b.gguf"]));
        for installed in [Vec::new(), strings(&["/m/b.gguf", "/x/real/M.gguf"])] {
            let out = entries(&recent, &installed, PathRule::Unix, canonical);
            assert_eq!(refs(&out), ["local:/x/link/M.gguf", "local:/m/b.gguf"]);
            assert_eq!(shown(&out)[0], ("M.gguf".to_owned(), None));
        }
        // Opened again by its real path: that spelling, once.
        let recent = remembered(recent, "/x/real/M.gguf", PathRule::Unix, canonical);
        assert_eq!(recent, strings(&["/x/real/M.gguf", "/m/b.gguf"]));
        let out = entries(&recent, &[], PathRule::Unix, canonical);
        assert_eq!(refs(&out), ["local:/x/real/M.gguf", "local:/m/b.gguf"]);

        // A Windows mapped drive, typed in another case, and its network path, the one the
        // file system gives back and the one Installed lists.
        let unc = |p: &str| {
            Some(if PathRule::Windows.key(p).ends_with(r"\MODELS\M.GGUF") {
                r"\\server\share\models\M.gguf".to_owned()
            } else {
                p.to_owned()
            })
        };
        let recent = remembered(
            strings(&[r"\\server\share\models\M.gguf"]),
            r"z:\MODELS\m.gguf",
            PathRule::Windows,
            unc,
        );
        assert_eq!(recent, strings(&[r"z:\MODELS\m.gguf"]));
        let out = entries(
            &recent,
            &strings(&[r"\\server\share\models\M.gguf"]),
            PathRule::Windows,
            unc,
        );
        assert_eq!(shown(&out), [("m.gguf".to_owned(), None)]);
        assert_eq!(refs(&out), [r"local:z:\MODELS\m.gguf"]);
    }

    /// Removing a model from Models → Installed forgets every spelling of its file; the rest of
    /// the list keeps its order.
    #[test]
    fn forgetting_drops_every_spelling_of_the_file() {
        let canonical = |p: &str| Some(p.replace("/link/", "/models/"));
        let out = forgotten(
            strings(&["/m/b.gguf", "/m/models/a.gguf", "/m/c.gguf"]),
            "/m/link/a.gguf",
            PathRule::Unix,
            canonical,
        );
        assert_eq!(out, strings(&["/m/b.gguf", "/m/c.gguf"]));
        let out = forgotten(
            strings(&[r"c:\models\X.gguf", r"C:\other\y.gguf"]),
            r"C:/Models/x.GGUF",
            PathRule::Windows,
            all_there,
        );
        assert_eq!(out, strings(&[r"C:\other\y.gguf"]));
        // A file that is gone is compared as written.
        let gone = |_: &str| None;
        let out = forgotten(
            strings(&["/m/a.gguf", "/m/b.gguf"]),
            "/m/a.gguf",
            PathRule::Unix,
            gone,
        );
        assert_eq!(out, strings(&["/m/b.gguf"]));
        let out = forgotten(
            strings(&["/m/a.gguf"]),
            "/m/z.gguf",
            PathRule::Unix,
            all_there,
        );
        assert_eq!(out, strings(&["/m/a.gguf"]));
    }

    /// The order every view reads: local files used recently, then the endpoints' models, then
    /// the installed models never used. The picker's two sections read as before; the first
    /// entries (the Welcome card, a new chat with nothing loaded) are the ones used last, as
    /// before installed models were listed. `recent` is sent only when true.
    #[test]
    fn recently_used_entries_are_marked_and_come_before_the_endpoints() {
        let local = entries(
            &strings(&["/m/used.gguf"]),
            &strings(&["/m/a-never.gguf", "/m/used.gguf"]),
            PathRule::Unix,
            all_there,
        );
        let endpoint = ModelOptionEntry::plain(ModelOptionDto {
            reference: "provider:1:remote-model".into(),
            label: "remote-model".into(),
            source: "provider".into(),
            context_length: Some(32768),
        });
        let out = arrange(local, [endpoint]);
        assert_eq!(
            refs(&out),
            [
                "local:/m/used.gguf",
                "provider:1:remote-model",
                "local:/m/a-never.gguf"
            ]
        );
        let json = serde_json::to_value(&out).unwrap();
        assert_eq!(json[0]["recent"], serde_json::json!(true));
        assert!(
            json[1].get("recent").is_none() && json[2].get("recent").is_none(),
            "{json}"
        );
        assert!(json[1].get("hint").is_none());
        // Nothing used yet and no endpoints: the installed models.
        let out = arrange(
            entries(&[], &strings(&["/m/a.gguf"]), PathRule::Unix, all_there),
            Vec::new(),
        );
        assert_eq!(refs(&out), ["local:/m/a.gguf"]);
    }

    /// A Hugging Face cache link (`snapshots/<rev>/x.gguf` pointing at `blobs/<sha>`): the
    /// file it points at has no `.gguf` name, and loading refuses such a path, so the link's
    /// own spelling is what is remembered. Other spellings of the file are still dropped.
    #[test]
    fn remembering_a_link_to_a_file_without_a_gguf_name_keeps_the_link() {
        let link = "/hf/snapshots/rev/Qwen-Q4_K_M.gguf";
        let canonical = |p: &str| {
            Some(if p == link || p == "/hf/blobs/3f2a9c0d1e" {
                "/hf/blobs/3f2a9c0d1e".to_owned()
            } else {
                p.to_owned()
            })
        };
        let out = remembered(
            strings(&["/hf/blobs/3f2a9c0d1e", "/m/b.gguf"]),
            link,
            PathRule::Unix,
            canonical,
        );
        assert_eq!(out, strings(&[link, "/m/b.gguf"]));
        let listed = entries(&out, &[], PathRule::Unix, canonical);
        assert_eq!(
            refs(&listed),
            [format!("local:{link}"), "local:/m/b.gguf".to_owned()]
        );
        assert_eq!(listed[0].option.label, "Qwen-Q4_K_M.gguf");
        // A link whose real path keeps a `.gguf` name, in any case, is kept as the link too.
        let upper = |p: &str| Some(p.replace("/link/", "/models/").replace("a.gguf", "A.GGUF"));
        let out = remembered(Vec::new(), "/m/link/a.gguf", PathRule::Unix, upper);
        assert_eq!(out, strings(&["/m/link/a.gguf"]));
        let dotfile = |_: &str| Some("/m/blobs/.gguf".to_owned());
        let out = remembered(Vec::new(), "/m/link/a.gguf", PathRule::Unix, dotfile);
        assert_eq!(out, strings(&["/m/link/a.gguf"]));
    }

    /// File names come from downloads and other programs; the label and the folder hint show
    /// every invisible or direction-changing character as a code (the picker's `visiblePath`
    /// does the same, so it changes nothing more), for every view that shows the list. The
    /// reference keeps the real path, so picking it loads that file.
    #[test]
    fn labels_and_hints_show_invisible_characters_as_codes() {
        let out = entries(
            &[],
            &strings(&[
                "/a/x\u{200B}y/evil\u{202E}fuug.Q4.gguf",
                "/b/z/evil\u{202E}fuug.Q4.gguf",
                "/c/tab\there\u{2066}.gguf",
            ]),
            PathRule::Unix,
            all_there,
        );
        assert_eq!(
            shown(&out),
            [
                (
                    "evil\\u{202E}fuug.Q4.gguf".to_owned(),
                    Some("a/x\\u{200B}y".to_owned())
                ),
                (
                    "evil\\u{202E}fuug.Q4.gguf".to_owned(),
                    Some("b/z".to_owned())
                ),
                ("tab\\u{0009}here\\u{2066}.gguf".to_owned(), None),
            ]
        );
        assert_eq!(
            out[0].option.reference,
            "local:/a/x\u{200B}y/evil\u{202E}fuug.Q4.gguf"
        );
        for (raw, shown) in [
            (
                "\u{0}\u{1F}\u{7F}\u{9F}\u{AD}\u{34F}\u{61C}",
                "\\u{0000}\\u{001F}\\u{007F}\\u{009F}\\u{00AD}\\u{034F}\\u{061C}",
            ),
            (
                "\u{115F}\u{1160}\u{17B4}\u{17B5}\u{180B}\u{180F}",
                "\\u{115F}\\u{1160}\\u{17B4}\\u{17B5}\\u{180B}\\u{180F}",
            ),
            (
                "\u{200F}\u{2028}\u{202A}\u{2060}\u{206F}\u{3164}",
                "\\u{200F}\\u{2028}\\u{202A}\\u{2060}\\u{206F}\\u{3164}",
            ),
            (
                "\u{FE00}\u{FE0F}\u{FEFF}\u{FFA0}\u{FFF9}\u{FFFB}",
                "\\u{FE00}\\u{FE0F}\\u{FEFF}\\u{FFA0}\\u{FFF9}\\u{FFFB}",
            ),
            (
                "\u{1BCA0}\u{1BCA3}\u{1D173}\u{1D17A}\u{E0001}\u{E0FFF}",
                "\\u{1BCA0}\\u{1BCA3}\\u{1D173}\\u{1D17A}\\u{E0001}\\u{E0FFF}",
            ),
            ("Straße é 模型 \\u{41} 🙂", "Straße é 模型 \\u{41} 🙂"),
        ] {
            assert_eq!(visible(raw), shown, "{raw:?}");
        }
    }

    /// The JSON the picker reads: the 0.0.4 fields as before, and `hint` only when there is one.
    #[test]
    fn entries_serialise_as_model_options_with_an_optional_hint() {
        let out = entries(
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

        let out = local_options(&store, None).await.unwrap();
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
    /// one entry, and remembering stores one spelling of it, the one used last; never used,
    /// the row spelled as the file's real path.
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
        assert_eq!(recent, vec![canonical.clone()]);

        let used_last = if cfg!(unix) {
            dir.join("link.gguf")
        } else {
            file.clone()
        };
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&file, &used_last).unwrap();
            register(&store, &used_last).await;
            remember(&store, &used_last.to_string_lossy()).await;
            let recent: Vec<String> = store
                .get_setting(RECENT_LOCAL_MODELS)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(recent, vec![used_last.to_string_lossy().into_owned()]);
        }
        let out = local_options(&store, None).await.unwrap();
        assert_eq!(out.len(), 1, "{:?}", refs(&out));
        assert_eq!(out[0].hint, None);
        // The spelling used last, the chat's reference.
        assert_eq!(refs(&out), [format!("local:{}", used_last.display())]);
        // Never used: the row spelled as the file's real path, not the link's.
        store
            .set_setting(RECENT_LOCAL_MODELS, &serde_json::json!([]))
            .await
            .unwrap();
        let out = local_options(&store, None).await.unwrap();
        assert_eq!(refs(&out), [format!("local:{}", file.display())]);
        assert_eq!(out[0].option.label, "m.gguf");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The Hugging Face cache through the real file system: a model opened through
    /// `snapshots/<rev>/x.gguf`, a link to `blobs/<sha>`, is remembered and listed by the
    /// link, whose name loading accepts.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_store_remembers_a_hub_cache_link_by_its_own_name() {
        let dir = temp_dir("hfcache");
        std::fs::create_dir_all(dir.join("blobs")).unwrap();
        std::fs::create_dir_all(dir.join("snapshots").join("rev")).unwrap();
        let blob = dir.join("blobs").join("3f2a9c0d1e");
        std::fs::write(&blob, b"GGUF").unwrap();
        let link = dir.join("snapshots").join("rev").join("Qwen-Q4_K_M.gguf");
        std::os::unix::fs::symlink("../../blobs/3f2a9c0d1e", &link).unwrap();
        let store = store::Store::open_in_memory().unwrap();
        remember(&store, &link.to_string_lossy()).await;
        let recent: Vec<String> = store
            .get_setting(RECENT_LOCAL_MODELS)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(recent, vec![link.to_string_lossy().into_owned()]);
        let out = local_options(&store, None).await.unwrap();
        assert_eq!(refs(&out), [format!("local:{}", link.display())]);
        assert_eq!(out[0].option.label, "Qwen-Q4_K_M.gguf");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// An imported model (import also remembers it) removed from Models → Installed leaves
    /// the chat's model list; its file stays. A model loaded through a link is listed under
    /// the link, while it is loaded and after.
    #[tokio::test]
    async fn the_store_forgets_a_removed_model_and_keeps_the_loaded_spelling() {
        let dir = temp_dir("forget");
        let file = dir.join("Imported.gguf");
        let other = dir.join("other.gguf");
        std::fs::write(&file, b"GGUF").unwrap();
        std::fs::write(&other, b"GGUF").unwrap();
        let store = store::Store::open_in_memory().unwrap();
        register(&store, &file).await;
        remember(&store, &other.to_string_lossy()).await;
        remember(&store, &file.to_string_lossy()).await;
        let model = store.list_models().await.unwrap().remove(0);
        store.delete_model(model.id).await.unwrap();
        forget(&store, &file.to_string_lossy()).await;
        let out = local_options(&store, None).await.unwrap();
        assert_eq!(refs(&out), [format!("local:{}", other.display())]);
        assert!(file.exists());
        // Forgetting a file that is not on the list writes nothing new.
        forget(&store, &file.to_string_lossy()).await;
        let recent: Vec<String> = store
            .get_setting(RECENT_LOCAL_MODELS)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(recent, vec![other.to_string_lossy().into_owned()]);

        #[cfg(unix)]
        {
            let link = dir.join("link.gguf");
            std::os::unix::fs::symlink(&other, &link).unwrap();
            let link = link.to_string_lossy().into_owned();
            remember(&store, &link).await;
            let out = local_options(&store, Some(&link)).await.unwrap();
            assert_eq!(refs(&out), [format!("local:{link}")]);
            let out = local_options(&store, None).await.unwrap();
            assert_eq!(refs(&out), [format!("local:{link}")]);
        }
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
        let out = local_options(&store, None).await.unwrap();
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
