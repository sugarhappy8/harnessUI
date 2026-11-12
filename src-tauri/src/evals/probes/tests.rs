//! The probes' verifications on the scripted endpoint: a model that does the probe right
//! passes it, one that does it wrong fails it.

use serde_json::{Value, json};

use super::*;
use crate::evals::testing::{Endpoint, Say, audit_rows, call, model, task_of, text, tool_results};

/// A model that does every probe right. The recovery probe's first edit names only the
/// ambiguous text, as a model would, and is fixed after the tool's error.
fn capable(request: &Value) -> Say {
    let task = task_of(request);
    let results = tool_results(request);
    if task.contains("note.txt") {
        match results.last() {
            None => call("read_file", json!({"path": "note.txt"})),
            Some(result) => {
                let word = result
                    .split("The code word is ")
                    .nth(1)
                    .and_then(|rest| rest.split('.').next())
                    .unwrap_or("nothing")
                    .to_owned();
                text(&word)
            }
        }
    } else if task.contains("settings.txt") {
        match results.len() {
            0 => call(
                "edit_file",
                json!({"path": "settings.txt", "old_text": "color = blue", "new_text": "color = green"}),
            ),
            _ => text("Changed."),
        }
    } else if task.contains("limits.txt") {
        match results.len() {
            0 => call(
                "edit_file",
                json!({"path": "limits.txt", "old_text": "limit = 10", "new_text": "limit = 20"}),
            ),
            1 => call(
                "edit_file",
                json!({"path": "limits.txt", "old_text": "[server]\nlimit = 10", "new_text": "[server]\nlimit = 20"}),
            ),
            _ => text("Changed."),
        }
    } else {
        text("?")
    }
}

/// A model that calls the tools but gets every probe wrong.
fn careless(request: &Value) -> Say {
    let task = task_of(request);
    let results = tool_results(request);
    if !results.is_empty() {
        return text("pelican");
    }
    if task.contains("note.txt") {
        call("read_file", json!({"path": "note.txt"}))
    } else if task.contains("settings.txt") {
        call(
            "edit_file",
            json!({"path": "settings.txt", "old_text": "size = 3", "new_text": "size = 4"}),
        )
    } else {
        call(
            "edit_file",
            json!({"path": "limits.txt", "old_text": "limit = 10", "new_text": "limit = 20", "occurrence": 1}),
        )
    }
}

async fn probe_with(model_fn: fn(&Value) -> Say, probe: Probe) -> ProbeResult {
    let endpoint = Endpoint::start(model_fn).await;
    let (audit, _) = audit_rows();
    run_probe(
        &model(&endpoint),
        probe,
        Settings::default(),
        &[],
        &CancellationToken::new(),
        &audit,
    )
    .await
}

#[tokio::test]
async fn a_capable_model_passes_every_probe() {
    for probe in PROBES {
        let result = probe_with(capable, probe).await;
        assert!(result.passed, "{probe:?}: {result:?}");
        assert_eq!(result.arrival, Arrival::Native);
        assert_eq!(result.blocked, 0);
    }
    let recovery = probe_with(capable, Probe::ErrorRecovery).await;
    assert_eq!(recovery.failed_edits, 1, "{recovery:?}");
    assert!(recovery.note.contains("Recovered"), "{recovery:?}");
}

#[tokio::test]
async fn a_careless_model_fails_every_probe() {
    for probe in PROBES {
        let result = probe_with(careless, probe).await;
        assert!(!result.passed, "{probe:?}: {result:?}");
    }
}

#[tokio::test]
async fn a_model_that_never_calls_a_tool_fails() {
    fn talker(_: &Value) -> Say {
        text("The code word is heron and I changed the files.")
    }
    for probe in PROBES {
        let result = probe_with(talker, probe).await;
        assert!(!result.passed, "{probe:?}: {result:?}");
        assert_eq!(result.arrival, Arrival::None);
    }
}

/// The edit probes need `edit_file`: rewriting the whole file is not what they measure.
#[tokio::test]
async fn rewriting_the_file_does_not_pass_an_edit_probe() {
    fn rewriter(request: &Value) -> Say {
        if tool_results(request).is_empty() {
            call(
                "write_file",
                json!({"path": "settings.txt", "content": EDIT_AFTER}),
            )
        } else {
            text("Done.")
        }
    }
    let result = probe_with(rewriter, Probe::Editing).await;
    assert!(!result.passed, "{result:?}");
    assert!(result.note.contains("not through edit_file"), "{result:?}");
}

/// A model that writes its calls as text is recorded as such and can still pass.
#[tokio::test]
async fn calls_written_as_text_are_reported() {
    fn texter(request: &Value) -> Say {
        match capable(request) {
            Say::Calls(calls) => {
                let (name, args) = calls.into_iter().next().unwrap();
                Say::TextCall(name, args)
            }
            other => other,
        }
    }
    let result = probe_with(texter, Probe::Editing).await;
    assert!(result.passed, "{result:?}");
    assert_eq!(result.arrival, Arrival::Text);
}

#[test]
fn variants_follow_what_the_model_can_do() {
    let labels =
        |can_think| -> Vec<String> { variants(can_think).into_iter().map(|(l, _)| l).collect() };
    assert_eq!(labels(false), ["Default", "Temperature 0.2"]);
    assert_eq!(
        labels(true),
        [
            "Default",
            "Thinking off",
            "Temperature 0.2",
            "Thinking off, temperature 0.2"
        ]
    );
    assert_eq!(variants(true)[0].1, Settings::default());
}

fn variant(label: &str, passed: u32, tool_failures: u32) -> VariantResult {
    VariantResult {
        label: label.into(),
        settings: Settings::default(),
        passed,
        tool_failures,
        probes: Vec::new(),
    }
}

#[test]
fn the_best_variant_passes_most_then_fails_least_then_comes_first() {
    let results = [
        variant("a", 2, 1),
        variant("b", 3, 2),
        variant("c", 3, 0),
        variant("d", 3, 0),
    ];
    assert_eq!(best(&results).unwrap().label, "c");
    let tie = [variant("a", 2, 0), variant("b", 2, 0)];
    assert_eq!(best(&tie).unwrap().label, "a");
    assert!(best(&[variant("a", 0, 0), variant("b", 0, 0)]).is_none());
    assert!(best(&[]).is_none());
}

#[test]
fn a_chosen_variant_becomes_request_defaults() {
    assert_eq!(defaults_of(Settings::default()), Default::default());
    let d = defaults_of(Settings {
        thinking_off: true,
        temperature: Some(0.2),
    });
    assert_eq!(d.thinking, Some(false));
    assert_eq!(d.temperature, Some(0.2));
}

/// The whole check, with a model that does the probes right only at a low temperature:
/// that variant is the best one.
#[tokio::test]
async fn the_check_finds_the_settings_that_work() {
    fn cool_only(request: &Value) -> Say {
        let cool = request["temperature"]
            .as_f64()
            .is_some_and(|t| (t - 0.2).abs() < 1e-6);
        if cool {
            capable(request)
        } else {
            careless(request)
        }
    }
    let endpoint = Endpoint::start(cool_only).await;
    let (audit, rows) = audit_rows();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let kept = seen.clone();
    let results = check(
        &model(&endpoint),
        &variants(false),
        &[],
        CancellationToken::new(),
        audit,
        std::sync::Arc::new(move |done, total, what| {
            kept.lock().unwrap().push((done, total, what))
        }),
    )
    .await;
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].passed, 0, "{:?}", results[0]);
    assert_eq!(results[1].passed, 3, "{:?}", results[1]);
    assert_eq!(best(&results).unwrap().label, "Temperature 0.2");
    let seen = seen.lock().unwrap();
    assert_eq!(seen.first().unwrap().0, 0);
    assert_eq!(seen.last().unwrap().0, 6);
    assert!(seen.iter().all(|(_, total, _)| *total == 6));
    assert!(!rows.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_cancelled_check_stops_before_the_next_probe() {
    let endpoint = Endpoint::start(capable).await;
    let (audit, _) = audit_rows();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let results = check(
        &model(&endpoint),
        &variants(true),
        &[],
        cancel,
        audit,
        std::sync::Arc::new(|_, _, _| {}),
    )
    .await;
    assert!(results.is_empty());
    assert!(endpoint.requests().is_empty());
}
