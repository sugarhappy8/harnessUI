//! Skill folders are written only by the skills UI (ARCHITECTURE.md 9.3, 10). A skill
//! added to one is loaded at the next reload and its text reaches the system message, so
//! the file and git tools refuse to create, change or delete anything inside the folders
//! skills are loaded from ([`mcp_host::ChatContext::skill_roots`]), whatever was approved.
//! Reading is not limited, and neither is the terminal, which may run in a skill folder.

use policy::path::{CaseRule, Flavor, Fs, LexicalPath, default_case, is_within, lexical};

/// What counts as touching a skill folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reach {
    /// The path is a skill folder or inside one (a file write, a commit).
    Inside,
    /// Also when a skill folder is inside the path (a clone or pull that writes a tree).
    InsideOrAround,
}

/// The refusal for a path in (or, when not `inside`, around) the skill folder `root`.
fn refusal(path: &str, root: &str, inside: bool) -> String {
    let path = policy::sanitize::one_line(path, 300);
    let root = policy::sanitize::one_line(root, 300);
    let place = if inside { "is in" } else { "contains" };
    format!(
        "{path} {place} the skills folder {root}. Skills can only be added or changed in Settings → Skills."
    )
}

/// One skill folder as it is written and as it resolves (the deepest existing folder
/// resolved, with the rest appended: the folder may not exist yet), with the case rule of
/// the volume it is on.
fn forms(
    root: &str,
    flavor: Flavor,
    fs: &dyn Fs,
    case_of: &dyn Fn(&str) -> CaseRule,
) -> Vec<(LexicalPath, CaseRule)> {
    let Ok(lex) = lexical(root, None, flavor) else {
        return Vec::new();
    };
    let mut existing = lex.clone();
    let mut rest: Vec<String> = Vec::new();
    let resolved = loop {
        match fs.canonicalize(&existing.to_string()) {
            Ok(Some(canon)) => {
                let case = case_of(&canon);
                break lexical(&canon, None, flavor).ok().map(|mut c| {
                    rest.reverse();
                    c.components.extend(rest);
                    (c, case)
                });
            }
            Ok(None) => match existing.components.pop() {
                Some(last) => rest.push(last),
                None => break None,
            },
            Err(_) => break None,
        }
    };
    let case = resolved
        .as_ref()
        .map_or_else(|| default_case(flavor), |(_, case)| *case);
    let mut out = vec![(lex, case)];
    out.extend(resolved);
    out
}

/// The skill folder `path` touches, if any: `path` (canonical, as the sandbox resolved it)
/// is inside one of `roots`, or with [`Reach::InsideOrAround`] one of `roots` is inside
/// `path`. Returns the folder as written and whether `path` is inside it.
pub(crate) fn skill_root_for(
    path: &str,
    roots: &[String],
    flavor: Flavor,
    reach: Reach,
    fs: &dyn Fs,
    case_of: &dyn Fn(&str) -> CaseRule,
) -> Option<(String, bool)> {
    let Ok(target) = lexical(path, None, flavor) else {
        return None;
    };
    roots.iter().find_map(|root| {
        let forms = forms(root, flavor, fs, case_of);
        if forms.iter().any(|(f, case)| is_within(&target, f, *case)) {
            return Some((root.clone(), true));
        }
        (reach == Reach::InsideOrAround
            && forms.iter().any(|(f, case)| is_within(f, &target, *case)))
        .then(|| (root.clone(), false))
    })
}

/// [`skill_root_for`] on the real file system, with the refusal message.
pub(crate) fn check(
    path: &str,
    roots: &[String],
    flavor: Flavor,
    reach: Reach,
) -> Result<(), String> {
    match skill_root_for(path, roots, flavor, reach, &policy::path::RealFs, &|p| {
        policy::path::volume_case(p)
    }) {
        Some((root, inside)) => Err(refusal(path, &root, inside)),
        None => Ok(()),
    }
}

/// [`check`] of `path`, with a refusal that names `said` instead: the path as the model
/// gave it, for a message given before approval, which never names where a link on the
/// way leads (13.9, [`crate::errors::said_path`]).
pub(crate) fn check_as(
    path: &str,
    said: &str,
    roots: &[String],
    flavor: Flavor,
    reach: Reach,
) -> Result<(), String> {
    match skill_root_for(path, roots, flavor, reach, &policy::path::RealFs, &|p| {
        policy::path::volume_case(p)
    }) {
        Some((root, inside)) => Err(refusal(said, &root, inside)),
        None => Ok(()),
    }
}
