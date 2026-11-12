//! The benchmark tasks: shipped data in `bench/tasks/<name>/`, embedded in the app when it
//! is built (`include_bytes!`), so the installed app needs no extra files and a task can
//! never come from anywhere else (there is no "import a task").
//!
//! A task folder holds:
//! - `task.json`: title, kind, the toolchain it needs (`python`, `node` or none), the
//!   prompt, the checks, and which file types get a parse check;
//! - `files/`: the small fixture project the model works on (copied into the run's folder);
//! - `verify/`: verification scripts, written to a separate folder after the model
//!   finished and run by the app, never visible to the model;
//! - `solution/` and `wrong/`: a correct and a plausible wrong answer, used only by the
//!   tests (not embedded in the app).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One task's files as `(path inside the task folder, content)`.
macro_rules! task {
    ($name:literal, [$($path:literal),* $(,)?]) => {
        (
            $name,
            &[$(($path, include_bytes!(concat!("../../../bench/tasks/", $name, "/", $path)).as_slice())),*],
        )
    };
}

/// Every shipped file of every task, embedded when the app is built.
pub static SHIPPED: &[(&str, &[(&str, &[u8])])] = &[
    task!(
        "py-fix-failing-test",
        [
            "task.json",
            "files/stats.py",
            "files/test_stats.py",
            "verify/verify.py",
        ]
    ),
    task!(
        "py-add-feature",
        [
            "task.json",
            "files/todo.py",
            "files/test_todo.py",
            "verify/verify.py",
        ]
    ),
    task!(
        "py-fix-from-error",
        [
            "task.json",
            "files/report.py",
            "files/orders.json",
            "verify/verify.py",
        ]
    ),
    task!(
        "py-rename-across-files",
        [
            "task.json",
            "files/users.py",
            "files/app.py",
            "files/test_app.py",
            "verify/verify.py",
        ]
    ),
    task!(
        "js-fix-failing-test",
        [
            "task.json",
            "files/slugify.js",
            "files/test.js",
            "verify/verify.cjs",
        ]
    ),
    task!(
        "js-add-feature",
        [
            "task.json",
            "files/cart.js",
            "files/cart.test.js",
            "verify/verify.cjs",
        ]
    ),
    task!(
        "js-fix-from-error",
        [
            "task.json",
            "files/names.js",
            "files/index.js",
            "files/users.json",
            "verify/verify.cjs",
        ]
    ),
    task!(
        "config-json-settings",
        ["task.json", "files/settings.json",]
    ),
    task!("config-ini-section", ["task.json", "files/app.ini",]),
    task!("docs-readme-install", ["task.json", "files/README.md",]),
    task!(
        "docs-changelog-release",
        ["task.json", "files/package.json", "files/CHANGELOG.md",]
    ),
];
/// A correct solution and, for some tasks, a plausible wrong one: files written over the
/// fixture by the scripted model in tests. Not part of the app.
#[cfg(test)]
pub static ANSWERS: &[(&str, &[(&str, &[u8])])] = &[
    task!(
        "py-fix-failing-test",
        ["solution/stats.py", "wrong/test_stats.py",]
    ),
    task!("py-add-feature", ["solution/todo.py",]),
    task!("py-fix-from-error", ["solution/report.py",]),
    task!(
        "py-rename-across-files",
        [
            "solution/users.py",
            "solution/app.py",
            "solution/test_app.py",
            "wrong/users.py",
        ]
    ),
    task!("js-fix-failing-test", ["solution/slugify.js",]),
    task!("js-add-feature", ["solution/cart.js",]),
    task!(
        "js-fix-from-error",
        ["solution/names.js", "wrong/names.js",]
    ),
    task!(
        "config-json-settings",
        ["solution/settings.json", "wrong/settings.json",]
    ),
    task!("config-ini-section", ["solution/app.ini", "wrong/app.ini",]),
    task!(
        "docs-readme-install",
        ["solution/README.md", "wrong/README.md",]
    ),
    task!(
        "docs-changelog-release",
        ["solution/package.json", "solution/CHANGELOG.md",]
    ),
];

/// The toolchain a task's verification runs with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Toolchain {
    Python,
    Node,
}

impl Toolchain {
    /// Program names to look for, in order.
    pub fn programs(self) -> &'static [&'static str] {
        match self {
            Toolchain::Python if cfg!(windows) => &["python", "python3"],
            Toolchain::Python => &["python3", "python"],
            Toolchain::Node => &["node"],
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Toolchain::Python => "Python",
            Toolchain::Node => "Node.js",
        }
    }
}

/// One check of a task's result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Check {
    /// Runs `verify/<script>` with the task's toolchain; passes on exit code 0 and a last
    /// line `VERIFIED <nonce>`, where the nonce is new for every run and reaches only the
    /// script (on its standard input).
    Run { script: String },
    /// The value at a JSON pointer of a JSON file.
    Json {
        file: String,
        pointer: String,
        equals: Value,
    },
    /// The value of `key` in `[section]` of an INI file.
    Ini {
        file: String,
        section: String,
        key: String,
        equals: String,
    },
    /// The texts appear in the file in this order.
    Order { file: String, texts: Vec<String> },
    /// No file with this extension contains the text.
    Absent { text: String, ext: String },
    /// The file is exactly as shipped.
    Unchanged { file: String },
}

/// A task as `task.json` describes it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSpec {
    pub title: String,
    /// `python`, `javascript`, `config` or `docs`.
    pub kind: String,
    pub needs: Option<Toolchain>,
    /// `{python}` and `{node}` are replaced by the program found on this computer.
    pub prompt: String,
    pub checks: Vec<Check>,
    /// File extensions whose files must still parse after the run (`py`, `js`, `json`).
    #[serde(default)]
    pub parse: Vec<String>,
}

/// A task, ready to run.
#[derive(Debug, Clone)]
pub struct Task {
    pub name: String,
    pub spec: TaskSpec,
    /// The fixture: paths relative to the run's folder.
    pub files: Vec<(String, &'static [u8])>,
    /// The verification files: paths relative to the verification folder.
    pub verify: Vec<(String, &'static [u8])>,
}

impl Task {
    /// The shipped content of a fixture file.
    pub fn fixture(&self, path: &str) -> Option<&'static [u8]> {
        self.files.iter().find(|(p, _)| p == path).map(|(_, c)| *c)
    }
}

fn under<'a>(files: &[(&'a str, &'static [u8])], folder: &str) -> Vec<(String, &'static [u8])> {
    files
        .iter()
        .filter_map(|(path, content)| {
            path.strip_prefix(folder)
                .and_then(|p| p.strip_prefix('/'))
                .map(|p| (p.to_owned(), *content))
        })
        .collect()
}

/// Every task, in the shipped order. A task whose `task.json` does not parse is left out
/// (a test makes sure none is).
pub fn all() -> Vec<Task> {
    SHIPPED
        .iter()
        .filter_map(|(name, files)| {
            let json = files.iter().find(|(p, _)| *p == "task.json")?.1;
            let spec: TaskSpec = match serde_json::from_slice(json) {
                Ok(spec) => spec,
                Err(e) => {
                    tracing::error!(task = name, "a benchmark task does not parse: {e}");
                    return None;
                }
            };
            Some(Task {
                name: (*name).to_owned(),
                spec,
                files: under(files, "files"),
                verify: under(files, "verify"),
            })
        })
        .collect()
}

/// A task's answer files from [`ANSWERS`] (`solution` or `wrong`).
#[cfg(test)]
pub fn answer(task: &str, kind: &str) -> Option<Vec<(String, &'static [u8])>> {
    let files = ANSWERS.iter().find(|(name, _)| *name == task)?.1;
    let picked = under(files, kind);
    (!picked.is_empty()).then_some(picked)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;

    use super::*;

    fn on_disk() -> BTreeSet<String> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../bench/tasks");
        let mut out = BTreeSet::new();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    let rel = path.strip_prefix(&root).unwrap();
                    let parts: Vec<String> = rel
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy().into_owned())
                        .collect();
                    out.insert(parts.join("/"));
                }
            }
        }
        out
    }

    /// The embedded listing names exactly the files in `bench/tasks`: a file added there
    /// must be listed, or it would silently not reach the app.
    #[test]
    fn the_listing_matches_the_folder() {
        let listed: BTreeSet<String> = SHIPPED
            .iter()
            .chain(ANSWERS)
            .flat_map(|(name, files)| files.iter().map(move |(p, _)| format!("{name}/{p}")))
            .collect();
        assert_eq!(listed, on_disk());
        // Answers are never shipped.
        assert!(
            SHIPPED
                .iter()
                .flat_map(|(_, files)| files.iter())
                .all(|(p, _)| !p.starts_with("solution/") && !p.starts_with("wrong/"))
        );
    }

    #[test]
    fn every_task_parses_and_the_mix_is_as_specified() {
        let tasks = all();
        assert_eq!(tasks.len(), SHIPPED.len());
        assert!(tasks.len() >= 10);
        let kinds: BTreeSet<&str> = tasks.iter().map(|t| t.spec.kind.as_str()).collect();
        assert_eq!(
            kinds,
            ["config", "docs", "javascript", "python"]
                .into_iter()
                .collect()
        );
        for task in &tasks {
            assert!(!task.files.is_empty(), "{}", task.name);
            assert!(!task.spec.checks.is_empty(), "{}", task.name);
            assert!(
                answer(&task.name, "solution").is_some(),
                "{} has no solution",
                task.name
            );
            // A script check has its script, and a toolchain to run it with.
            for check in &task.spec.checks {
                if let Check::Run { script } = check {
                    assert!(
                        task.verify.iter().any(|(p, _)| p == script),
                        "{}",
                        task.name
                    );
                    assert!(task.spec.needs.is_some(), "{}", task.name);
                }
                if let Check::Unchanged { file } = check {
                    assert!(task.fixture(file).is_some(), "{}", task.name);
                }
            }
            // The prompt names its program only through the placeholder.
            if let Some(needs) = task.spec.needs {
                let placeholder = match needs {
                    Toolchain::Python => "{python}",
                    Toolchain::Node => "{node}",
                };
                assert!(task.spec.prompt.contains(placeholder), "{}", task.name);
            }
            // Fixtures are tiny.
            let size: usize = task.files.iter().map(|(_, c)| c.len()).sum();
            assert!(size < 4_000, "{} is {size} bytes", task.name);
        }
    }
}
