//! `browser_capture_site` as a policy action (ARCHITECTURE.md 13.3, 13.4): its arguments,
//! the folder check and the card.
//!
//! Saving the copy is a write into an approved folder, so the folder goes through the same
//! path sandbox as `write_file` (approved folders, the app's own folders never, skill
//! folders never, sensitive places always asking). The site goes through the same address
//! checks as `browser_navigate`. The grant scope names both
//! ([`policy::Scope::SiteCapture`]).

use mcp_host::ChatContext;
use policy::path::{
    CaseRule, Flavor, LexicalPath, PathSandbox, RealFs, is_within, lexical, volume_case,
};
use serde_json::{Map, Value};

use crate::capture::assets::{Caps, MAX_FILE_BYTES, megabytes, thousands};

/// Widths when the model gives none: a desktop and a phone.
pub const DEFAULT_WIDTHS: [u32; 2] = [1440, 390];
/// The narrowest and widest screen width accepted.
pub const MIN_WIDTH: u32 = 320;
pub const MAX_WIDTH: u32 = 2560;
/// Widths in one capture at most.
pub const MAX_WIDTHS: usize = 4;

/// The arguments, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub url: Option<String>,
    pub folder: Option<String>,
    pub widths: Vec<u32>,
    pub scroll: bool,
    pub caps: Caps,
}

/// Reads the arguments (the schema already held them to their types).
pub fn parse_args(args: &Map<String, Value>) -> Result<Args, String> {
    let s = |k: &str| {
        args.get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    let mut widths: Vec<u32> = Vec::new();
    if let Some(list) = args.get("widths").and_then(Value::as_array) {
        for w in list {
            let Some(n) = w.as_u64() else {
                return Err(
                    "widths must be whole numbers of pixels, for example [1440, 390]".into(),
                );
            };
            let n = u32::try_from(n).unwrap_or(u32::MAX);
            if !(MIN_WIDTH..=MAX_WIDTH).contains(&n) {
                return Err(format!(
                    "each width must be between {MIN_WIDTH} and {MAX_WIDTH} pixels; {n} is not"
                ));
            }
            if !widths.contains(&n) {
                widths.push(n);
            }
        }
        if widths.len() > MAX_WIDTHS {
            return Err(format!("give at most {MAX_WIDTHS} widths"));
        }
    }
    if widths.is_empty() {
        widths = DEFAULT_WIDTHS.to_vec();
    }
    let total = args.get("max_total_mb").and_then(Value::as_u64);
    let files = args
        .get("max_files")
        .and_then(Value::as_u64)
        .map(|f| usize::try_from(f).unwrap_or(usize::MAX));
    Ok(Args {
        url: s("url"),
        folder: s("folder"),
        widths,
        scroll: args.get("scroll").and_then(Value::as_bool).unwrap_or(true),
        caps: Caps::lowered(total, files),
    })
}

/// The approved folder a capture goes into, as the sandbox resolved it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    /// Canonical path of the folder (it may not exist yet).
    pub canonical: String,
    /// The approved folder that holds it (canonical).
    pub approved: String,
    pub exists: bool,
    pub was_symlink: bool,
    /// Why writing here always asks (a `.git` folder, an autostart folder…), if it does.
    pub always_asks: Option<&'static str>,
}

fn sandbox(chat: &ChatContext, flavor: Flavor) -> PathSandbox {
    let (sb, skipped) = PathSandbox::new(
        flavor,
        &chat.approved_folders,
        &chat.forbidden_folders,
        &RealFs,
        &|p| volume_case(p),
    );
    for (folder, why) in skipped {
        tracing::warn!(%folder, "approved folder skipped: {why}");
    }
    sb
}

/// Checks the folder a capture would write into: inside the chat's approved folders (in
/// Full access the whole computer, as the chat context already says), never in the app's
/// own folders or a skill folder, and a real folder when it exists. `Err` is a hard limit,
/// in words for the model.
pub fn check_folder(
    chat: &ChatContext,
    folder: Option<&str>,
    flavor: Flavor,
) -> Result<Folder, String> {
    let sb = sandbox(chat, flavor);
    let base = chat
        .working_folder
        .clone()
        .or_else(|| sb.roots().first().map(|r| r.canonical.to_string()));
    let roots: Vec<String> = sb.roots().iter().map(|r| r.canonical.to_string()).collect();
    let Some(path) = folder.map(str::to_string).or_else(|| base.clone()) else {
        return Err("No folder is approved for this chat yet, so the copy cannot be saved anywhere. The user can approve a folder in the Tools panel.".into());
    };
    let checked = sb.check(&path, base.as_deref(), &RealFs).map_err(|e| {
        if roots.is_empty() {
            format!("{e}. No folder is approved for this chat yet. The user can approve a folder in the Tools panel.")
        } else {
            format!(
                "{e}. Approved folders: {}. Save the copy inside one of them; the user can approve another folder in the Tools panel.",
                roots.join(", ")
            )
        }
    })?;
    let meta = std::fs::metadata(&checked.canonical);
    if let Ok(m) = &meta
        && !m.is_dir()
    {
        return Err(format!(
            "{} is a file, not a folder. Give the folder the copy should be saved in.",
            policy::sanitize::one_line(&checked.canonical, 300)
        ));
    }
    if let Some(root) = skill_folder_hit(&checked.canonical, &chat.skill_roots, flavor) {
        return Err(format!(
            "{} is in the skills folder {}. Skills can only be added or changed in Settings → Skills.",
            policy::sanitize::one_line(&checked.canonical, 300),
            policy::sanitize::one_line(&root, 300)
        ));
    }
    let sep = if flavor == Flavor::Windows { '\\' } else { '/' };
    let sample = format!(
        "{}{sep}capture{sep}index.html",
        checked.canonical.trim_end_matches(['/', '\\'])
    );
    let always_asks = policy::files::always_ask_reason(&sample, policy::WriteKind::Create, flavor);
    Ok(Folder {
        approved: sb
            .roots()
            .get(checked.root)
            .map(|r| r.canonical.to_string())
            .unwrap_or_default(),
        canonical: checked.canonical,
        exists: checked.exists,
        was_symlink: checked.was_symlink,
        always_asks,
    })
}

/// Checks the folder again right before the copy is written: the same canonical path,
/// still inside the approved folders, no link appeared at its end.
pub fn recheck_folder(chat: &ChatContext, approved: &Folder, flavor: Flavor) -> Result<(), String> {
    let again = check_folder(chat, Some(&approved.canonical), flavor)?;
    let a = lexical(&again.canonical, None, flavor).map_err(|e| e.to_string())?;
    let b = lexical(&approved.canonical, None, flavor).map_err(|e| e.to_string())?;
    let case = if flavor == Flavor::Windows || cfg!(target_os = "macos") {
        CaseRule::Insensitive
    } else {
        CaseRule::Sensitive
    };
    let same = a.components.len() == b.components.len() && is_within(&a, &b, case);
    if !same || (again.was_symlink && !approved.was_symlink) {
        return Err("Refused: the folder changed since this was approved (it now leads somewhere else). Describe the capture again.".into());
    }
    Ok(())
}

/// The skill folder `path` is inside, if any (as written or as it resolves).
fn skill_folder_hit(path: &str, roots: &[String], flavor: Flavor) -> Option<String> {
    let target = lexical(path, None, flavor).ok()?;
    roots.iter().find_map(|root| {
        let forms = forms_of(root, flavor);
        forms
            .iter()
            .any(|(f, case)| is_within(&target, f, *case))
            .then(|| root.clone())
    })
}

/// A folder as written and as it resolves (the deepest existing part resolved, the rest
/// appended), with the case rule of its volume.
fn forms_of(root: &str, flavor: Flavor) -> Vec<(LexicalPath, CaseRule)> {
    let Ok(lex) = lexical(root, None, flavor) else {
        return Vec::new();
    };
    let case = policy::path::default_case(flavor);
    let mut out = vec![(lex.clone(), case)];
    let mut existing = lex;
    let mut rest: Vec<String> = Vec::new();
    loop {
        if let Ok(canon) = dunce::canonicalize(existing.to_string()) {
            let canon = canon.to_string_lossy().into_owned();
            if let Ok(mut c) = lexical(&canon, None, flavor) {
                rest.reverse();
                c.components.extend(rest);
                out.push((c, volume_case(&canon)));
            }
            break;
        }
        match existing.components.pop() {
            Some(last) => rest.push(last),
            None => break,
        }
    }
    out
}

/// The card: a one-line summary and the preview, in plain words.
pub fn card(
    host: &str,
    shown_url: &str,
    origin: &str,
    addresses: &str,
    folder: &Folder,
    args: &Args,
    reload: bool,
) -> (String, String) {
    let folder_shown = policy::sanitize::visible(&folder.canonical);
    let widths = args
        .widths
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(" and ");
    let summary = format!("Save a copy of how {host} looks into {folder_shown}");
    let open = if reload {
        format!("Load the page open in the agent's browser again ({shown_url})")
    } else {
        format!("Open {shown_url} in the agent's browser")
    };
    let scroll = if args.scroll {
        ", scroll through it"
    } else {
        ""
    };
    let mut preview = String::new();
    if let Some(why) = folder.always_asks {
        let mut c = why.chars();
        if let Some(first) = c.next() {
            preview.push_str(&first.to_uppercase().collect::<String>());
            preview.push_str(c.as_str());
            preview.push_str(".\n\n");
        }
    }
    preview.push_str(&format!(
        "{open}{scroll}, and save a copy of its front end into a new folder inside:\n{folder_shown}\n"
    ));
    preview.push_str(&format!(
        "It saves the page as shown (with its scripts switched off), its style sheets, pictures, fonts and videos, a list of its animations, screenshots {widths} pixels wide, and a short report for the model.\n"
    ));
    preview.push_str(&format!(
        "At most {} and {} files in all; a file over {} is left out.\n",
        megabytes(args.caps.total),
        thousands(args.caps.files as u64),
        megabytes(MAX_FILE_BYTES)
    ));
    preview.push_str("It only reads the site: it does not click, type or send forms. The site's own scripts run in the agent's browser while the page is open, as on any visit.\n");
    preview.push_str(crate::capture::guide::PURPOSE);
    preview.push('\n');
    preview.push_str(&format!("Site: {origin}\n"));
    if !addresses.is_empty() {
        preview.push_str(&format!("Address: {addresses}\n"));
    }
    preview.push_str("Allow for this chat lets the agent save copies of this site into this folder again without asking.");
    (summary, preview)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn chat_with(folders: &[&std::path::Path]) -> ChatContext {
        ChatContext {
            chat_id: 1,
            approved_folders: folders.iter().map(|p| p.display().to_string()).collect(),
            result_cap_tokens: 4000,
            ..Default::default()
        }
    }

    #[test]
    fn arguments_default_and_refuse_what_is_out_of_range() {
        let a = parse_args(json!({}).as_object().unwrap()).unwrap();
        assert_eq!(a.widths, [1440, 390]);
        assert!(a.scroll);
        assert_eq!(a.caps, Caps::default());
        assert_eq!(a.url, None);
        let b = parse_args(
            json!({"url": " https://a.example ", "widths": [390, 390, 768], "scroll": false, "max_total_mb": 20, "max_files": 10})
                .as_object()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(b.url.as_deref(), Some("https://a.example"));
        assert_eq!(b.widths, [390, 768], "repeats are dropped");
        assert!(!b.scroll);
        assert_eq!(b.caps.total, 20 * 1024 * 1024);
        assert_eq!(b.caps.files, 10);
        for bad in [
            json!({"widths": [100]}),
            json!({"widths": [99999]}),
            json!({"widths": [400, 500, 600, 700, 800]}),
        ] {
            assert!(parse_args(bad.as_object().unwrap()).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_folder_must_be_inside_an_approved_folder() {
        let approved = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let chat = chat_with(&[approved.path()]);
        let ok = check_folder(
            &chat,
            Some(&approved.path().join("designs").display().to_string()),
            Flavor::native(),
        )
        .unwrap();
        assert!(!ok.exists, "a folder that does not exist yet is fine");
        assert!(ok.always_asks.is_none());
        let def = check_folder(&chat, None, Flavor::native()).unwrap();
        assert_eq!(
            std::path::Path::new(&def.canonical),
            dunce::canonicalize(approved.path()).unwrap(),
            "without a folder the copy goes into the working folder"
        );
        let err = check_folder(
            &chat,
            Some(&outside.path().display().to_string()),
            Flavor::native(),
        )
        .unwrap_err();
        assert!(err.contains("Approved folders:"), "{err}");
        let err = check_folder(&chat, Some("../../etc"), Flavor::native()).unwrap_err();
        assert!(err.contains("Approved folders:"), "{err}");
        let none = check_folder(&chat_with(&[]), None, Flavor::native()).unwrap_err();
        assert!(none.contains("No folder is approved"), "{none}");
        // The app's own folders are never reachable.
        let mut forbidden = chat_with(&[approved.path()]);
        forbidden.forbidden_folders = vec![approved.path().join("app-data").display().to_string()];
        assert!(
            check_folder(
                &forbidden,
                Some(&approved.path().join("app-data").display().to_string()),
                Flavor::native()
            )
            .is_err()
        );
        // Skill folders are written only by the skills UI.
        let mut skills = chat_with(&[approved.path()]);
        skills.skill_roots = vec![approved.path().join("skills").display().to_string()];
        let err = check_folder(
            &skills,
            Some(
                &approved
                    .path()
                    .join("skills")
                    .join("x")
                    .display()
                    .to_string(),
            ),
            Flavor::native(),
        )
        .unwrap_err();
        assert!(err.contains("skills folder"), "{err}");
        // A file is not a folder.
        std::fs::write(approved.path().join("notes.txt"), "x").unwrap();
        let err = check_folder(
            &chat,
            Some(&approved.path().join("notes.txt").display().to_string()),
            Flavor::native(),
        )
        .unwrap_err();
        assert!(err.contains("is a file"), "{err}");
        // A .git folder always asks.
        let git = check_folder(
            &chat,
            Some(&approved.path().join(".git").display().to_string()),
            Flavor::native(),
        )
        .unwrap();
        assert!(git.always_asks.is_some());
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_that_becomes_a_link_elsewhere_is_refused_at_the_recheck() {
        let approved = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let chat = chat_with(&[approved.path()]);
        let target = approved.path().join("site");
        std::fs::create_dir(&target).unwrap();
        let folder =
            check_folder(&chat, Some(&target.display().to_string()), Flavor::native()).unwrap();
        assert!(recheck_folder(&chat, &folder, Flavor::native()).is_ok());
        std::fs::remove_dir(&target).unwrap();
        std::os::unix::fs::symlink(outside.path(), &target).unwrap();
        assert!(recheck_folder(&chat, &folder, Flavor::native()).is_err());
    }

    #[test]
    fn the_card_says_what_where_and_how_much_in_plain_words() {
        let folder = Folder {
            canonical: "/home/u/site".into(),
            approved: "/home/u".into(),
            exists: true,
            was_symlink: false,
            always_asks: None,
        };
        let args = parse_args(json!({}).as_object().unwrap()).unwrap();
        let (summary, preview) = card(
            "example.com",
            "https://example.com/",
            "https://example.com",
            "93.184.215.14",
            &folder,
            &args,
            false,
        );
        assert_eq!(
            summary,
            "Save a copy of how example.com looks into /home/u/site"
        );
        for part in [
            "Open https://example.com/ in the agent's browser, scroll through it",
            "/home/u/site",
            "scripts switched off",
            "screenshots 1440 and 390 pixels wide",
            "At most 300 MB and 2,000 files in all; a file over 25 MB is left out.",
            "does not click, type or send forms",
            "logos, brand names or sign-in pages",
            "Site: https://example.com",
            "Address: 93.184.215.14",
            "Allow for this chat",
        ] {
            assert!(preview.contains(part), "{part} missing:\n{preview}");
        }
        assert!(!preview.contains('!'));
        let git = Folder {
            always_asks: Some("writing inside a .git folder always asks"),
            ..folder
        };
        let (_, preview) = card(
            "example.com",
            "https://example.com/",
            "https://example.com",
            "",
            &git,
            &args,
            true,
        );
        assert!(preview.starts_with("Writing inside a .git folder always asks.\n\nLoad the page open in the agent's browser again"), "{preview}");
    }
}
