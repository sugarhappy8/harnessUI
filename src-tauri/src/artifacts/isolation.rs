//! Scripts run in previews only after the sandbox has been checked on this system.
//!
//! What keeps a preview from calling the app differs by webview, and only one of the
//! three can be tried where the app is developed:
//!
//! - WebKitGTK and WKWebView give the app's script interface to the main frame only. A
//!   frame can post to the same message channel, but every message needs a key that only
//!   the main frame's interface holds.
//! - WebView2 gives the interface, key included, to every frame. There a call is refused
//!   because the frame's address is not the app's own (see `protocol.rs`).
//!
//! So the app does not assume. When it starts, the main window loads a page of the app's
//! own into a hidden, sandboxed frame from the same address previews use (`probe.js`).
//! That page tries to call the app with whatever interface it finds, tries to make
//! requests its rules forbid, and looks for storage and for the app's page. The call and
//! the requests are tripwires: [`Gate::tripwire`] is reached only if the sandbox does
//! not hold. Only a check that ends with no tripwire touched and nothing found turns
//! scripts on; until then, and after a failed check, previews are served with scripts
//! off (`Scripts::Off`), where nothing the model wrote runs at all.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// What is known about the sandbox on this system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Isolation {
    /// Not checked yet. Previews show without scripts.
    Unchecked,
    /// A check is running.
    Checking,
    /// The check passed. Previews run their scripts.
    Proven,
    /// The check failed, for this reason. Previews show without scripts.
    Broken { reason: String },
}

/// What the check's page says it saw. It is the app's own page, but it runs where a
/// hostile page would, so a missing or odd report counts as a failed check.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ProbeReport {
    pub origin: String,
    pub storage: String,
    pub parent: String,
    pub cookie: String,
}

const CALLED: &str = "A page in the preview was able to call the app.";
const REQUESTED: &str = "A page in the preview was able to make a request.";
const NOT_APART: &str = "A page in the preview was not kept apart from the app.";
const STORAGE: &str = "A page in the preview was able to store data.";
const UNFINISHED: &str = "The check of the preview did not finish.";

/// A tripwire a preview page touched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tripwire {
    /// It called a command of the app.
    Command,
    /// It made a request its rules forbid.
    Request,
}

#[derive(Debug, Default)]
struct Inner {
    state: Option<Isolation>,
    /// The check in progress.
    nonce: Option<String>,
    /// A tripwire touched during it.
    touched: Option<Tripwire>,
}

/// The state of the check. One for the app.
#[derive(Debug, Default)]
pub struct Gate(Mutex<Inner>);

impl Gate {
    fn with<T>(&self, f: impl FnOnce(&mut Inner) -> T) -> T {
        // A poisoned lock means a panic mid-update: fall back to "not proven".
        let mut inner = self.0.lock().unwrap_or_else(|poisoned| {
            let mut inner = poisoned.into_inner();
            *inner = Inner {
                state: Some(Isolation::Broken {
                    reason: UNFINISHED.into(),
                }),
                ..Inner::default()
            };
            inner
        });
        f(&mut inner)
    }

    /// What is known now.
    pub fn status(&self) -> Isolation {
        self.with(|inner| inner.state.clone().unwrap_or(Isolation::Unchecked))
    }

    /// Whether previews may run scripts: only after a check that passed.
    pub fn scripts_allowed(&self) -> bool {
        self.status() == Isolation::Proven
    }

    /// Starts a check and returns its nonce. Scripts are off until it has passed.
    pub fn begin(&self) -> String {
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        self.with(|inner| {
            inner.state = Some(Isolation::Checking);
            inner.nonce = Some(nonce.clone());
            inner.touched = None;
        });
        nonce
    }

    /// Whether `nonce` is the check in progress (its page is served for no other).
    pub fn is_current(&self, nonce: &str) -> bool {
        self.with(|inner| inner.nonce.as_deref() == Some(nonce))
    }

    /// A preview page reached the app. During a check it fails the check; at any other
    /// time it turns scripts off at once.
    pub fn tripwire(&self, what: Tripwire) {
        self.with(|inner| {
            if inner.nonce.is_some() {
                inner.touched.get_or_insert(what);
            } else {
                inner.state = Some(Isolation::Broken {
                    reason: reason_for(what).into(),
                });
            }
        });
    }

    /// Ends the check `nonce` with what its page reported (`None`: it never reported).
    /// A stale nonce changes nothing.
    pub fn finish(&self, nonce: &str, report: Option<&ProbeReport>) -> Isolation {
        self.with(|inner| {
            if inner.nonce.as_deref() != Some(nonce) {
                return inner.state.clone().unwrap_or(Isolation::Unchecked);
            }
            let verdict = match (inner.touched.take(), report) {
                (Some(what), _) => Err(reason_for(what)),
                (None, None) => Err(UNFINISHED),
                (None, Some(report)) => judge(report),
            };
            inner.nonce = None;
            let state = match verdict {
                Ok(()) => Isolation::Proven,
                Err(reason) => Isolation::Broken {
                    reason: reason.into(),
                },
            };
            inner.state = Some(state.clone());
            state
        })
    }
}

fn reason_for(what: Tripwire) -> &'static str {
    match what {
        Tripwire::Command => CALLED,
        Tripwire::Request => REQUESTED,
    }
}

/// A sandboxed page has the origin `null`, no storage, no cookies and no view of the
/// page around it. Anything else, or a field left out, is a failed check.
fn judge(report: &ProbeReport) -> Result<(), &'static str> {
    if report.origin != "null" || report.parent != "blocked" {
        return Err(NOT_APART);
    }
    if report.storage != "blocked" || report.cookie != "blocked" {
        return Err(STORAGE);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean() -> ProbeReport {
        ProbeReport {
            origin: "null".into(),
            storage: "blocked".into(),
            parent: "blocked".into(),
            cookie: "blocked".into(),
        }
    }

    fn broken(reason: &str) -> Isolation {
        Isolation::Broken {
            reason: reason.into(),
        }
    }

    #[test]
    fn scripts_are_off_until_a_check_has_passed() {
        let gate = Gate::default();
        assert_eq!(gate.status(), Isolation::Unchecked);
        assert!(!gate.scripts_allowed());
        let nonce = gate.begin();
        assert_eq!(gate.status(), Isolation::Checking);
        assert!(
            !gate.scripts_allowed(),
            "scripts ran while the check was open"
        );
        assert!(gate.is_current(&nonce) && !gate.is_current("other"));
        assert_eq!(gate.finish(&nonce, Some(&clean())), Isolation::Proven);
        assert!(gate.scripts_allowed());
        assert!(!gate.is_current(&nonce), "a finished check stays open");
    }

    #[test]
    fn a_tripwire_during_the_check_fails_it_whatever_the_page_reports() {
        for (what, reason) in [(Tripwire::Command, CALLED), (Tripwire::Request, REQUESTED)] {
            let gate = Gate::default();
            let nonce = gate.begin();
            gate.tripwire(what);
            assert_eq!(gate.status(), Isolation::Checking);
            assert_eq!(gate.finish(&nonce, Some(&clean())), broken(reason));
            assert!(!gate.scripts_allowed());
        }
        // The first one touched is the reason given.
        let gate = Gate::default();
        let nonce = gate.begin();
        gate.tripwire(Tripwire::Request);
        gate.tripwire(Tripwire::Command);
        assert_eq!(gate.finish(&nonce, None), broken(REQUESTED));
    }

    #[test]
    fn a_tripwire_after_a_passed_check_turns_scripts_off_at_once() {
        let gate = Gate::default();
        let nonce = gate.begin();
        gate.finish(&nonce, Some(&clean()));
        assert!(gate.scripts_allowed());
        gate.tripwire(Tripwire::Command);
        assert_eq!(gate.status(), broken(CALLED));
        assert!(!gate.scripts_allowed());
        // And before any check too.
        let gate = Gate::default();
        gate.tripwire(Tripwire::Request);
        assert_eq!(gate.status(), broken(REQUESTED));
    }

    #[test]
    fn a_check_that_never_reports_or_reports_oddly_fails() {
        let gate = Gate::default();
        let nonce = gate.begin();
        assert_eq!(gate.finish(&nonce, None), broken(UNFINISHED));

        type Change = fn(&mut ProbeReport);
        let cases: [(Change, &str); 6] = [
            (
                |r| r.origin = "artifact://sandbox.localhost".into(),
                NOT_APART,
            ),
            (|r| r.origin = String::new(), NOT_APART),
            (|r| r.parent = "reachable".into(), NOT_APART),
            (|r| r.storage = "available".into(), STORAGE),
            (|r| r.cookie = "available".into(), STORAGE),
            (|r| r.storage = String::new(), STORAGE),
        ];
        for (change, reason) in cases {
            let gate = Gate::default();
            let nonce = gate.begin();
            let mut report = clean();
            change(&mut report);
            assert_eq!(
                gate.finish(&nonce, Some(&report)),
                broken(reason),
                "{report:?}"
            );
        }
        // A report with fields missing reads as empty fields.
        let partial: ProbeReport = serde_json::from_str(r#"{"origin":"null"}"#).unwrap();
        let gate = Gate::default();
        let nonce = gate.begin();
        assert_eq!(gate.finish(&nonce, Some(&partial)), broken(NOT_APART));
    }

    #[test]
    fn a_stale_or_made_up_nonce_decides_nothing() {
        let gate = Gate::default();
        assert_eq!(gate.finish("made-up", Some(&clean())), Isolation::Unchecked);
        let first = gate.begin();
        let second = gate.begin();
        assert_eq!(gate.finish(&first, Some(&clean())), Isolation::Checking);
        assert!(!gate.scripts_allowed());
        assert_eq!(gate.finish(&second, Some(&clean())), Isolation::Proven);
        // Finishing again does not change the verdict.
        assert_eq!(gate.finish(&second, None), Isolation::Proven);
    }

    #[test]
    fn a_failed_check_can_be_run_again() {
        let gate = Gate::default();
        gate.tripwire(Tripwire::Command);
        assert!(matches!(gate.status(), Isolation::Broken { .. }));
        let nonce = gate.begin();
        assert_eq!(gate.finish(&nonce, Some(&clean())), Isolation::Proven);
        // A new check starts from "not proven", and a tripwire from the old one is gone.
        let nonce = gate.begin();
        assert!(!gate.scripts_allowed());
        assert_eq!(gate.finish(&nonce, Some(&clean())), Isolation::Proven);
    }

    #[test]
    fn the_state_goes_to_the_window_as_plain_names() {
        let json = |i: &Isolation| serde_json::to_value(i).unwrap();
        assert_eq!(
            json(&Isolation::Proven),
            serde_json::json!({"state": "proven"})
        );
        assert_eq!(
            json(&Isolation::Unchecked),
            serde_json::json!({"state": "unchecked"})
        );
        assert_eq!(
            json(&broken("why")),
            serde_json::json!({"state": "broken", "reason": "why"})
        );
    }
}
