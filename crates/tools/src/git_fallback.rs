//! The libgit2 fallback for the git tools (ARCHITECTURE.md 9.3), used when system git is
//! not installed.
//!
//! Local work (status, diff, log, commit, the fast-forward half of pull, resolving the
//! push URL) runs in the app's process on a blocking thread and checks the call's
//! cancellation token between steps (4.2, "local libgit2 work"). Network work runs in
//! the worker process (`git_worker`).
//!
//! libgit2 runs no hooks, no file-system monitor, no external diff or text-conversion
//! driver, no filter or merge driver and no credential helper, and it has no Git LFS.
//! Because filter drivers do not run, `git_commit` refuses paths whose `filter`
//! attribute names one (Git LFS, git-crypt, ...): staging them without their driver
//! would store the wrong content (for git-crypt, the plaintext). Remotes are https only
//! (no ssh); local paths are accepted only by the crate's own tests.

use std::path::{Path, PathBuf};

use git2::{
    AttrCheckFlags, AttrValue, DiffFormat, DiffOptions, IndexAddOption, Pathspec, PathspecFlags,
    Repository, RepositoryOpenFlags, RepositoryState, Sort, Status, StatusOptions,
};
use tokio_util::sync::CancellationToken;

use crate::git::check_remote_url;

/// Returned when Stop cancels a call.
pub(crate) const STOPPED: &str = "Stopped by the user";
/// Output kept at most (the same cap as the system-git path).
const OUTPUT_LIMIT: usize = 1024 * 1024;
/// Commits `git_log` looks at, at most, while filtering by path.
const MAX_LOG_SCAN: usize = 200_000;

/// Added to results of the tools where hooks or LFS would have mattered.
pub(crate) const RESULT_NOTE: &str = "(Git is not installed, so the built-in Git library ran this: no hooks ran, and Git LFS is not supported.)";
/// Added after a clone or pull when the repository uses filter drivers.
pub(crate) const FILTER_NOTE: &str = "This repository's .gitattributes names filter drivers (for example Git LFS). The built-in Git library cannot run them, so the files they manage are left as stored in the repository (LFS pointer files, encrypted files). Install Git, and Git LFS if needed, to get the real content.";

fn gerr(e: git2::Error) -> String {
    e.message().to_string()
}

fn check(cancel: &CancellationToken) -> Result<(), String> {
    if cancel.is_cancelled() {
        Err(STOPPED.into())
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------
// Remote URLs.
// ---------------------------------------------------------------------------------------

/// Why the fallback will not use a remote URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RemoteRefusal {
    /// Not allowed at all (a policy hard limit, as on the system-git path).
    Blocked(String),
    /// Allowed with system git, but the built-in library will not use it: ssh, or a URL
    /// not written in standard form. The model can fix the call.
    Unsupported(String),
}

impl RemoteRefusal {
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Blocked(m) | Self::Unsupported(m) => m,
        }
    }
}

/// Checks a remote URL for the fallback and returns the host a clone grant covers (the
/// same value the system-git path uses). https only; `allow_local` (tests only) also
/// accepts absolute local paths, reported as host `local`.
///
/// The URL must be exactly the url crate's serialisation of itself. libgit2 parses URLs
/// with its own parser, which takes everything between `//` and the first `/` as the
/// authority and the text after its last `@` as the host, while the url crate also ends
/// the authority at `\`, `?` and `#`. `https://github.com\@evil.example/` would otherwise
/// be approved, granted and keyed for tokens as github.com while libgit2 connects to
/// evil.example (and names the URL to the credential callback in the same ambiguous
/// form). In standard form the two parsers agree: the user part has `@`, `:` and `/`
/// percent-encoded, and the host is lowercase ASCII with no default port.
pub(crate) fn fallback_remote(url: &str, allow_local: bool) -> Result<String, RemoteRefusal> {
    if allow_local && !url.starts_with('-') && Path::new(url).is_absolute() {
        return Ok("local".into());
    }
    let host = check_remote_url(url).map_err(RemoteRefusal::Blocked)?;
    match url::Url::parse(url) {
        Ok(u) if u.scheme() == "https" && !host.is_empty() => {
            if u.as_str() == url {
                Ok(host)
            } else {
                Err(RemoteRefusal::Unsupported(format!(
                    "Git is not installed, and the built-in Git library only uses https URLs written in standard form, so that the server it connects to is the one shown for approval. Use {} instead.",
                    display_url(u.as_str())
                )))
            }
        }
        _ => Err(RemoteRefusal::Unsupported(
            "Git is not installed, and the built-in Git library reaches remotes over https only, not ssh. Use the repository's https address, or install Git.".into(),
        )),
    }
}

/// The host a token is stored under: lowercase ASCII (punycode), with the port when it is
/// not the default. `None` for anything that is not a URL with a host.
pub(crate) fn host_key(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    policy::sanitize::display_host(&u).map(|h| h.to_ascii_lowercase())
}

/// A URL as an approval card shows it: punycode host, and any user name and password in it
/// replaced by `***` (credentials are never shown or logged, 9.2; a token is often the
/// user name alone). Anything else is unchanged.
pub(crate) fn display_url(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(u) if u.has_host() => mask_userinfo(u.as_str()),
        _ => mask_userinfo(url),
    }
}

/// One `scheme://…` URL with its user name and password replaced by `***`: everything
/// before the last `@` of the authority, which is the text after `://` up to the first `/`,
/// `?` or `#` (as git and curl read it, see `git_url_host`). The rest is unchanged byte for
/// byte, so the card still shows the host git connects to. Text without `://` is returned
/// as it is.
pub(crate) fn mask_userinfo(url: &str) -> String {
    let Some(start) = url.find("://").map(|i| i + 3) else {
        return url.to_string();
    };
    let rest = &url[start..];
    let authority = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    match authority.rfind('@') {
        Some(at) => format!("{}***{}", &url[..start], &rest[at..]),
        None => url.to_string(),
    }
}

/// Free text (git's output, an error message) with the user name and password of every
/// `scheme://…` URL in it replaced by `***`. A URL's authority ends at `/`, `?`, `#` or
/// white space.
pub(crate) fn hide_credentials(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("://") {
        let start = i + 3;
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let end = after
            .find(|c: char| matches!(c, '/' | '?' | '#') || c.is_whitespace())
            .unwrap_or(after.len());
        let authority = &after[..end];
        match authority.rfind('@') {
            Some(at) => {
                out.push_str("***");
                out.push_str(&authority[at..]);
            }
            None => out.push_str(authority),
        }
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------------------------------
// Opening repositories.
// ---------------------------------------------------------------------------------------

/// Where a repository lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepoPaths {
    /// The working tree (what `git rev-parse --show-toplevel` prints).
    pub workdir: PathBuf,
    /// The repository's own git folder (`.git`, or the target of a `.git` file).
    pub gitdir: PathBuf,
    /// The shared git folder of a linked worktree (the same as `gitdir` otherwise).
    pub commondir: PathBuf,
}

fn clean(p: &Path) -> PathBuf {
    // libgit2 returns folders with a trailing slash, and forward slashes on Windows.
    p.components().collect()
}

fn repo_paths(repo: &Repository) -> Result<RepoPaths, String> {
    let workdir = repo.workdir().ok_or_else(|| {
        format!(
            "{} is a bare repository; the git tools need a working tree",
            repo.path().display()
        )
    })?;
    Ok(RepoPaths {
        workdir: clean(workdir),
        gitdir: clean(repo.path()),
        commondir: clean(repo.commondir()),
    })
}

/// The end of the refusal of a folder that is in no repository.
pub(crate) const NOT_INSIDE_A_REPOSITORY: &str = " is not inside a git repository";

/// Why [`discover`] found no repository to work on. The caller words the refusal, naming
/// the folder as the model gave it (13.9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NoRepository {
    /// The folder is in no repository.
    NotInside,
    /// The folder is in a bare repository, this git folder, which has no working tree.
    Bare(PathBuf),
}

/// The repository containing `start`, searching upward like `git rev-parse`.
pub(crate) fn discover(start: &Path) -> Result<RepoPaths, NoRepository> {
    let repo = Repository::open_ext(start, RepositoryOpenFlags::empty(), &[] as &[&str])
        .map_err(|_| NoRepository::NotInside)?;
    repo_paths(&repo).map_err(|_| NoRepository::Bare(clean(repo.path())))
}

/// Opens exactly the repository whose working tree is `root` (no search, no `GIT_DIR`).
pub(crate) fn open_at(root: &Path) -> Result<Repository, String> {
    let repo = Repository::open_ext(root, RepositoryOpenFlags::NO_SEARCH, &[] as &[&str]).map_err(
        |e| {
            format!(
                "{} is not a git repository: {}",
                root.display(),
                e.message()
            )
        },
    )?;
    repo_paths(&repo)?;
    Ok(repo)
}

/// The paths of the repository at `root`, for re-checking them before a side effect.
pub(crate) fn paths_at(root: &Path) -> Result<RepoPaths, String> {
    repo_paths(&open_at(root)?)
}

/// The full name of the checked-out branch (`refs/heads/main`), even before its first
/// commit. An error when HEAD is detached.
pub(crate) fn current_branch(repo: &Repository) -> Result<String, String> {
    let head = repo.find_reference("HEAD").map_err(gerr)?;
    match head.symbolic_target() {
        Ok(Some(t)) if t.starts_with("refs/heads/") => Ok(t.to_string()),
        _ => Err("HEAD is detached (no branch is checked out); check out a branch first".into()),
    }
}

/// The remote the branch pulls from (`branch.<name>.remote`).
pub(crate) fn upstream_remote(repo: &Repository, branch: &str) -> Result<String, String> {
    let buf = repo.branch_upstream_remote(branch).map_err(|_| {
        format!(
            "there is no tracking information for the branch {}; set an upstream first",
            short_ref(branch)
        )
    })?;
    buf.as_str()
        .map(str::to_string)
        .map_err(|_| "the upstream remote has an unreadable name".into())
}

/// The push URL as libgit2 resolves it (pushurl, else url, with insteadOf applied).
pub(crate) fn resolved_push_url(remote: &git2::Remote<'_>) -> Option<String> {
    match remote.pushurl() {
        Ok(Some(u)) if !u.is_empty() => Some(u.to_string()),
        _ => remote
            .url()
            .ok()
            .filter(|u| !u.is_empty())
            .map(str::to_string),
    }
}

/// The push URL of `remote` in the repository at `root`.
pub(crate) fn push_url(root: &Path, remote: &str) -> Result<String, String> {
    let repo = open_at(root)?;
    let r = repo
        .find_remote(remote)
        .map_err(|_| format!("the remote {remote} does not exist"))?;
    resolved_push_url(&r).ok_or_else(|| format!("the remote {remote} has no URL"))
}

/// The fetch URL of the current branch's upstream remote.
pub(crate) fn upstream_url(root: &Path) -> Result<String, String> {
    let repo = open_at(root)?;
    let branch = current_branch(&repo)?;
    let name = upstream_remote(&repo, &branch)?;
    let r = repo
        .find_remote(&name)
        .map_err(|_| format!("the remote {name} does not exist"))?;
    r.url()
        .ok()
        .filter(|u| !u.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("the remote {name} has no URL"))
}

/// Whether the top-level `.gitattributes` names a filter driver.
pub(crate) fn uses_filter_drivers(workdir: &Path) -> bool {
    std::fs::read_to_string(workdir.join(".gitattributes"))
        .map(|t| gitattributes_names_filter(&t))
        .unwrap_or(false)
}

/// Whether `.gitattributes` text assigns a filter driver to any pattern.
pub(crate) fn gitattributes_names_filter(text: &str) -> bool {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .any(|l| {
            l.split_whitespace()
                .skip(1)
                .any(|a| a.strip_prefix("filter=").is_some_and(|d| !d.is_empty()))
        })
}

fn short_ref(name: &str) -> &str {
    name.strip_prefix("refs/heads/")
        .or_else(|| name.strip_prefix("refs/remotes/"))
        .unwrap_or(name)
}

/// A user-given path as a libgit2 pathspec, relative to the repository root.
pub(crate) fn pathspec_for(root: &Path, p: &str) -> String {
    let path = Path::new(p);
    let rel = if path.is_absolute() {
        path.strip_prefix(root).unwrap_or(path)
    } else {
        path
    };
    let mut s = rel.to_string_lossy().into_owned();
    if cfg!(windows) {
        s = s.replace('\\', "/");
    }
    let s = s.trim_start_matches("./");
    if s.is_empty() || s == "." {
        "*".into()
    } else {
        s.to_string()
    }
}

// ---------------------------------------------------------------------------------------
// git_status
// ---------------------------------------------------------------------------------------

/// The checked-out branch, for the status header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HeadState {
    Branch { name: String, unborn: bool },
    Detached,
}

/// The branch's upstream, for the status header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Upstream {
    None,
    /// Configured, but the remote-tracking branch does not exist.
    Gone(String),
    Tracking {
        name: String,
        ahead: usize,
        behind: usize,
    },
}

/// The `## ...` line of `git status --porcelain=v1 --branch`.
pub(crate) fn format_branch_header(head: &HeadState, up: &Upstream) -> String {
    let mut s = match head {
        HeadState::Detached => return "## HEAD (no branch)".into(),
        HeadState::Branch { name, unborn: true } => format!("## No commits yet on {name}"),
        HeadState::Branch { name, .. } => format!("## {name}"),
    };
    match up {
        Upstream::None => {}
        Upstream::Gone(u) => s.push_str(&format!("...{u} [gone]")),
        Upstream::Tracking {
            name,
            ahead,
            behind,
        } => {
            s.push_str(&format!("...{name}"));
            match (ahead, behind) {
                (0, 0) => {}
                (a, 0) => s.push_str(&format!(" [ahead {a}]")),
                (0, b) => s.push_str(&format!(" [behind {b}]")),
                (a, b) => s.push_str(&format!(" [ahead {a}, behind {b}]")),
            }
        }
    }
    s
}

/// The two status letters of porcelain v1, or `None` for an unchanged or ignored entry.
pub(crate) fn porcelain_xy(s: Status) -> Option<[char; 2]> {
    if s.is_ignored() {
        return None;
    }
    if s.is_conflicted() {
        return Some(['U', 'U']);
    }
    let index = Status::INDEX_NEW
        | Status::INDEX_MODIFIED
        | Status::INDEX_DELETED
        | Status::INDEX_RENAMED
        | Status::INDEX_TYPECHANGE;
    if s.is_wt_new() && !s.intersects(index) {
        return Some(['?', '?']);
    }
    let x = if s.is_index_new() {
        'A'
    } else if s.is_index_modified() {
        'M'
    } else if s.is_index_deleted() {
        'D'
    } else if s.is_index_renamed() {
        'R'
    } else if s.is_index_typechange() {
        'T'
    } else {
        ' '
    };
    let y = if s.is_wt_modified() {
        'M'
    } else if s.is_wt_deleted() {
        'D'
    } else if s.is_wt_typechange() {
        'T'
    } else if s.is_wt_renamed() {
        'R'
    } else {
        ' '
    };
    if x == ' ' && y == ' ' {
        None
    } else {
        Some([x, y])
    }
}

fn head_state(repo: &Repository) -> (HeadState, Upstream) {
    let Ok(branch_ref) = current_branch(repo) else {
        return (HeadState::Detached, Upstream::None);
    };
    let name = short_ref(&branch_ref).to_string();
    let local = repo.refname_to_id(&branch_ref).ok();
    let head = HeadState::Branch {
        name,
        unborn: local.is_none(),
    };
    let Ok(up) = repo.branch_upstream_name(&branch_ref) else {
        return (head, Upstream::None);
    };
    let Ok(up) = up.as_str().map(str::to_string) else {
        return (head, Upstream::None);
    };
    let up_short = short_ref(&up).to_string();
    let Ok(up_id) = repo.refname_to_id(&up) else {
        return (head, Upstream::Gone(up_short));
    };
    let (ahead, behind) = local
        .and_then(|l| repo.graph_ahead_behind(l, up_id).ok())
        .unwrap_or((0, 0));
    (
        head,
        Upstream::Tracking {
            name: up_short,
            ahead,
            behind,
        },
    )
}

/// `git status --porcelain=v1 --branch`.
pub(crate) fn status(root: &Path, cancel: &CancellationToken) -> Result<String, String> {
    let repo = open_at(root)?;
    let (head, up) = head_state(&repo);
    let mut out = format_branch_header(&head, &up);
    check(cancel)?;
    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(false)
        .include_ignored(false)
        .renames_head_to_index(true);
    let statuses = repo.statuses(Some(&mut opts)).map_err(gerr)?;
    check(cancel)?;
    let mut tracked = Vec::new();
    let mut untracked = Vec::new();
    let mut size = out.len();
    let mut cut = false;
    for entry in statuses.iter() {
        let Some(xy) = porcelain_xy(entry.status()) else {
            continue;
        };
        let path = String::from_utf8_lossy(entry.path_bytes()).into_owned();
        let old = entry
            .head_to_index()
            .filter(|_| xy[0] == 'R')
            .and_then(|d| {
                d.old_file()
                    .path()
                    .map(|p| p.to_string_lossy().into_owned())
            });
        let line = match old {
            Some(old) if old != path => format!("{}{} {old} -> {path}", xy[0], xy[1]),
            _ => format!("{}{} {path}", xy[0], xy[1]),
        };
        size += line.len() + 1;
        if size > OUTPUT_LIMIT {
            cut = true;
            break;
        }
        if xy == ['?', '?'] {
            untracked.push(line);
        } else {
            tracked.push(line);
        }
    }
    for l in tracked.into_iter().chain(untracked) {
        out.push('\n');
        out.push_str(&l);
    }
    if cut {
        out.push_str("\n[... status cut at 1 MiB ...]");
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------
// git_diff
// ---------------------------------------------------------------------------------------

/// `git diff [--cached] -- [path]`, with no external or text-conversion drivers.
pub(crate) fn diff(
    root: &Path,
    staged: bool,
    path: Option<&str>,
    cancel: &CancellationToken,
) -> Result<String, String> {
    let repo = open_at(root)?;
    let mut opts = DiffOptions::new();
    if let Some(p) = path {
        opts.pathspec(pathspec_for(root, p));
    }
    let d = if staged {
        let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
        repo.diff_tree_to_index(head_tree.as_ref(), None, Some(&mut opts))
    } else {
        repo.diff_index_to_workdir(None, Some(&mut opts))
    }
    .map_err(gerr)?;
    let mut out = String::new();
    let mut cut = false;
    let mut stopped = false;
    let printed = d.print(DiffFormat::Patch, |_, _, line| {
        if cancel.is_cancelled() {
            stopped = true;
            return false;
        }
        if out.len() >= OUTPUT_LIMIT {
            cut = true;
            return false;
        }
        if matches!(line.origin(), '+' | '-' | ' ') {
            out.push(line.origin());
        }
        out.push_str(&String::from_utf8_lossy(line.content()));
        true
    });
    if stopped {
        return Err(STOPPED.into());
    }
    if cut {
        out.push_str("\n[... diff cut at 1 MiB ...]");
    } else {
        printed.map_err(gerr)?;
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------
// git_log
// ---------------------------------------------------------------------------------------

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    // Howard Hinnant's days-to-civil algorithm (proleptic Gregorian calendar).
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// `--date=short`: the date in the commit's own time zone.
pub(crate) fn short_date(seconds: i64, offset_minutes: i32) -> String {
    let local = seconds + i64::from(offset_minutes) * 60;
    let (y, m, d) = civil_from_days(local.div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

fn short_id(obj: &git2::Object<'_>) -> String {
    obj.short_id()
        .ok()
        .and_then(|b| b.as_str().ok().map(str::to_string))
        .unwrap_or_else(|| obj.id().to_string().chars().take(7).collect())
}

/// Whether the commit changes `spec`: against its parent, or for a merge against every
/// parent (git's default history simplification shows a merge only then).
fn touches(repo: &Repository, commit: &git2::Commit<'_>, spec: &str) -> Result<bool, String> {
    let tree = commit.tree().map_err(gerr)?;
    let differs = |parent: Option<&git2::Tree<'_>>| -> Result<bool, String> {
        let mut o = DiffOptions::new();
        o.pathspec(spec);
        let d = repo
            .diff_tree_to_tree(parent, Some(&tree), Some(&mut o))
            .map_err(gerr)?;
        Ok(d.deltas().len() > 0)
    };
    if commit.parent_count() == 0 {
        return differs(None);
    }
    for p in commit.parents() {
        if !differs(Some(&p.tree().map_err(gerr)?))? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// `git log -n<limit> --date=short --pretty=format:'%h %ad %an: %s' -- [path]`.
pub(crate) fn log(
    root: &Path,
    limit: usize,
    path: Option<&str>,
    cancel: &CancellationToken,
) -> Result<String, String> {
    let repo = open_at(root)?;
    let branch = current_branch(&repo).ok();
    if repo.head().is_err() {
        let b = branch.as_deref().map(short_ref).unwrap_or("HEAD");
        return Err(format!(
            "your current branch '{b}' does not have any commits yet"
        ));
    }
    let spec = path.map(|p| pathspec_for(root, p));
    let mut walk = repo.revwalk().map_err(gerr)?;
    walk.set_sorting(Sort::TIME).map_err(gerr)?;
    walk.push_head().map_err(gerr)?;
    let mut lines = Vec::new();
    for (scanned, oid) in walk.enumerate() {
        check(cancel)?;
        if scanned >= MAX_LOG_SCAN {
            lines.push(format!(
                "[... stopped after looking at {MAX_LOG_SCAN} commits ...]"
            ));
            break;
        }
        let commit = repo.find_commit(oid.map_err(gerr)?).map_err(gerr)?;
        if let Some(spec) = &spec
            && !touches(&repo, &commit, spec)?
        {
            continue;
        }
        let when = commit.author().when();
        lines.push(format!(
            "{} {} {}: {}",
            short_id(commit.as_object()),
            short_date(when.seconds(), when.offset_minutes()),
            String::from_utf8_lossy(commit.author().name_bytes()),
            String::from_utf8_lossy(commit.summary_bytes().unwrap_or_default())
        ));
        if lines.len() >= limit {
            break;
        }
    }
    Ok(lines.join("\n"))
}

// ---------------------------------------------------------------------------------------
// git_commit
// ---------------------------------------------------------------------------------------

/// Name and email for `role` ("AUTHOR" or "COMMITTER"): `GIT_<role>_NAME` /
/// `GIT_<role>_EMAIL` from the tool's environment, else `user.name` / `user.email` from
/// git config, else `EMAIL` for the address.
pub(crate) fn identity(
    env: &[(String, String)],
    role: &str,
    config_name: Option<&str>,
    config_email: Option<&str>,
) -> Option<(String, String)> {
    let get = |k: &str| {
        env.iter()
            .rev()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let cfg = |v: Option<&str>| {
        v.map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    let name = get(&format!("GIT_{role}_NAME")).or_else(|| cfg(config_name))?;
    let email = get(&format!("GIT_{role}_EMAIL"))
        .or_else(|| cfg(config_email))
        .or_else(|| get("EMAIL"))?;
    Some((name, email))
}

/// The summary line git prints under a commit or a fast-forward.
pub(crate) fn format_stats(files: usize, insertions: usize, deletions: usize) -> String {
    let plural = |n: usize, one: &str, many: &str| {
        if n == 1 {
            format!("{n} {one}")
        } else {
            format!("{n} {many}")
        }
    };
    let mut s = format!(" {}", plural(files, "file changed", "files changed"));
    if insertions > 0 || deletions == 0 {
        s.push_str(&format!(
            ", {}",
            plural(insertions, "insertion(+)", "insertions(+)")
        ));
    }
    if deletions > 0 || insertions == 0 {
        s.push_str(&format!(
            ", {}",
            plural(deletions, "deletion(-)", "deletions(-)")
        ));
    }
    s
}

fn stats_line(
    repo: &Repository,
    old: Option<&git2::Tree<'_>>,
    new: &git2::Tree<'_>,
) -> Option<String> {
    let d = repo.diff_tree_to_tree(old, Some(new), None).ok()?;
    let s = d.stats().ok()?;
    Some(format_stats(
        s.files_changed(),
        s.insertions(),
        s.deletions(),
    ))
}

/// The filter driver git would run for `path`, if any.
fn filter_driver(repo: &Repository, path: &Path) -> Option<String> {
    let v = repo
        .get_attr(path, "filter", AttrCheckFlags::FILE_THEN_INDEX)
        .ok()?;
    match AttrValue::from_string(v) {
        AttrValue::String(d) if !d.is_empty() => Some(d.to_string()),
        _ => None,
    }
}

/// `git add --all -- <specs>` (everything when `specs` is empty), refusing any path a
/// filter driver manages. Nothing is written when a path is refused.
fn stage(
    repo: &Repository,
    index: &mut git2::Index,
    specs: &[String],
    cancel: &CancellationToken,
) -> Result<(), String> {
    let all = specs.is_empty();
    let list: Vec<String> = if all {
        vec!["*".into()]
    } else {
        specs.to_vec()
    };
    let mut refused: Vec<(String, String)> = Vec::new();
    for spec in &list {
        if !all {
            let ps = Pathspec::new([spec.as_str()]).map_err(gerr)?;
            let in_workdir = ps
                .match_workdir(repo, PathspecFlags::DEFAULT)
                .map(|m| m.entries().len() > 0)
                .unwrap_or(false);
            let in_index = ps
                .match_index(index, PathspecFlags::DEFAULT)
                .map(|m| m.entries().len() > 0)
                .unwrap_or(false);
            if !in_workdir && !in_index {
                return Err(format!("pathspec '{spec}' did not match any files"));
            }
        }
        let mut gate = |path: &Path, _matched: &[u8]| -> i32 {
            if cancel.is_cancelled() {
                return -1; // abort
            }
            // Filter drivers run on file content only: removing a managed file, or a
            // symlink, needs no driver (git commits those without one too).
            let content = repo
                .workdir()
                .and_then(|w| std::fs::symlink_metadata(w.join(path)).ok())
                .is_some_and(|m| m.is_file());
            if !content {
                return 0;
            }
            if let Some(driver) = filter_driver(repo, path) {
                let shown = path.to_string_lossy().into_owned();
                if !refused.iter().any(|(p, _)| *p == shown) {
                    refused.push((shown, driver));
                }
                return 1; // skip
            }
            0
        };
        let added = index.add_all([spec.as_str()], IndexAddOption::DEFAULT, Some(&mut gate));
        if cancel.is_cancelled() {
            break;
        }
        added.map_err(gerr)?;
        let updated = index.update_all([spec.as_str()], Some(&mut gate));
        if cancel.is_cancelled() {
            break;
        }
        updated.map_err(gerr)?;
    }
    let stopped = cancel.is_cancelled();
    if stopped || !refused.is_empty() {
        // Throw away the in-memory changes; nothing was written.
        let _ = index.read(true);
    }
    if stopped {
        return Err(STOPPED.into());
    }
    if !refused.is_empty() {
        let shown: Vec<String> = refused
            .iter()
            .take(5)
            .map(|(p, d)| format!("{p} (filter \"{d}\")"))
            .collect();
        let more = refused.len().saturating_sub(5);
        return Err(format!(
            "nothing was committed: {}{} {} managed by a filter driver (such as Git LFS or git-crypt). Git is not installed, and the built-in Git library cannot run filter drivers, so committing {} would store the wrong content. Install Git{} to commit {}.",
            shown.join(", "),
            if more > 0 {
                format!(" and {more} more")
            } else {
                String::new()
            },
            if refused.len() == 1 { "is" } else { "are" },
            if refused.len() == 1 { "it" } else { "them" },
            if refused.iter().any(|(_, d)| d == "lfs") {
                " and Git LFS"
            } else {
                ""
            },
            if refused.len() == 1 { "it" } else { "them" },
        ));
    }
    Ok(())
}

/// Stages `paths` (everything when empty) and commits with `message`. No hooks run.
pub(crate) fn commit(
    root: &Path,
    message: &str,
    paths: &[String],
    env: &[(String, String)],
    cancel: &CancellationToken,
) -> Result<String, String> {
    let repo = open_at(root)?;
    if repo.state() != RepositoryState::Clean {
        return Err(format!(
            "the repository is in the middle of another operation ({:?}); finish it first",
            repo.state()
        ));
    }
    let message = git2::message_prettify(message, None).map_err(gerr)?;
    if message.trim().is_empty() {
        return Err("Aborting commit due to empty commit message.".into());
    }
    let config = repo.config().map_err(gerr)?;
    let cfg_name = config.get_string("user.name").ok();
    let cfg_email = config.get_string("user.email").ok();
    let who = |role: &str| {
        identity(env, role, cfg_name.as_deref(), cfg_email.as_deref()).ok_or_else(|| {
            "Commits need an author name and email, and none is set. Git is not installed, so add them to the Git settings file in your home folder (.gitconfig):\n[user]\n    name = Your Name\n    email = you@example.com".to_string()
        })
    };
    let (author_name, author_email) = who("AUTHOR")?;
    let (committer_name, committer_email) = who("COMMITTER")?;
    let specs: Vec<String> = paths.iter().map(|p| pathspec_for(root, p)).collect();
    let mut index = repo.index().map_err(gerr)?;
    stage(&repo, &mut index, &specs, cancel)?;
    check(cancel)?;
    index.write().map_err(gerr)?;
    let tree_id = index.write_tree().map_err(gerr)?;
    let tree = repo.find_tree(tree_id).map_err(gerr)?;
    let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    let parent_tree = parent.as_ref().and_then(|p| p.tree().ok());
    let unchanged = match &parent_tree {
        Some(t) => t.id() == tree_id,
        None => tree.is_empty(),
    };
    if unchanged {
        return Err("nothing to commit, working tree clean".into());
    }
    check(cancel)?;
    let author = git2::Signature::now(&author_name, &author_email).map_err(gerr)?;
    let committer = git2::Signature::now(&committer_name, &committer_email).map_err(gerr)?;
    let parents: Vec<&git2::Commit<'_>> = parent.iter().collect();
    let id = repo
        .commit(Some("HEAD"), &author, &committer, &message, &tree, &parents)
        .map_err(gerr)?;
    let commit = repo.find_commit(id).map_err(gerr)?;
    let branch = current_branch(&repo)
        .map(|b| short_ref(&b).to_string())
        .unwrap_or_else(|_| "HEAD".into());
    let mut out = format!(
        "[{branch}{} {}] {}",
        if parent.is_none() {
            " (root-commit)"
        } else {
            ""
        },
        short_id(commit.as_object()),
        String::from_utf8_lossy(commit.summary_bytes().unwrap_or_default())
    );
    if let Some(stats) = stats_line(&repo, parent_tree.as_ref(), &tree) {
        out.push('\n');
        out.push_str(&stats);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------
// git_pull (the local half)
// ---------------------------------------------------------------------------------------

/// Fast-forwards the current branch to its (already fetched) upstream, like
/// `git merge --ff-only`: local changes that the update would overwrite stop it, and
/// nothing changes then.
pub(crate) fn fast_forward(root: &Path, cancel: &CancellationToken) -> Result<String, String> {
    let repo = open_at(root)?;
    if repo.state() != RepositoryState::Clean {
        return Err(format!(
            "the repository is in the middle of another operation ({:?}); finish it first",
            repo.state()
        ));
    }
    let branch = current_branch(&repo)?;
    let up = repo.branch_upstream_name(&branch).map_err(|_| {
        format!(
            "there is no tracking information for the branch {}",
            short_ref(&branch)
        )
    })?;
    let up = up
        .as_str()
        .map_err(|_| "the upstream has an unreadable name".to_string())?
        .to_string();
    let up_ref = repo
        .find_reference(&up)
        .map_err(|_| format!("the upstream {} does not exist", short_ref(&up)))?;
    let up_id = up_ref
        .resolve()
        .ok()
        .and_then(|r| r.target())
        .ok_or_else(|| format!("the upstream {} does not point at a commit", short_ref(&up)))?;
    let annotated = repo.reference_to_annotated_commit(&up_ref).map_err(gerr)?;
    let (analysis, _) = repo.merge_analysis(&[&annotated]).map_err(gerr)?;
    if analysis.is_up_to_date() {
        return Ok("Already up to date.".into());
    }
    if !analysis.is_fast_forward() && !analysis.is_unborn() {
        return Err(format!(
            "Not possible to fast-forward: {} and {} have diverged. Nothing was changed; merge or rebase them yourself.",
            short_ref(&branch),
            short_ref(&up)
        ));
    }
    check(cancel)?;
    let old_id = repo.refname_to_id(&branch).ok();
    let old_tree = old_id
        .and_then(|id| repo.find_commit(id).ok())
        .and_then(|c| c.tree().ok());
    let target = repo.find_commit(up_id).map_err(gerr)?;
    let new_tree = target.tree().map_err(gerr)?;
    let mut co = git2::build::CheckoutBuilder::new();
    co.safe();
    repo.checkout_tree(target.as_object(), Some(&mut co))
        .map_err(|e| {
            format!(
                "Nothing was changed: the update would overwrite local changes ({}). Commit or undo them first.",
                e.message()
            )
        })?;
    let log_msg = format!("pull: fast-forward to {}", short_ref(&up));
    match repo.find_reference(&branch) {
        Ok(mut r) => {
            r.set_target(up_id, &log_msg).map_err(gerr)?;
        }
        Err(_) => {
            repo.reference(&branch, up_id, false, &log_msg)
                .map_err(gerr)?;
        }
    }
    let new_short = short_id(target.as_object());
    let mut out = match old_id {
        Some(old) => format!(
            "Updating {}..{new_short}\nFast-forward",
            old.to_string()
                .chars()
                .take(new_short.len())
                .collect::<String>()
        ),
        None => format!("Checked out {new_short} on {}", short_ref(&branch)),
    };
    if let Some(stats) = stats_line(&repo, old_tree.as_ref(), &new_tree) {
        out.push('\n');
        out.push_str(&stats);
    }
    if uses_filter_drivers(root) {
        out.push('\n');
        out.push_str(FILTER_NOTE);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_letters() {
        assert_eq!(porcelain_xy(Status::WT_NEW), Some(['?', '?']));
        assert_eq!(porcelain_xy(Status::INDEX_NEW), Some(['A', ' ']));
        assert_eq!(
            porcelain_xy(Status::INDEX_NEW | Status::WT_MODIFIED),
            Some(['A', 'M'])
        );
        assert_eq!(porcelain_xy(Status::WT_MODIFIED), Some([' ', 'M']));
        assert_eq!(porcelain_xy(Status::WT_DELETED), Some([' ', 'D']));
        assert_eq!(porcelain_xy(Status::INDEX_DELETED), Some(['D', ' ']));
        assert_eq!(porcelain_xy(Status::INDEX_RENAMED), Some(['R', ' ']));
        assert_eq!(porcelain_xy(Status::INDEX_TYPECHANGE), Some(['T', ' ']));
        assert_eq!(porcelain_xy(Status::CONFLICTED), Some(['U', 'U']));
        assert_eq!(porcelain_xy(Status::IGNORED), None);
        assert_eq!(porcelain_xy(Status::CURRENT), None);
    }

    #[test]
    fn branch_headers() {
        let b = |unborn| HeadState::Branch {
            name: "main".into(),
            unborn,
        };
        assert_eq!(format_branch_header(&b(false), &Upstream::None), "## main");
        assert_eq!(
            format_branch_header(&b(true), &Upstream::None),
            "## No commits yet on main"
        );
        assert_eq!(
            format_branch_header(&HeadState::Detached, &Upstream::None),
            "## HEAD (no branch)"
        );
        let t = |ahead, behind| Upstream::Tracking {
            name: "origin/main".into(),
            ahead,
            behind,
        };
        assert_eq!(
            format_branch_header(&b(false), &t(0, 0)),
            "## main...origin/main"
        );
        assert_eq!(
            format_branch_header(&b(false), &t(2, 0)),
            "## main...origin/main [ahead 2]"
        );
        assert_eq!(
            format_branch_header(&b(false), &t(0, 3)),
            "## main...origin/main [behind 3]"
        );
        assert_eq!(
            format_branch_header(&b(false), &t(1, 1)),
            "## main...origin/main [ahead 1, behind 1]"
        );
        assert_eq!(
            format_branch_header(&b(false), &Upstream::Gone("origin/x".into())),
            "## main...origin/x [gone]"
        );
    }

    #[test]
    fn dates_are_in_the_commit_time_zone() {
        assert_eq!(short_date(0, 0), "1970-01-01");
        // 2026-11-10T23:30:00Z is already the 11th at UTC+01:00 ...
        assert_eq!(short_date(1_794_353_400, 0), "2026-11-10");
        assert_eq!(short_date(1_794_353_400, 60), "2026-11-11");
        // ... and still the 10th at UTC-08:00.
        assert_eq!(short_date(1_794_353_400, -480), "2026-11-10");
        assert_eq!(short_date(951_782_400, 0), "2000-02-29");
        assert_eq!(short_date(-86_400, 0), "1969-12-31");
        assert_eq!(short_date(1_709_164_800, 0), "2024-02-29");
    }

    #[test]
    fn identity_prefers_the_environment_then_config() {
        let env = vec![
            ("GIT_AUTHOR_NAME".to_string(), "Env Author".to_string()),
            ("GIT_AUTHOR_EMAIL".to_string(), "a@env".to_string()),
        ];
        assert_eq!(
            identity(&env, "AUTHOR", Some("Cfg"), Some("c@cfg")),
            Some(("Env Author".into(), "a@env".into()))
        );
        assert_eq!(
            identity(&env, "COMMITTER", Some("Cfg"), Some("c@cfg")),
            Some(("Cfg".into(), "c@cfg".into()))
        );
        let email_only = vec![("EMAIL".to_string(), "e@mail".to_string())];
        assert_eq!(
            identity(&email_only, "AUTHOR", Some("Cfg"), None),
            Some(("Cfg".into(), "e@mail".into()))
        );
        assert_eq!(identity(&[], "AUTHOR", Some("  "), Some("x@y")), None);
        assert_eq!(identity(&[], "AUTHOR", Some("N"), None), None);
    }

    #[test]
    fn stats_lines_match_git() {
        assert_eq!(format_stats(1, 1, 0), " 1 file changed, 1 insertion(+)");
        assert_eq!(
            format_stats(2, 3, 1),
            " 2 files changed, 3 insertions(+), 1 deletion(-)"
        );
        assert_eq!(format_stats(1, 0, 2), " 1 file changed, 2 deletions(-)");
        assert_eq!(
            format_stats(1, 0, 0),
            " 1 file changed, 0 insertions(+), 0 deletions(-)"
        );
    }

    #[test]
    fn fallback_remotes_are_https_only() {
        assert_eq!(
            fallback_remote("https://github.com/a/b.git", false),
            Ok("github.com".into())
        );
        assert!(matches!(
            fallback_remote("git@github.com:a/b.git", false),
            Err(RemoteRefusal::Unsupported(_))
        ));
        assert!(matches!(
            fallback_remote("ssh://git@host/x", false),
            Err(RemoteRefusal::Unsupported(_))
        ));
        for bad in [
            "http://plain.example/x",
            "file:///etc",
            "/local/path",
            "ext::sh -c touch",
            "-uploadpack=evil",
            "git://x/y",
        ] {
            assert!(
                matches!(fallback_remote(bad, false), Err(RemoteRefusal::Blocked(_))),
                "{bad}"
            );
        }
        // Local paths only when the crate's tests allow them, and never file: URLs.
        let local = if cfg!(windows) {
            "C:\\r\\bare.git"
        } else {
            "/r/bare.git"
        };
        assert_eq!(fallback_remote(local, true), Ok("local".into()));
        assert!(fallback_remote("file:///r/bare.git", true).is_err());
        assert!(fallback_remote("relative/bare.git", true).is_err());
    }

    #[test]
    fn fallback_remotes_must_be_written_in_standard_form() {
        // libgit2 takes everything between "//" and the first "/" as the authority and the
        // text after its last '@' as the host; the url crate stops at '\', '?' and '#'.
        // Each of these would be approved (and keyed for tokens) as github.com while
        // libgit2 connects to evil.example, or the other way round.
        for bad in [
            "https://github.com\\@evil.example/r.git",
            "https://evil.example?@github.com/r.git",
            "https://evil.example#@github.com/r.git",
            // Not dangerous, but not in standard form either: refused the same way.
            "https://GitHub.com/a/b.git",
            "https://github.com:443/a/b.git",
            "https://bücher.example/r.git",
            "https://github.com/a/../b.git",
        ] {
            match fallback_remote(bad, false) {
                Err(RemoteRefusal::Unsupported(m)) => {
                    assert!(m.contains("standard form"), "{bad}: {m}")
                }
                // Refused earlier by the shared URL check (a URL git reads differently).
                Err(RemoteRefusal::Blocked(m)) => {
                    assert!(
                        m.contains("backslash") || m.contains("ambiguous"),
                        "{bad}: {m}"
                    )
                }
                other => panic!("{bad}: {other:?}"),
            }
        }
        // The message names the standard form, with any user name and password hidden.
        let Err(why) = fallback_remote("https://u:pw@GitHub.com/a/b.git", false) else {
            panic!()
        };
        assert!(
            why.message().contains("https://***@github.com/a/b.git"),
            "{why:?}"
        );
        for good in [
            "https://github.com/a/b.git",
            "https://user@github.com/a/b.git",
            "https://user:p%40ss@github.com/a/b.git",
            "https://git.corp:8443/a/b.git",
            "https://xn--bcher-kva.example/r.git",
        ] {
            assert!(fallback_remote(good, false).is_ok(), "{good}");
        }
    }

    #[test]
    fn token_hosts_include_non_default_ports() {
        assert_eq!(
            host_key("https://GitHub.com/a/b").as_deref(),
            Some("github.com")
        );
        assert_eq!(
            host_key("https://git.corp:8443/a").as_deref(),
            Some("git.corp:8443")
        );
        assert_eq!(
            host_key("https://git.corp:443/a").as_deref(),
            Some("git.corp")
        );
        assert_eq!(
            host_key("https://bücher.example/r").as_deref(),
            Some("xn--bcher-kva.example")
        );
        assert_eq!(host_key("git@github.com:a/b.git"), None);
    }

    #[test]
    fn display_urls_hide_passwords_and_use_punycode() {
        assert_eq!(
            display_url("https://user:ghp_secret@github.com/a/b.git"),
            "https://***@github.com/a/b.git"
        );
        // A token is often the user name alone.
        assert_eq!(
            display_url("https://ghp_secret@github.com/a/b.git"),
            "https://***@github.com/a/b.git"
        );
        assert_eq!(
            display_url("https://github.com/a/b.git"),
            "https://github.com/a/b.git"
        );
        assert_eq!(
            display_url("https://bücher.example/r.git"),
            "https://xn--bcher-kva.example/r.git"
        );
        assert_eq!(
            display_url("git@github.com:a/b.git"),
            "git@github.com:a/b.git"
        );
    }

    #[test]
    fn user_names_and_passwords_are_masked_in_urls_as_written() {
        // One URL: everything before the last '@' of the authority, as git reads it; the
        // rest stays byte for byte.
        for (url, shown) in [
            (
                "https://user:ghp_x@GitHub.com/a/b.git",
                "https://***@GitHub.com/a/b.git",
            ),
            (
                "https://ghp_x@github.com/a/b.git",
                "https://***@github.com/a/b.git",
            ),
            ("https://a.com\\@b.com/x", "https://***@b.com/x"),
            ("https://u:p@ss@host:8443", "https://***@host:8443"),
            ("ssh://git@host:22/x", "ssh://***@host:22/x"),
            ("https://github.com/a@b.git", "https://github.com/a@b.git"),
            ("https://github.com/a/b.git", "https://github.com/a/b.git"),
            ("git@github.com:a/b.git", "git@github.com:a/b.git"),
            ("https://us er:tok@host/r", "https://***@host/r"),
            // The authority ends at the first '/', '?' or '#', as git and curl read it, so
            // the card never shows a host other than the one git connects to.
            (
                "https://evil.example?@github.com/r.git",
                "https://evil.example?@github.com/r.git",
            ),
            (
                "https://evil.example#@github.com/r.git",
                "https://evil.example#@github.com/r.git",
            ),
            (
                "https://u:t@evil.example#@github.com/x",
                "https://***@evil.example#@github.com/x",
            ),
        ] {
            assert_eq!(mask_userinfo(url), shown, "{url}");
        }
        // Free text such as an error message from git: every URL in it.
        let text = "fatal: unable to access 'https://user:ghp_1@host.example/r.git/': 403\n\
                    To https://ghp_2@mirror.example/r.git\n\
                    mail me@example.com, see https://example.com/a@b";
        let hidden = hide_credentials(text);
        assert!(!hidden.contains("ghp_"), "{hidden}");
        assert_eq!(
            hide_credentials("to https://evil.example?@github.com/r and https://u:t@h.example#@x"),
            "to https://evil.example?@github.com/r and https://***@h.example#@x"
        );
        assert!(
            hidden.contains("'https://***@host.example/r.git/'"),
            "{hidden}"
        );
        assert!(
            hidden.contains("To https://***@mirror.example/r.git"),
            "{hidden}"
        );
        assert!(
            hidden.contains("me@example.com, see https://example.com/a@b"),
            "{hidden}"
        );
    }

    #[test]
    fn gitattributes_filters() {
        assert!(gitattributes_names_filter(
            "*.psd filter=lfs diff=lfs merge=lfs -text\n"
        ));
        assert!(gitattributes_names_filter(
            "# c\nsecret/** filter=git-crypt diff=git-crypt"
        ));
        assert!(!gitattributes_names_filter(
            "*.txt text eol=lf\n# *.bin filter=lfs\n"
        ));
        assert!(!gitattributes_names_filter("*.bin -filter\n*.x !filter\n"));
    }

    #[test]
    fn pathspecs_are_relative_to_the_root() {
        let root = if cfg!(windows) {
            PathBuf::from("C:\\r")
        } else {
            PathBuf::from("/r")
        };
        let abs = root.join("src").join("a.rs");
        assert_eq!(pathspec_for(&root, &abs.to_string_lossy()), "src/a.rs");
        assert_eq!(pathspec_for(&root, "src/a.rs"), "src/a.rs");
        assert_eq!(pathspec_for(&root, "./src/a.rs"), "src/a.rs");
        assert_eq!(pathspec_for(&root, "."), "*");
    }
}
