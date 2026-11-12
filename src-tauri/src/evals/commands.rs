//! The Tauri commands of the capability check and the benchmarks (main window only).
//!
//! A run starts only from a command the user's click sends, after the confirmation in
//! Settings. One run at a time; `cancel_eval` stops it within a second or two. Each run
//! writes one audit row of its own (decided by the user, no chat) and one row per tool
//! call that points at it, so the audit log shows what ran on the computer. Progress goes
//! to the main window as `eval-progress` events. Results stay on the computer.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio_util::sync::CancellationToken;

use super::bench::{self, BenchRun, BenchSummary};
use super::harness::{AuditFn, Model, Settings};
use super::probes;
use super::profile::{self, Identity, Profile};
use super::tasks::{self, Toolchain};
use super::verify::Toolchains;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// The event that carries a run's progress.
pub const PROGRESS_EVENT: &str = "eval-progress";

/// The run in progress, if any.
static RUNNING: Mutex<Option<CancellationToken>> = Mutex::new(None);

/// Clears [`RUNNING`] when a run ends, however it ends.
struct Running;

impl Drop for Running {
    fn drop(&mut self) {
        if let Ok(mut running) = RUNNING.lock() {
            *running = None;
        }
    }
}

fn start_run() -> AppResult<(Running, CancellationToken)> {
    let mut running = RUNNING
        .lock()
        .map_err(|_| AppError::internal("the run state is unavailable"))?;
    if running.is_some() {
        return Err(AppError::new(
            "eval_busy",
            "A check or benchmark is already running.",
            "Wait for it to finish or stop it.",
            String::new(),
        ));
    }
    let token = CancellationToken::new();
    *running = Some(token.clone());
    Ok((Running, token))
}

fn only_main_window(label: &str) -> AppResult<()> {
    if label == crate::MAIN_WINDOW {
        Ok(())
    } else {
        Err(AppError::bad_request(
            "Only the main window can start or stop a check.",
        ))
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    kind: &'static str,
    done: u32,
    total: u32,
    current: String,
}

fn progress_fn(app: &AppHandle, kind: &'static str) -> Arc<dyn Fn(u32, u32, String) + Send + Sync> {
    let app = app.clone();
    Arc::new(move |done, total, current| {
        let _ = app.emit_to(
            crate::MAIN_WINDOW,
            PROGRESS_EVENT,
            Progress {
                kind,
                done,
                total,
                current,
            },
        );
    })
}

// ---- The model a run talks to ----------------------------------------------------

/// The model behind a model reference, as a run talks to it: the loaded local model, or
/// a model of a configured endpoint.
async fn model_for(state: &AppState, model_ref: &str) -> AppResult<(Model, Identity)> {
    let not_loaded = || {
        AppError::new(
            "model_not_loaded",
            "This model is not loaded.",
            "Load it from the list at the top of a chat, then check it here.",
            model_ref.to_owned(),
        )
    };
    if let Some(path) = model_ref.strip_prefix("local:") {
        let (base_url, key, loaded, n_ctx) =
            state.runtime.endpoint().await.ok_or_else(not_loaded)?;
        if loaded.to_string_lossy() != path {
            return Err(not_loaded());
        }
        let identity = profile::identity_for(state, model_ref)
            .await
            .ok_or_else(not_loaded)?;
        let client = crate::runtime_manager::local_client(base_url, key)?;
        let status = state.runtime.status();
        let caps = agent::ModelCaps {
            tools: status.supports_tools,
            vision: status.vision,
            n_ctx,
            local: true,
            thinking: status.thinking,
            preserve_reasoning: status.preserve_reasoning,
            images_in_tool_results: false,
        };
        let mut template = agent::RequestTemplate {
            model: "local".into(),
            ..Default::default()
        };
        template.extra.insert("cache_prompt".into(), json!(true));
        let sanitizer = agent::Sanitizer::for_endpoint(&client).await;
        return Ok((
            Model {
                client,
                caps,
                template,
                sanitizer,
            },
            identity,
        ));
    }
    let malformed = || AppError::bad_request("This model reference is not recognised.");
    let rest = model_ref.strip_prefix("remote:").ok_or_else(malformed)?;
    let (id, model) = rest.split_once(':').ok_or_else(malformed)?;
    let id: i64 = id.parse().map_err(|_| malformed())?;
    let (client, _) = crate::providers::client_for(state, store::ProviderId(id)).await?;
    let identity = profile::identity_for(state, model_ref)
        .await
        .ok_or_else(malformed)?;
    let n_ctx = match client.context_length(model).await.ok().flatten() {
        Some(n) => n,
        None => {
            let stored: Option<serde_json::Value> = state
                .store
                .get_setting(crate::settings::REMOTE_CONTEXT_LENGTH)
                .await
                .ok()
                .flatten();
            crate::settings::remote_context_length(stored.as_ref())
        }
    };
    let caps = agent::ModelCaps {
        tools: true,
        vision: true,
        n_ctx,
        local: false,
        thinking: false,
        preserve_reasoning: false,
        images_in_tool_results: true,
    };
    let template = agent::RequestTemplate {
        model: model.to_owned(),
        ..Default::default()
    };
    let sanitizer = agent::Sanitizer::for_endpoint(&client).await;
    Ok((
        Model {
            client,
            caps,
            template,
            sanitizer,
        },
        identity,
    ))
}

/// Where the model's requests go, for the run's audit row.
fn where_it_goes(identity: &Identity) -> String {
    match identity {
        Identity::Local { .. } => "The model runs on this computer.".into(),
        Identity::Remote { url, .. } => format!(
            "The tasks and the model's replies go to {}.",
            url::Url::parse(url)
                .ok()
                .and_then(|u| u.host_str().map(str::to_owned))
                .unwrap_or_else(|| url.clone())
        ),
    }
}

// ---- Audit ------------------------------------------------------------------------

/// The run's own audit row and a writer for the rows of its calls, which point at it.
struct RunAudit {
    store: store::Store,
    row: Option<store::AuditId>,
    sender: tokio::sync::mpsc::UnboundedSender<store::AuditEntry>,
    writer: tokio::task::JoinHandle<()>,
    started: Instant,
}

impl RunAudit {
    async fn open(
        state: &AppState,
        tool: &str,
        mark: &'static str,
        arguments: serde_json::Value,
        preview: String,
    ) -> Self {
        let store = state.store.clone();
        let row = store
            .append_audit(store::AuditEntry {
                ts: None,
                chat_id: None,
                server: "evals".into(),
                tool: tool.into(),
                arguments,
                preview,
                risk: "normal".into(),
                decision: store::AuditDecision::AllowOnce,
                decided_by: "user".into(),
                note: None,
                parent_audit_id: None,
                result: None,
            })
            .await
            .map_err(|e| tracing::error!("could not write the audit log: {e}"))
            .ok();
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<store::AuditEntry>();
        let writer_store = store.clone();
        let writer = tokio::spawn(async move {
            while let Some(mut entry) = receiver.recv().await {
                entry.chat_id = None;
                entry.parent_audit_id = row;
                entry.preview = format!("{mark}{}", entry.preview)
                    .chars()
                    .take(20_000)
                    .collect();
                if let Err(e) = writer_store.append_audit(entry).await {
                    tracing::error!("could not write the audit log: {e}");
                }
            }
        });
        Self {
            store,
            row,
            sender,
            writer,
            started: Instant::now(),
        }
    }

    fn calls(&self) -> AuditFn {
        let sender = self.sender.clone();
        Arc::new(move |entry| {
            let _ = sender.send(entry);
        })
    }

    async fn close(self, status: &str, summary: String) {
        let RunAudit {
            store,
            row,
            sender,
            writer,
            started,
        } = self;
        drop(sender);
        let _ = writer.await;
        if let Some(row) = row {
            let _ = store
                .complete_audit(
                    row,
                    status.into(),
                    Some(summary),
                    Some(started.elapsed().as_millis() as u64),
                )
                .await;
        }
    }
}

// ---- Profiles ---------------------------------------------------------------------

/// A model's profile as Settings shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileView {
    /// The model can be checked now (a local model must be the loaded one).
    pub available: bool,
    /// Why it cannot, in words.
    pub reason: Option<String>,
    /// The model and backend, as named in the table.
    pub label: Option<String>,
    /// The settings key of its profile, for "Forget".
    pub key: Option<String>,
    pub profile: Option<Profile>,
}

/// The saved profile of a model, and whether it can be checked now.
#[tauri::command]
pub async fn get_model_profile(
    state: State<'_, AppState>,
    model_ref: String,
) -> AppResult<ProfileView> {
    let Some(identity) = profile::identity_for(&state, &model_ref).await else {
        return Ok(ProfileView {
            available: false,
            reason: Some(if model_ref.starts_with("local:") {
                "Load this model to check it.".into()
            } else {
                "This model's endpoint is not set up.".into()
            }),
            label: None,
            key: None,
            profile: None,
        });
    };
    Ok(ProfileView {
        available: true,
        reason: None,
        label: Some(identity.label()),
        key: Some(identity.key()),
        profile: profile::load(&state.store, &identity).await,
    })
}

/// Removes a saved profile. Only profile keys are accepted.
#[tauri::command]
pub async fn forget_model_profile(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    key: String,
) -> AppResult<bool> {
    only_main_window(window.label())?;
    if !is_profile_key(&key) {
        return Err(AppError::bad_request("That is not a saved model check."));
    }
    Ok(state.store.delete_setting(&key).await?)
}

/// `modelProfile:` and 16 lower-case hex digits.
fn is_profile_key(key: &str) -> bool {
    key.strip_prefix(profile::KEY_PREFIX).is_some_and(|hash| {
        hash.len() == 16
            && hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// Runs the capability check on a model and saves the best settings as its profile.
#[tauri::command]
pub async fn start_capability_check(
    app: AppHandle,
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    model_ref: String,
) -> AppResult<Profile> {
    only_main_window(window.label())?;
    let (model, identity) = model_for(&state, &model_ref).await?;
    let (_running, cancel) = start_run()?;
    let left = super::workspace::remove_leftovers();
    if left > 0 {
        tracing::info!(left, "removed folders left by earlier runs");
    }
    let label = identity.label();
    let audit = RunAudit::open(
        &state,
        "capability_check",
        "Model check: ",
        json!({"modelRef": model_ref}),
        format!(
            "Model check of {label}: the model gets only the file tools, in a temporary folder made for the check, and the app runs nothing the model writes. Nothing is asked while it runs. {}",
            where_it_goes(&identity)
        ),
    )
    .await;
    let variants = probes::variants(model.caps.thinking);
    let forbidden = crate::permissions::forbidden_for_tools(&app);
    let results = probes::check(
        &model,
        &variants,
        &forbidden,
        cancel.clone(),
        audit.calls(),
        progress_fn(&app, "check"),
    )
    .await;
    if cancel.is_cancelled() {
        audit
            .close("cancelled", "Stopped by the user.".into())
            .await;
        return Err(AppError::new(
            "eval_stopped",
            "The check was stopped.",
            "Start it again when you are ready.",
            String::new(),
        ));
    }
    let best = probes::best(&results).cloned();
    let best = best.as_ref();
    let saved = Profile {
        version: profile::VERSION,
        label: label.clone(),
        checked_at: chrono::Utc::now().timestamp_millis(),
        chosen: best.map(|b| b.label.clone()),
        defaults: best.map(|b| probes::defaults_of(b.settings)),
        results,
        identity: identity.clone(),
    };
    state.store.set_setting(&identity.key(), &saved).await?;
    let summary = match &saved.chosen {
        Some(chosen) => format!(
            "Chose \"{chosen}\": {} of 3 checks passed.",
            best.map(|b| b.passed).unwrap_or(0)
        ),
        None => "No setting passed a check; nothing will change for this model.".into(),
    };
    audit.close("ok", summary).await;
    tracing::info!(model = %label, chosen = ?saved.chosen, "a model check finished");
    Ok(saved)
}

/// Stops the check or benchmark in progress.
#[tauri::command]
pub fn cancel_eval(window: tauri::WebviewWindow) -> AppResult<bool> {
    only_main_window(window.label())?;
    let running = RUNNING
        .lock()
        .map_err(|_| AppError::internal("the run state is unavailable"))?;
    Ok(match running.as_ref() {
        Some(token) => {
            token.cancel();
            true
        }
        None => false,
    })
}

// ---- Benchmarks ---------------------------------------------------------------------

/// The terminal environment of the app's start-up, and the `PATH` in it.
fn terminal_and_path() -> (tools::TerminalConfig, Option<String>) {
    let captured = crate::toolset::captured_env_now();
    let mut config = crate::toolset::terminal_config(captured.as_ref(), Vec::new());
    config
        .base_env
        .insert("PYTHONDONTWRITEBYTECODE".into(), "1".into());
    let path = config
        .base_env
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("PATH"))
        .map(|(_, v)| v.clone());
    (config, path)
}

/// One task as the benchmark panel lists it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskInfo {
    pub name: String,
    pub title: String,
    pub kind: String,
    /// The toolchain it needs, by name.
    pub needs: Option<String>,
    /// False when that toolchain is not installed: the task will be skipped.
    pub runnable: bool,
}

/// The benchmark tasks, and which of them can run on this computer.
#[tauri::command]
pub async fn list_bench_tasks() -> AppResult<Vec<TaskInfo>> {
    let (_, path) = terminal_and_path();
    let toolchains = Toolchains::detect(path).await;
    Ok(tasks::all()
        .into_iter()
        .map(|t| TaskInfo {
            runnable: t.spec.needs.is_none_or(|n| toolchains.get(n).is_some()),
            needs: t.spec.needs.map(|n| n.name().to_owned()),
            name: t.name,
            title: t.spec.title,
            kind: t.spec.kind,
        })
        .collect())
}

fn bench_dir(app: &AppHandle) -> AppResult<std::path::PathBuf> {
    Ok(app
        .path()
        .app_local_data_dir()
        .map_err(|e| AppError::internal(e.to_string()))?
        .join("benchmarks"))
}

/// Runs every benchmark task on a model and saves the results.
#[tauri::command]
pub async fn start_benchmark(
    app: AppHandle,
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    model_ref: String,
) -> AppResult<BenchRun> {
    only_main_window(window.label())?;
    let (model, identity) = model_for(&state, &model_ref).await?;
    let dir = bench_dir(&app)?;
    let (_running, cancel) = start_run()?;
    let left = super::workspace::remove_leftovers();
    if left > 0 {
        tracing::info!(left, "removed folders left by earlier runs");
    }
    let label = identity.label();
    // The model's tested settings, when it has them.
    let settings = match profile::load(&state.store, &identity)
        .await
        .and_then(|p| p.defaults)
    {
        Some(d) => Settings {
            thinking_off: d.thinking == Some(false),
            temperature: d.temperature,
        },
        None => Settings::default(),
    };
    let (terminal, path) = terminal_and_path();
    let toolchains = Toolchains::detect(path).await;
    let audit = RunAudit::open(
        &state,
        "benchmark",
        "Benchmark: ",
        json!({"modelRef": model_ref}),
        format!(
            "Benchmark of {label}: the model writes code and the app runs it on this computer with the user's rights, in a temporary folder for each task (the task's tests, the model's scripts and the app's checks of the result). The app lets the model run only the task's tests and scripts and commands that look at files, and refuses everything else, but it cannot see inside code the model writes: such code could read or change files elsewhere or go online. Nothing is asked while it runs. {}",
            where_it_goes(&identity)
        ),
    )
    .await;
    let forbidden = crate::permissions::forbidden_for_tools(&app);
    let all = tasks::all();
    let started_at = chrono::Utc::now().timestamp_millis();
    let setup = bench::Setup {
        model: &model,
        settings,
        toolchains: &toolchains,
        terminal,
        forbidden: &forbidden,
        audit: audit.calls(),
    };
    let results = bench::run_all(&setup, &all, &cancel, progress_fn(&app, "bench")).await;
    // The setup holds a sender of the audit writer: drop it, or closing the audit below
    // would wait for the writer forever.
    drop(setup);
    let cancelled = cancel.is_cancelled();
    let run = BenchRun {
        id: bench::new_id(),
        started_at,
        finished_at: chrono::Utc::now().timestamp_millis(),
        model: label,
        model_ref,
        settings,
        toolchains: [Toolchain::Python, Toolchain::Node]
            .into_iter()
            .filter(|t| toolchains.get(*t).is_some())
            .map(|t| t.name().to_owned())
            .collect(),
        cancelled,
        totals: bench::totals(&results),
        tasks: results,
    };
    if let Err(e) = bench::save(&dir, &run) {
        tracing::error!("could not save the benchmark results: {e}");
    }
    audit
        .close(
            if cancelled { "cancelled" } else { "ok" },
            format!(
                "{} of {} tasks completed, {} skipped.",
                run.totals.completed, run.totals.ran, run.totals.skipped
            ),
        )
        .await;
    Ok(run)
}

/// The saved benchmark runs, newest first.
#[tauri::command]
pub async fn list_bench_runs(app: AppHandle) -> AppResult<Vec<BenchSummary>> {
    let dir = bench_dir(&app)?;
    Ok(tokio::task::spawn_blocking(move || bench::list(&dir))
        .await
        .unwrap_or_default())
}

/// One saved benchmark run.
#[tauri::command]
pub async fn get_bench_run(app: AppHandle, id: String) -> AppResult<BenchRun> {
    if !bench::valid_id(&id) {
        return Err(AppError::bad_request("That is not a saved benchmark run."));
    }
    let dir = bench_dir(&app)?;
    tokio::task::spawn_blocking(move || bench::load(&dir, &id))
        .await
        .ok()
        .flatten()
        .ok_or_else(|| AppError::not_found("benchmark run"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_profile_keys_can_be_forgotten() {
        let key = Identity::Remote {
            url: "http://h/v1".into(),
            model: "m".into(),
        }
        .key();
        assert!(is_profile_key(&key));
        for bad in [
            "theme",
            "modelProfile:",
            "modelProfile:../../x",
            "modelProfile:0123456789ABCDEF",
            "modelProfile:0123456789abcdef0",
            "stepsPerTurn",
        ] {
            assert!(!is_profile_key(bad), "{bad}");
        }
    }

    #[test]
    fn the_audit_says_where_requests_go() {
        let remote = Identity::Remote {
            url: "https://models.example.com:8443/v1".into(),
            model: "m".into(),
        };
        assert_eq!(
            where_it_goes(&remote),
            "The tasks and the model's replies go to models.example.com."
        );
        let local = Identity::Local {
            file: "m.gguf".into(),
            size: 1,
            backend: "cpu".into(),
        };
        assert_eq!(where_it_goes(&local), "The model runs on this computer.");
    }

    #[test]
    fn only_the_main_window_starts_a_run() {
        assert!(only_main_window(crate::MAIN_WINDOW).is_ok());
        assert!(only_main_window("overlay").is_err());
    }

    /// The run slot is one static: the tests that use it take turns.
    static SLOT_TESTS: Mutex<()> = Mutex::new(());

    /// A run that panics still releases the run slot.
    #[test]
    fn a_panicking_run_releases_the_slot() {
        let _turn = SLOT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let caught = std::panic::catch_unwind(|| {
            let (_guard, _token) = start_run().unwrap();
            panic!("the run failed");
        });
        assert!(caught.is_err());
        let (_again, _) = start_run().expect("the slot was released");
    }

    #[test]
    fn one_run_at_a_time() {
        let _turn = SLOT_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let (guard, token) = start_run().unwrap();
        assert!(start_run().is_err());
        token.cancel();
        drop(guard);
        let (_again, _) = start_run().unwrap();
    }
}
