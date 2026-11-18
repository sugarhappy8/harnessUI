//! Studio runs and the files they made (app 0.1.0).
//!
//! - A **run** is one press of Create: pictures or a video made by a company's computers
//!   through an endpoint, or by the Studio engine on this computer ([`RunsOn`]). The Studio
//!   manager picks its id (a uuid) and its creation time, as it does for each output.
//! - An **output** is one file a run made, in the gallery folder. Deleting a run deletes its
//!   outputs; deleting the last output of a finished run deletes the run.
//! - At most [`MAX_CREATION_RUNS`] runs are kept: one more is refused with
//!   [`StoreError::TooManyCreations`].
//! - **Spending stubs.** Deleting a result never lowers today's spending. A run deleted on
//!   the day it was made (created at or after the caller's `stub_since_ms`, the start of the
//!   local day) stays as a stub. The stub keeps what spending needs: the kind, where it ran,
//!   the endpoint, the model, the status, the remote id, the estimate, the cost, the error
//!   (which says whether the run was charged), the audit row and the times. It loses the
//!   prompt, the settings, the added pictures, `upstream`, the engine facts of a local run
//!   (installed model, device, time taken) and its outputs. Every read leaves stubs out
//!   except [`Store::creation_runs_since`], which spending reads, and
//!   [`Store::purge_creation_stubs`], which removes earlier days' stubs at start-up. Stubs do
//!   not count toward the cap.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Store;
use crate::error::StoreError;
use crate::ids::{
    AuditId, JsonColumn, ProviderId, json_text, like_contains, now_ms, sql_enum, sql_limit,
    sql_opt_text, sql_opt_u64, sql_u64, update_columns_by_text,
};

/// Studio keeps at most this many runs. Spending stubs are not counted.
pub const MAX_CREATION_RUNS: usize = 20_000;

sql_enum!(
    /// What a Studio run makes.
    CreationKind, "creation kind" {
        /// A picture.
        Image => "image",
        /// A video.
        Video => "video",
    }
);

sql_enum!(
    /// Where a Studio run is.
    CreationStatus, "creation status" {
        /// Waiting for its turn: for a free place in its endpoint's queue, or for the Studio
        /// engine and the graphics card.
        Queued => "queued",
        /// The request that creates it is being sent.
        Sending => "sending",
        /// Sent; the company is working on it (a video job being checked).
        Waiting => "waiting",
        /// A local run the engine is working on (cloud runs never use it).
        Running => "running",
        /// Its results are being downloaded.
        Downloading => "downloading",
        /// Finished, with its results.
        Done => "done",
        /// Ended with an error.
        Failed => "failed",
        /// Refused by the company's safety check.
        Blocked => "blocked",
        /// Stopped by the user.
        Cancelled => "cancelled",
        /// The app stopped while it was being sent, so whether it was made is not known.
        Interrupted => "interrupted",
    }
);

sql_enum!(
    /// Where a Studio run ran.
    RunsOn, "runs on" {
        /// A company's computers, through an endpoint.
        Cloud => "cloud",
        /// This computer, with the Studio engine.
        Local => "local",
    }
);

/// The statuses of a run still working, as an SQL list.
const LIVE_STATUSES: &str = "('queued', 'sending', 'waiting', 'running', 'downloading')";

/// The statuses of a run that ended without finishing, as an SQL list.
const UNFINISHED_STATUSES: &str = "('failed', 'blocked', 'cancelled', 'interrupted')";

/// Input for [`Store::insert_creation_run`]. A new run is [`CreationStatus::Queued`].
#[derive(Debug, Clone, PartialEq)]
pub struct NewCreationRun {
    /// A uuid picked by the caller.
    pub id: String,
    /// Pictures or a video.
    pub kind: CreationKind,
    /// Through an endpoint, or on this computer.
    pub runs_on: RunsOn,
    /// The endpoint; `None` for a run on this computer.
    pub provider_id: Option<ProviderId>,
    /// The endpoint's name ("This computer" for a local run), kept if the endpoint is deleted.
    pub provider_name: String,
    /// The endpoint's host (empty for a local run), kept if the endpoint is deleted.
    pub provider_host: String,
    /// The model's id.
    pub model: String,
    /// The model's name as shown.
    pub model_label: String,
    /// The prompt.
    pub prompt: String,
    /// Exactly what was sent (never a key).
    pub params: Value,
    /// The added pictures: a JSON array of `{sha256, role, …}`.
    pub refs: Value,
    /// The estimated cost in US dollars; `None` when the price is unknown.
    pub estimate_usd: Option<f64>,
    /// A local run: the installed model (its `gen_models` id).
    pub gen_model_id: Option<String>,
    /// A local run: the Studio engine's build number.
    pub engine_build: Option<u32>,
    /// A local run: the device it runs on, as shown.
    pub device: Option<String>,
    /// Unix ms, from the caller's clock.
    pub created_at: i64,
}

/// Input for [`Store::update_creation_run`]. `None` leaves a field unchanged; `Some(None)`
/// clears it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CreationPatch {
    /// New status.
    pub status: Option<CreationStatus>,
    /// The company's id for the request or job.
    pub remote_id: Option<Option<String>>,
    /// Progress from 0 to 1, when the company or the engine reports it.
    pub progress: Option<Option<f64>>,
    /// What the company charged, in US dollars.
    pub cost_usd: Option<Option<f64>>,
    /// The error, with whether the run was charged.
    pub error: Option<Option<Value>>,
    /// The run's audit row.
    pub audit_id: Option<Option<AuditId>>,
    /// The company OpenRouter ran it at.
    pub upstream: Option<Option<String>>,
    /// Unix ms when sending (or the engine) started.
    pub started_at: Option<Option<i64>>,
    /// Unix ms when it ended.
    pub finished_at: Option<Option<i64>>,
    /// A local run: how long the engine took.
    pub elapsed_ms: Option<Option<u64>>,
    /// A local run: the device it ran on, as shown.
    pub device: Option<Option<String>>,
}

/// A row of `creation_outputs`: one file a run made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreationOutput {
    /// A uuid picked by the caller.
    pub id: String,
    /// The run that made it.
    pub run_id: String,
    /// Its place among the run's outputs, from 0.
    pub idx: u32,
    /// Absolute path of the file in the gallery folder.
    pub path: String,
    /// Absolute path of its thumbnail, once made.
    pub thumb_path: Option<String>,
    /// MIME type read from the file's bytes.
    pub mime: String,
    /// File size.
    pub size_bytes: u64,
    /// Width in pixels, when known.
    pub width: Option<u32>,
    /// Height in pixels, when known.
    pub height: Option<u32>,
    /// A video's length.
    pub duration_ms: Option<u64>,
    /// The seed it was made with, when known.
    pub seed: Option<u64>,
    /// Starred by the user.
    pub starred: bool,
    /// Unix ms, from the caller's clock.
    pub created_at: i64,
}

/// Input for [`Store::insert_creation_output`]: a [`CreationOutput`] that is not starred.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCreationOutput {
    /// A uuid picked by the caller.
    pub id: String,
    /// The run that made it.
    pub run_id: String,
    /// Its place among the run's outputs, from 0.
    pub idx: u32,
    /// Absolute path of the file in the gallery folder.
    pub path: String,
    /// Absolute path of its thumbnail, if already made.
    pub thumb_path: Option<String>,
    /// MIME type read from the file's bytes.
    pub mime: String,
    /// File size.
    pub size_bytes: u64,
    /// Width in pixels, when known.
    pub width: Option<u32>,
    /// Height in pixels, when known.
    pub height: Option<u32>,
    /// A video's length.
    pub duration_ms: Option<u64>,
    /// The seed it was made with, when known.
    pub seed: Option<u64>,
    /// Unix ms, from the caller's clock.
    pub created_at: i64,
}

/// A row of `creation_runs`, with its outputs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreationRun {
    /// A uuid.
    pub id: String,
    /// Pictures or a video.
    pub kind: CreationKind,
    /// Through an endpoint, or on this computer.
    pub runs_on: RunsOn,
    /// The endpoint; `None` for a local run or once the endpoint is deleted.
    pub provider_id: Option<ProviderId>,
    /// The endpoint's name when the run was made ("This computer" for a local run).
    pub provider_name: String,
    /// The endpoint's host when the run was made (empty for a local run).
    pub provider_host: String,
    /// The model's id.
    pub model: String,
    /// The model's name as shown.
    pub model_label: String,
    /// The prompt (empty in a spending stub).
    pub prompt: String,
    /// Exactly what was sent (`{}` in a spending stub).
    pub params: Value,
    /// The added pictures (`[]` in a spending stub).
    pub refs: Value,
    /// Where it is.
    pub status: CreationStatus,
    /// The company's id for the request or job.
    pub remote_id: Option<String>,
    /// Progress from 0 to 1, when reported.
    pub progress: Option<f64>,
    /// The estimated cost in US dollars; `None` when the price was unknown.
    pub estimate_usd: Option<f64>,
    /// What the company charged, in US dollars.
    pub cost_usd: Option<f64>,
    /// The error, with whether the run was charged.
    pub error: Option<Value>,
    /// The run's audit row.
    pub audit_id: Option<AuditId>,
    /// The company OpenRouter ran it at, when known.
    pub upstream: Option<String>,
    /// A local run: the installed model.
    pub gen_model_id: Option<String>,
    /// A local run: the Studio engine's build number.
    pub engine_build: Option<u32>,
    /// A local run: the device it ran on, as shown.
    pub device: Option<String>,
    /// A local run: how long the engine took.
    pub elapsed_ms: Option<u64>,
    /// Unix ms when it was made.
    pub created_at: i64,
    /// Unix ms when sending (or the engine) started.
    pub started_at: Option<i64>,
    /// Unix ms when it ended.
    pub finished_at: Option<i64>,
    /// Unix ms when it was deleted: set only on a spending stub.
    pub deleted_at: Option<i64>,
    /// Its files, in order.
    pub outputs: Vec<CreationOutput>,
}

/// Filters for [`Store::list_creation_runs`]. Unset fields do not filter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CreationQuery {
    /// Only pictures, or only videos.
    pub kind: Option<CreationKind>,
    /// Only runs with a starred output.
    pub starred: bool,
    /// Only runs whose prompt contains this text (any case).
    pub text: Option<String>,
    /// Only runs of this model id.
    pub model: Option<String>,
    /// Only runs made at or after this Unix ms.
    pub since: Option<i64>,
    /// Only runs made before this Unix ms.
    pub until: Option<i64>,
    /// The page cursor: only runs that come after this one in the list's order (the previous
    /// page's last run).
    pub before: Option<RunCursor>,
    /// Maximum runs (the newest ones); 0 means no limit.
    pub limit: u32,
}

/// A place in [`Store::list_creation_runs`]'s order (`created_at` newest first, then `id`):
/// the last run of a page, so the next page starts just after it. Both fields are needed
/// because runs can share a creation time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunCursor {
    /// The run's `created_at`.
    pub created_at: i64,
    /// The run's id.
    pub id: String,
}

/// What Studio's files take, for Settings → Studio.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreationUsage {
    /// Outputs of picture runs.
    pub pictures: u64,
    /// Outputs of video runs.
    pub videos: u64,
    /// Bytes of all outputs.
    pub bytes: u64,
}

/// One finished local run's measured time, from [`Store::local_run_times`], for estimating
/// the next run's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalRunTime {
    /// Exactly what the run was given (its size, steps, pictures or frames).
    pub params: Value,
    /// How long the engine took.
    pub elapsed_ms: u64,
    /// Unix ms when it was made.
    pub created_at: i64,
}

/// What a delete removed, for removing files, sidecars and added pictures.
///
/// Each row is as it was before the call stubbed or deleted it, prompt, added pictures and
/// creation time included; each run carries the outputs it had.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Deleted {
    /// Every output removed.
    pub outputs: Vec<CreationOutput>,
    /// Every run removed or turned into a spending stub.
    pub runs: Vec<CreationRun>,
}

const RUN_COLUMNS: &str = "id, kind, runs_on, provider_id, provider_name, provider_host, model, \
     model_label, prompt, params_json, refs_json, status, remote_id, progress, estimate_usd, \
     cost_usd, error_json, audit_id, upstream, gen_model_id, engine_build, device, elapsed_ms, \
     created_at, started_at, finished_at, deleted_at";

fn run_from_row(r: &Row<'_>) -> rusqlite::Result<CreationRun> {
    Ok(CreationRun {
        id: r.get(0)?,
        kind: r.get(1)?,
        runs_on: r.get(2)?,
        provider_id: r.get(3)?,
        provider_name: r.get(4)?,
        provider_host: r.get(5)?,
        model: r.get(6)?,
        model_label: r.get(7)?,
        prompt: r.get(8)?,
        params: r.get::<_, JsonColumn>(9)?.0,
        refs: r.get::<_, JsonColumn>(10)?.0,
        status: r.get(11)?,
        remote_id: r.get(12)?,
        progress: r.get(13)?,
        estimate_usd: r.get(14)?,
        cost_usd: r.get(15)?,
        error: r.get::<_, Option<JsonColumn>>(16)?.map(|j| j.0),
        audit_id: r.get(17)?,
        upstream: r.get(18)?,
        gen_model_id: r.get(19)?,
        engine_build: r.get(20)?,
        device: r.get(21)?,
        elapsed_ms: r.get(22)?,
        created_at: r.get(23)?,
        started_at: r.get(24)?,
        finished_at: r.get(25)?,
        deleted_at: r.get(26)?,
        outputs: Vec::new(),
    })
}

const OUTPUT_COLUMNS: &str = "id, run_id, idx, path, thumb_path, mime, size_bytes, width, \
     height, duration_ms, seed, starred, created_at";

fn output_from_row(r: &Row<'_>) -> rusqlite::Result<CreationOutput> {
    Ok(CreationOutput {
        id: r.get(0)?,
        run_id: r.get(1)?,
        idx: r.get(2)?,
        path: r.get(3)?,
        thumb_path: r.get(4)?,
        mime: r.get(5)?,
        size_bytes: r.get(6)?,
        width: r.get(7)?,
        height: r.get(8)?,
        duration_ms: r.get(9)?,
        seed: r.get(10)?,
        starred: r.get(11)?,
        created_at: r.get(12)?,
    })
}

fn no_run(id: &str) -> StoreError {
    StoreError::InvalidInput(format!("there is no Studio run {id}"))
}

fn no_output(id: &str) -> StoreError {
    StoreError::InvalidInput(format!("there is no Studio output {id}"))
}

/// An insert's error: a taken id is [`StoreError::Conflict`].
fn taken(err: rusqlite::Error, what: &str, id: &str) -> StoreError {
    if StoreError::is_unique_violation(&err) {
        StoreError::Conflict(format!("a Studio {what} with the id {id} already exists"))
    } else {
        err.into()
    }
}

fn opt_real(value: Option<f64>) -> SqlValue {
    value.map_or(SqlValue::Null, SqlValue::Real)
}

fn opt_integer(value: Option<i64>) -> SqlValue {
    value.map_or(SqlValue::Null, SqlValue::Integer)
}

/// The outputs of run `run_id`, in order.
fn outputs_of(conn: &Connection, run_id: &str) -> Result<Vec<CreationOutput>, StoreError> {
    let sql =
        format!("SELECT {OUTPUT_COLUMNS} FROM creation_outputs WHERE run_id = ?1 ORDER BY idx, id");
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map([run_id], output_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The runs `SELECT {RUN_COLUMNS} FROM creation_runs <rest>` reads, each with its outputs.
fn select_runs(
    conn: &Connection,
    rest: &str,
    values: Vec<SqlValue>,
) -> Result<Vec<CreationRun>, StoreError> {
    let sql = format!("SELECT {RUN_COLUMNS} FROM creation_runs {rest}");
    let mut stmt = conn.prepare(&sql)?;
    let mut runs: Vec<CreationRun> = stmt
        .query_map(rusqlite::params_from_iter(values), run_from_row)?
        .collect::<Result<_, _>>()?;
    for run in &mut runs {
        run.outputs = outputs_of(conn, &run.id)?;
    }
    Ok(runs)
}

fn insert_run(conn: &mut Connection, run: NewCreationRun) -> Result<(), StoreError> {
    if run.id.is_empty() {
        return Err(StoreError::InvalidInput("a Studio run needs an id".into()));
    }
    let params_json = json_text(&run.params)?;
    let refs_json = json_text(&run.refs)?;
    let tx = conn.transaction()?;
    let held: i64 = tx.query_row(
        "SELECT count(*) FROM creation_runs WHERE deleted_at IS NULL",
        [],
        |r| r.get(0),
    )?;
    if held >= i64::try_from(MAX_CREATION_RUNS).unwrap_or(i64::MAX) {
        return Err(StoreError::TooManyCreations);
    }
    tx.execute(
        "INSERT INTO creation_runs (id, kind, runs_on, provider_id, provider_name, \
         provider_host, model, model_label, prompt, params_json, refs_json, status, \
         estimate_usd, gen_model_id, engine_build, device, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
        params![
            run.id,
            run.kind,
            run.runs_on,
            run.provider_id,
            run.provider_name,
            run.provider_host,
            run.model,
            run.model_label,
            run.prompt,
            params_json,
            refs_json,
            CreationStatus::Queued,
            run.estimate_usd,
            run.gen_model_id,
            run.engine_build,
            run.device,
            run.created_at
        ],
    )
    .map_err(|e| taken(e, "run", &run.id))?;
    tx.commit()?;
    Ok(())
}

fn update_run(conn: &Connection, id: &str, mut patch: CreationPatch) -> Result<(), StoreError> {
    let is_stub: bool = conn
        .query_row(
            "SELECT deleted_at IS NOT NULL FROM creation_runs WHERE id = ?1",
            [id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| no_run(id))?;
    if is_stub {
        // A stub keeps these cleared; its status, cost, error and times still change (a
        // cost that arrives after the delete still counts).
        patch.upstream = None;
        patch.device = None;
        patch.elapsed_ms = None;
    }
    let mut sets: Vec<(&'static str, SqlValue)> = Vec::new();
    if let Some(v) = patch.status {
        sets.push(("status", SqlValue::Text(v.as_str().to_owned())));
    }
    if let Some(v) = patch.remote_id {
        sets.push(("remote_id", sql_opt_text(v)));
    }
    if let Some(v) = patch.progress {
        sets.push(("progress", opt_real(v)));
    }
    if let Some(v) = patch.cost_usd {
        sets.push(("cost_usd", opt_real(v)));
    }
    if let Some(v) = patch.error {
        let text = v.as_ref().map(json_text).transpose()?;
        sets.push(("error_json", sql_opt_text(text)));
    }
    if let Some(v) = patch.audit_id {
        sets.push(("audit_id", opt_integer(v.map(|a| a.0))));
    }
    if let Some(v) = patch.upstream {
        sets.push(("upstream", sql_opt_text(v)));
    }
    if let Some(v) = patch.started_at {
        sets.push(("started_at", opt_integer(v)));
    }
    if let Some(v) = patch.finished_at {
        sets.push(("finished_at", opt_integer(v)));
    }
    if let Some(v) = patch.elapsed_ms {
        sets.push(("elapsed_ms", sql_opt_u64("elapsed_ms", v)?));
    }
    if let Some(v) = patch.device {
        sets.push(("device", sql_opt_text(v)));
    }
    if !sets.is_empty() && update_columns_by_text(conn, "creation_runs", "id", id, sets)? == 0 {
        return Err(no_run(id));
    }
    Ok(())
}

fn get_run(conn: &Connection, id: &str) -> Result<Option<CreationRun>, StoreError> {
    Ok(select_runs(
        conn,
        "WHERE id = ?1 AND deleted_at IS NULL",
        vec![SqlValue::Text(id.to_owned())],
    )?
    .pop())
}

fn list_runs(conn: &Connection, q: CreationQuery) -> Result<Vec<CreationRun>, StoreError> {
    let mut rest = String::from("WHERE deleted_at IS NULL");
    let mut values: Vec<SqlValue> = Vec::new();
    if let Some(kind) = q.kind {
        rest.push_str(" AND kind = ?");
        values.push(SqlValue::Text(kind.as_str().to_owned()));
    }
    if q.starred {
        rest.push_str(
            " AND EXISTS (SELECT 1 FROM creation_outputs o \
             WHERE o.run_id = creation_runs.id AND o.starred = 1)",
        );
    }
    if let Some(text) = q.text.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        rest.push_str(" AND prompt LIKE ? ESCAPE '\\'");
        values.push(SqlValue::Text(like_contains(text)));
    }
    if let Some(model) = q.model {
        rest.push_str(" AND model = ?");
        values.push(SqlValue::Text(model));
    }
    if let Some(since) = q.since {
        rest.push_str(" AND created_at >= ?");
        values.push(SqlValue::Integer(since));
    }
    if let Some(until) = q.until {
        rest.push_str(" AND created_at < ?");
        values.push(SqlValue::Integer(until));
    }
    if let Some(before) = q.before {
        // The same order as ORDER BY, so runs sharing a creation time are neither skipped
        // nor repeated at a page break.
        rest.push_str(" AND (created_at, id) < (?, ?)");
        values.push(SqlValue::Integer(before.created_at));
        values.push(SqlValue::Text(before.id));
    }
    rest.push_str(" ORDER BY created_at DESC, id DESC LIMIT ?");
    values.push(SqlValue::Integer(sql_limit(q.limit)));
    select_runs(conn, &rest, values)
}

fn insert_output(conn: &Connection, out: NewCreationOutput) -> Result<(), StoreError> {
    let run: Option<Option<i64>> = conn
        .query_row(
            "SELECT deleted_at FROM creation_runs WHERE id = ?1",
            [&out.run_id],
            |r| r.get(0),
        )
        .optional()?;
    match run {
        None => return Err(no_run(&out.run_id)),
        Some(Some(_)) => {
            return Err(StoreError::InvalidInput(format!(
                "the Studio run {} was deleted",
                out.run_id
            )));
        }
        Some(None) => {}
    }
    conn.execute(
        "INSERT INTO creation_outputs (id, run_id, idx, path, thumb_path, mime, size_bytes, \
         width, height, duration_ms, seed, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            out.id,
            out.run_id,
            out.idx,
            out.path,
            out.thumb_path,
            out.mime,
            sql_u64("size_bytes", out.size_bytes)?,
            out.width,
            out.height,
            sql_opt_u64("duration_ms", out.duration_ms)?,
            sql_opt_u64("seed", out.seed)?,
            out.created_at
        ],
    )
    .map_err(|e| taken(e, "output", &out.id))?;
    Ok(())
}

fn get_output(conn: &Connection, id: &str) -> Result<Option<CreationOutput>, StoreError> {
    let sql = format!(
        "SELECT {OUTPUT_COLUMNS} FROM creation_outputs WHERE id = ?1 AND EXISTS \
         (SELECT 1 FROM creation_runs r \
          WHERE r.id = creation_outputs.run_id AND r.deleted_at IS NULL)"
    );
    Ok(conn.query_row(&sql, [id], output_from_row).optional()?)
}

fn set_output_column(
    conn: &Connection,
    id: &str,
    column: &'static str,
    value: SqlValue,
) -> Result<(), StoreError> {
    if update_columns_by_text(conn, "creation_outputs", "id", id, vec![(column, value)])? == 0 {
        return Err(no_output(id));
    }
    Ok(())
}

/// Removes run `run` (read before any change): a stub when it was made at or after
/// `stub_since_ms`, else the row and, through the cascade, its outputs.
fn remove_run(conn: &Connection, run: &CreationRun, stub_since_ms: i64) -> Result<(), StoreError> {
    if run.created_at >= stub_since_ms {
        // The row stays, so the cascade does not fire: the outputs go explicitly.
        conn.execute("DELETE FROM creation_outputs WHERE run_id = ?1", [&run.id])?;
        conn.execute(
            "UPDATE creation_runs SET prompt = '', params_json = '{}', refs_json = '[]', \
             upstream = NULL, gen_model_id = NULL, device = NULL, elapsed_ms = NULL, \
             deleted_at = ?2 WHERE id = ?1",
            params![run.id, now_ms()],
        )?;
    } else {
        conn.execute("DELETE FROM creation_runs WHERE id = ?1", [&run.id])?;
    }
    Ok(())
}

fn delete_outputs(
    conn: &mut Connection,
    ids: Vec<String>,
    stub_since_ms: i64,
) -> Result<Deleted, StoreError> {
    let tx = conn.transaction()?;
    let mut deleted = Deleted::default();
    // The runs the outputs belong to, as they were.
    let mut runs: BTreeMap<String, CreationRun> = BTreeMap::new();
    for id in ids {
        let Some(out) = get_output(&tx, &id)? else {
            continue;
        };
        if !runs.contains_key(&out.run_id)
            && let Some(run) = get_run(&tx, &out.run_id)?
        {
            runs.insert(run.id.clone(), run);
        }
        tx.execute("DELETE FROM creation_outputs WHERE id = ?1", [&out.id])?;
        deleted.outputs.push(out);
    }
    // A finished run left without outputs goes too; a run still working, or one that
    // ended without finishing, keeps its card.
    for run in runs.into_values() {
        if run.status != CreationStatus::Done {
            continue;
        }
        let left: i64 = tx.query_row(
            "SELECT count(*) FROM creation_outputs WHERE run_id = ?1",
            [&run.id],
            |r| r.get(0),
        )?;
        if left == 0 {
            remove_run(&tx, &run, stub_since_ms)?;
            deleted.runs.push(run);
        }
    }
    tx.commit()?;
    Ok(deleted)
}

fn delete_runs(
    conn: &mut Connection,
    ids: Vec<String>,
    stub_since_ms: i64,
) -> Result<Deleted, StoreError> {
    let tx = conn.transaction()?;
    let mut deleted = Deleted::default();
    for id in ids {
        let Some(run) = get_run(&tx, &id)? else {
            continue;
        };
        remove_run(&tx, &run, stub_since_ms)?;
        deleted.outputs.extend(run.outputs.iter().cloned());
        deleted.runs.push(run);
    }
    tx.commit()?;
    Ok(deleted)
}

/// The SHA-256 of every added picture a run that is not a stub uses.
fn ref_hashes(conn: &Connection) -> Result<BTreeSet<String>, StoreError> {
    let mut stmt =
        conn.prepare_cached("SELECT refs_json FROM creation_runs WHERE deleted_at IS NULL")?;
    let rows = stmt.query_map([], |r| r.get::<_, JsonColumn>(0))?;
    let mut hashes = BTreeSet::new();
    for refs in rows {
        if let Value::Array(items) = refs?.0 {
            hashes.extend(
                items
                    .iter()
                    .filter_map(|item| item.get("sha256")?.as_str().map(str::to_owned)),
            );
        }
    }
    Ok(hashes)
}

fn usage(conn: &Connection) -> Result<CreationUsage, StoreError> {
    Ok(conn.query_row(
        "SELECT coalesce(sum(r.kind = 'image'), 0), coalesce(sum(r.kind = 'video'), 0), \
         coalesce(sum(o.size_bytes), 0) \
         FROM creation_outputs o JOIN creation_runs r ON r.id = o.run_id \
         WHERE r.deleted_at IS NULL",
        [],
        |r| {
            Ok(CreationUsage {
                pictures: r.get(0)?,
                videos: r.get(1)?,
                bytes: r.get(2)?,
            })
        },
    )?)
}

impl Store {
    /// Records a new run as [`CreationStatus::Queued`].
    ///
    /// Fails with [`StoreError::TooManyCreations`] when [`MAX_CREATION_RUNS`] runs are
    /// already kept (spending stubs not counted), [`StoreError::Conflict`] when the id is
    /// taken, and [`StoreError::InvalidInput`] for an empty id.
    pub async fn insert_creation_run(&self, run: NewCreationRun) -> Result<(), StoreError> {
        self.call(move |c| insert_run(c, run)).await
    }

    /// Changes the fields `patch` sets on run `id`. A spending stub can still be changed
    /// (a cost that arrives after a delete still counts), except `upstream`, `device` and
    /// `elapsed_ms`, which a stub keeps cleared; a run that does not exist is
    /// [`StoreError::InvalidInput`].
    pub async fn update_creation_run(
        &self,
        id: &str,
        patch: CreationPatch,
    ) -> Result<(), StoreError> {
        let id = id.to_owned();
        self.call(move |c| update_run(c, &id, patch)).await
    }

    /// Run `id` with its outputs; `None` when it does not exist or is a spending stub.
    pub async fn get_creation_run(&self, id: &str) -> Result<Option<CreationRun>, StoreError> {
        let id = id.to_owned();
        self.call(move |c| get_run(c, &id)).await
    }

    /// The runs `q` selects, newest first, each with its outputs. Spending stubs are left out.
    pub async fn list_creation_runs(
        &self,
        q: CreationQuery,
    ) -> Result<Vec<CreationRun>, StoreError> {
        self.call(move |c| list_runs(c, q)).await
    }

    /// The runs still working (queued, sending, waiting, running or downloading), oldest
    /// first, each with its outputs. Spending stubs are left out.
    pub async fn live_creation_runs(&self) -> Result<Vec<CreationRun>, StoreError> {
        self.call(|c| {
            select_runs(
                c,
                &format!(
                    "WHERE status IN {LIVE_STATUSES} AND deleted_at IS NULL \
                     ORDER BY created_at, id"
                ),
                Vec::new(),
            )
        })
        .await
    }

    /// Every run made at or after `since_ms`, spending stubs included (spending reads
    /// this), newest first.
    pub async fn creation_runs_since(&self, since_ms: i64) -> Result<Vec<CreationRun>, StoreError> {
        self.call(move |c| {
            select_runs(
                c,
                "WHERE created_at >= ?1 ORDER BY created_at DESC, id DESC",
                vec![SqlValue::Integer(since_ms)],
            )
        })
        .await
    }

    /// Records a file run `out.run_id` made. A run that does not exist, or a spending stub,
    /// is [`StoreError::InvalidInput`]; an id that is taken is [`StoreError::Conflict`].
    pub async fn insert_creation_output(&self, out: NewCreationOutput) -> Result<(), StoreError> {
        self.call(move |c| insert_output(c, out)).await
    }

    /// Output `id`; `None` when it does not exist.
    pub async fn get_creation_output(
        &self,
        id: &str,
    ) -> Result<Option<CreationOutput>, StoreError> {
        let id = id.to_owned();
        self.call(move |c| get_output(c, &id)).await
    }

    /// Records output `id`'s thumbnail. An output that does not exist is
    /// [`StoreError::InvalidInput`].
    pub async fn set_creation_thumb(&self, id: &str, thumb_path: &str) -> Result<(), StoreError> {
        let id = id.to_owned();
        let thumb = SqlValue::Text(thumb_path.to_owned());
        self.call(move |c| set_output_column(c, &id, "thumb_path", thumb))
            .await
    }

    /// Stars or unstars output `id`. An output that does not exist is
    /// [`StoreError::InvalidInput`].
    pub async fn set_creation_starred(&self, id: &str, starred: bool) -> Result<(), StoreError> {
        let id = id.to_owned();
        let value = SqlValue::Integer(i64::from(starred));
        self.call(move |c| set_output_column(c, &id, "starred", value))
            .await
    }

    /// Deletes outputs; a finished run left with none is deleted too. A deleted run created
    /// at or after `stub_since_ms` (the start of today) stays as a stub, everything else
    /// goes.
    ///
    /// Ids that do not exist are skipped. A run still working, or one that ended without
    /// finishing, keeps its card when its last output goes.
    pub async fn delete_creation_outputs(
        &self,
        ids: Vec<String>,
        stub_since_ms: i64,
    ) -> Result<Deleted, StoreError> {
        self.call(move |c| delete_outputs(c, ids, stub_since_ms))
            .await
    }

    /// Deletes runs and their outputs. A run created at or after `stub_since_ms` (the start
    /// of today) stays as a spending stub, everything else goes. Ids that do not exist, and
    /// spending stubs, are skipped.
    pub async fn delete_creation_runs(
        &self,
        ids: Vec<String>,
        stub_since_ms: i64,
    ) -> Result<Deleted, StoreError> {
        self.call(move |c| delete_runs(c, ids, stub_since_ms)).await
    }

    /// Removes the spending stubs of runs made before `before_ms` (at start-up: yesterday's
    /// stubs) and returns how many went.
    pub async fn purge_creation_stubs(&self, before_ms: i64) -> Result<usize, StoreError> {
        self.call(move |c| {
            Ok(c.execute(
                "DELETE FROM creation_runs WHERE deleted_at IS NOT NULL AND created_at < ?1",
                [before_ms],
            )?)
        })
        .await
    }

    /// The ids of runs that ended without finishing (failed, blocked, cancelled or
    /// interrupted), oldest first, for *Delete stopped and failed runs*. Spending stubs are
    /// left out.
    pub async fn unfinished_creation_run_ids(&self) -> Result<Vec<String>, StoreError> {
        self.call(|c| {
            let sql = format!(
                "SELECT id FROM creation_runs WHERE status IN {UNFINISHED_STATUSES} \
                 AND deleted_at IS NULL ORDER BY created_at, id"
            );
            let mut stmt = c.prepare(&sql)?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
        .await
    }

    /// The SHA-256 of every added picture a run uses (spending stubs use none), so a
    /// delete removes only the pictures no run still needs.
    pub async fn creation_ref_hashes(&self) -> Result<BTreeSet<String>, StoreError> {
        self.call(|c| ref_hashes(c)).await
    }

    /// How many pictures and videos Studio keeps, and their bytes.
    pub async fn creation_usage(&self) -> Result<CreationUsage, StoreError> {
        self.call(|c| usage(c)).await
    }

    /// This model's finished local runs on `device` (the device as shown, the string a run
    /// records), newest first, at most `limit` (0 means no limit): this computer's measured
    /// times. Runs that did not finish, runs with no time taken recorded, cloud runs and
    /// spending stubs are left out.
    pub async fn local_run_times(
        &self,
        gen_model_id: &str,
        device: &str,
        limit: u32,
    ) -> Result<Vec<LocalRunTime>, StoreError> {
        let gen_model_id = gen_model_id.to_owned();
        let device = device.to_owned();
        self.call(move |c| {
            let mut stmt = c.prepare_cached(
                "SELECT params_json, elapsed_ms, created_at FROM creation_runs \
                 WHERE runs_on = ?1 AND status = ?2 AND gen_model_id = ?3 AND device = ?4 \
                 AND elapsed_ms IS NOT NULL AND deleted_at IS NULL \
                 ORDER BY created_at DESC, id DESC LIMIT ?5",
            )?;
            let rows = stmt.query_map(
                params![
                    RunsOn::Local,
                    CreationStatus::Done,
                    gen_model_id,
                    device,
                    sql_limit(limit)
                ],
                |r| {
                    Ok(LocalRunTime {
                        params: r.get::<_, JsonColumn>(0)?.0,
                        elapsed_ms: r.get(1)?,
                        created_at: r.get(2)?,
                    })
                },
            )?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashSet};

    use super::*;
    use crate::{ProviderInput, StoreError};
    use serde_json::json;

    const DAY_MS: i64 = 86_400_000;
    /// 2026-11-17 00:00 UTC.
    const DAY_START: i64 = 1_794_873_600_000;

    fn run(id: &str) -> NewCreationRun {
        NewCreationRun {
            id: id.into(),
            kind: CreationKind::Image,
            runs_on: RunsOn::Cloud,
            provider_id: None,
            provider_name: "OpenRouter".into(),
            provider_host: "openrouter.ai".into(),
            model: "acme/picture-1".into(),
            model_label: "Picture One".into(),
            prompt: "a lighthouse at dusk".into(),
            params: json!({"size": "1024x1024"}),
            refs: json!([]),
            estimate_usd: Some(0.04),
            gen_model_id: None,
            engine_build: None,
            device: None,
            created_at: DAY_START + 1_000,
        }
    }

    fn output(id: &str, run_id: &str, idx: u32) -> NewCreationOutput {
        NewCreationOutput {
            id: id.into(),
            run_id: run_id.into(),
            idx,
            path: format!("/g/2026-11/{run_id}-{idx}.png"),
            thumb_path: None,
            mime: "image/png".into(),
            size_bytes: 1_000,
            width: Some(1024),
            height: Some(1024),
            duration_ms: None,
            seed: Some(42),
            created_at: DAY_START + 2_000,
        }
    }

    fn status(s: CreationStatus) -> CreationPatch {
        CreationPatch {
            status: Some(s),
            ..CreationPatch::default()
        }
    }

    async fn insert_done(store: &Store, new: NewCreationRun, outputs: u32) {
        let id = new.id.clone();
        store.insert_creation_run(new).await.expect("insert run");
        for idx in 0..outputs {
            store
                .insert_creation_output(output(&format!("{id}-{idx}"), &id, idx))
                .await
                .expect("insert output");
        }
        store
            .update_creation_run(&id, status(CreationStatus::Done))
            .await
            .expect("done");
    }

    #[tokio::test]
    async fn a_run_reads_back_as_it_was_written() {
        let store = Store::open_in_memory().expect("open");
        store
            .insert_creation_run(NewCreationRun {
                refs: json!([{"sha256": "ab".repeat(32), "role": "reference"}]),
                ..run("r1")
            })
            .await
            .expect("insert");
        let fresh = store
            .get_creation_run("r1")
            .await
            .expect("get")
            .expect("exists");
        assert_eq!(fresh.status, CreationStatus::Queued, "a new run is queued");
        assert_eq!(fresh.runs_on, RunsOn::Cloud);
        assert_eq!(fresh.params, json!({"size": "1024x1024"}));
        assert_eq!(fresh.refs[0]["role"], "reference");
        assert_eq!(fresh.created_at, DAY_START + 1_000, "the caller's clock");
        assert_eq!(
            (
                fresh.remote_id,
                fresh.cost_usd,
                fresh.error,
                fresh.deleted_at
            ),
            (None, None, None, None)
        );

        store
            .update_creation_run(
                "r1",
                CreationPatch {
                    status: Some(CreationStatus::Waiting),
                    remote_id: Some(Some("job-7".into())),
                    progress: Some(Some(0.5)),
                    cost_usd: Some(Some(0.05)),
                    error: Some(Some(json!({"charged": "unknown"}))),
                    audit_id: Some(Some(AuditId(12))),
                    upstream: Some(Some("Acme Cloud".into())),
                    started_at: Some(Some(DAY_START + 1_500)),
                    finished_at: Some(Some(DAY_START + 9_000)),
                    ..CreationPatch::default()
                },
            )
            .await
            .expect("update");
        let r = store
            .get_creation_run("r1")
            .await
            .expect("get")
            .expect("exists");
        assert_eq!(r.status, CreationStatus::Waiting);
        assert_eq!(r.remote_id.as_deref(), Some("job-7"));
        assert_eq!((r.progress, r.cost_usd), (Some(0.5), Some(0.05)));
        assert_eq!(r.error, Some(json!({"charged": "unknown"})));
        assert_eq!(r.audit_id, Some(AuditId(12)));
        assert_eq!(r.upstream.as_deref(), Some("Acme Cloud"));
        assert_eq!(
            (r.started_at, r.finished_at),
            (Some(DAY_START + 1_500), Some(DAY_START + 9_000))
        );
        assert_eq!(
            r.prompt, "a lighthouse at dusk",
            "fields not in the patch stay"
        );

        // `Some(None)` clears a field; an empty patch changes nothing.
        store
            .update_creation_run(
                "r1",
                CreationPatch {
                    progress: Some(None),
                    error: Some(None),
                    ..CreationPatch::default()
                },
            )
            .await
            .expect("clear");
        store
            .update_creation_run("r1", CreationPatch::default())
            .await
            .expect("empty patch");
        let r = store
            .get_creation_run("r1")
            .await
            .expect("get")
            .expect("exists");
        assert_eq!((r.progress, r.error), (None, None));
        assert_eq!(r.remote_id.as_deref(), Some("job-7"));

        // The same id twice is a conflict; a run that does not exist cannot be changed.
        assert!(matches!(
            store.insert_creation_run(run("r1")).await,
            Err(StoreError::Conflict(_))
        ));
        assert!(matches!(
            store
                .update_creation_run("nope", status(CreationStatus::Done))
                .await,
            Err(StoreError::InvalidInput(_))
        ));
        assert!(matches!(
            store
                .update_creation_run("nope", CreationPatch::default())
                .await,
            Err(StoreError::InvalidInput(_))
        ));
        assert!(store.get_creation_run("nope").await.expect("get").is_none());
    }

    #[tokio::test]
    async fn deleting_an_endpoint_keeps_its_runs_with_their_names() {
        let store = Store::open_in_memory().expect("open");
        let provider = store
            .upsert_provider(ProviderInput {
                id: None,
                name: "OpenRouter".into(),
                kind: "openrouter".into(),
                base_url: "https://openrouter.ai/api/v1".into(),
                secret_ref: None,
                enabled: true,
            })
            .await
            .expect("provider");
        store
            .insert_creation_run(NewCreationRun {
                provider_id: Some(provider),
                ..run("r1")
            })
            .await
            .expect("insert");
        assert_eq!(
            store
                .get_creation_run("r1")
                .await
                .expect("get")
                .expect("exists")
                .provider_id,
            Some(provider)
        );

        assert!(store.delete_provider(provider).await.expect("delete"));

        let r = store
            .get_creation_run("r1")
            .await
            .expect("get")
            .expect("the run outlives its endpoint");
        assert_eq!(r.provider_id, None);
        assert_eq!(
            (r.provider_name.as_str(), r.provider_host.as_str()),
            ("OpenRouter", "openrouter.ai")
        );
        assert_eq!(r.runs_on, RunsOn::Cloud, "a deleted endpoint is not local");
    }

    #[tokio::test]
    async fn deleting_a_run_deletes_its_outputs() {
        let store = Store::open_in_memory().expect("open");
        insert_done(&store, run("r1"), 2).await;
        insert_done(&store, run("r2"), 1).await;
        let r1 = store
            .get_creation_run("r1")
            .await
            .expect("get")
            .expect("exists");
        assert_eq!(r1.outputs.len(), 2);
        assert_eq!(
            r1.outputs.iter().map(|o| o.idx).collect::<Vec<_>>(),
            vec![0, 1]
        );

        // Created before `stub_since_ms`: deleted outright.
        let d = store
            .delete_creation_runs(vec!["r1".into(), "missing".into()], i64::MAX)
            .await
            .expect("delete");
        let mut ids: Vec<&str> = d.outputs.iter().map(|o| o.id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec!["r1-0", "r1-1"], "returned for file removal");
        assert_eq!(d.runs.len(), 1);
        assert_eq!(d.runs[0].id, "r1");
        assert_eq!(d.runs[0].outputs.len(), 2, "the run as it was");
        assert!(store.get_creation_run("r1").await.expect("get").is_none());
        assert!(
            store
                .get_creation_output("r1-0")
                .await
                .expect("get")
                .is_none()
        );
        let left: i64 = store
            .call(|c| {
                Ok(c.query_row(
                    "SELECT count(*) FROM creation_outputs WHERE run_id = 'r1'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .await
            .expect("count");
        assert_eq!(left, 0);
        let gone: i64 = store
            .call(|c| {
                Ok(c.query_row(
                    "SELECT count(*) FROM creation_runs WHERE id = 'r1'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .await
            .expect("count");
        assert_eq!(gone, 0, "no stub for a run of an earlier day");
        assert!(
            store
                .get_creation_output("r2-0")
                .await
                .expect("get")
                .is_some(),
            "other runs keep theirs"
        );
    }

    #[tokio::test]
    async fn a_status_outside_the_list_is_refused() {
        let store = Store::open_in_memory().expect("open");
        store.insert_creation_run(run("r1")).await.expect("insert");
        let refused = store
            .call(|c| {
                Ok(c.execute(
                    "UPDATE creation_runs SET status = 'generating' WHERE id = 'r1'",
                    [],
                )?)
            })
            .await;
        assert!(matches!(refused, Err(StoreError::Sqlite(_))), "{refused:?}");
        for s in CreationStatus::ALL {
            store
                .update_creation_run("r1", status(*s))
                .await
                .expect("every listed status is accepted");
        }
    }

    #[tokio::test]
    async fn runs_list_newest_first_with_filters() {
        let store = Store::open_in_memory().expect("open");
        let at = |n: i64| DAY_START + n * 1_000;
        for (id, n, kind, model, prompt) in [
            (
                "a",
                1,
                CreationKind::Image,
                "acme/picture-1",
                "100% cotton shirt",
            ),
            (
                "b",
                2,
                CreationKind::Video,
                "acme/video-1",
                "100 cotton shirts",
            ),
            (
                "c",
                3,
                CreationKind::Image,
                "acme/picture-2",
                "snake_case sign",
            ),
            (
                "d",
                4,
                CreationKind::Image,
                "acme/picture-1",
                "snakeXcase sign",
            ),
        ] {
            store
                .insert_creation_run(NewCreationRun {
                    kind,
                    model: model.into(),
                    prompt: prompt.into(),
                    created_at: at(n),
                    ..run(id)
                })
                .await
                .expect("insert");
        }
        store
            .insert_creation_output(output("c-0", "c", 0))
            .await
            .expect("output");
        store
            .insert_creation_output(output("c-1", "c", 1))
            .await
            .expect("output");
        store.set_creation_starred("c-1", true).await.expect("star");
        store
            .insert_creation_output(output("d-0", "d", 0))
            .await
            .expect("output");

        let ids = |runs: Vec<CreationRun>| runs.into_iter().map(|r| r.id).collect::<Vec<_>>();
        let list = |q: CreationQuery| {
            let store = store.clone();
            async move { ids(store.list_creation_runs(q).await.expect("list")) }
        };

        assert_eq!(list(CreationQuery::default()).await, ["d", "c", "b", "a"]);
        assert_eq!(
            store
                .list_creation_runs(CreationQuery::default())
                .await
                .expect("list")[1]
                .outputs
                .len(),
            2,
            "each run comes with its outputs"
        );
        assert_eq!(
            list(CreationQuery {
                kind: Some(CreationKind::Video),
                ..CreationQuery::default()
            })
            .await,
            ["b"]
        );
        assert_eq!(
            list(CreationQuery {
                starred: true,
                ..CreationQuery::default()
            })
            .await,
            ["c"],
            "a run with a starred output"
        );
        assert_eq!(
            list(CreationQuery {
                text: Some("100%".into()),
                ..CreationQuery::default()
            })
            .await,
            ["a"],
            "% is a character, not a wildcard"
        );
        assert_eq!(
            list(CreationQuery {
                text: Some("e_c".into()),
                ..CreationQuery::default()
            })
            .await,
            ["c"],
            "_ is a character, not a wildcard"
        );
        assert_eq!(
            list(CreationQuery {
                text: Some("COTTON".into()),
                ..CreationQuery::default()
            })
            .await,
            ["b", "a"],
            "case does not matter"
        );
        assert_eq!(
            list(CreationQuery {
                model: Some("acme/picture-1".into()),
                ..CreationQuery::default()
            })
            .await,
            ["d", "a"]
        );
        assert_eq!(
            list(CreationQuery {
                since: Some(at(2)),
                until: Some(at(4)),
                ..CreationQuery::default()
            })
            .await,
            ["c", "b"],
            "since is inclusive, until is not"
        );
        assert_eq!(
            list(CreationQuery {
                limit: 2,
                ..CreationQuery::default()
            })
            .await,
            ["d", "c"]
        );
        assert_eq!(
            list(CreationQuery {
                before: Some(RunCursor {
                    created_at: at(3),
                    id: "c".into(),
                }),
                limit: 2,
                ..CreationQuery::default()
            })
            .await,
            ["b", "a"],
            "the next page starts below the cursor"
        );
    }

    #[tokio::test]
    async fn paging_walks_runs_that_share_a_creation_time() {
        let store = Store::open_in_memory().expect("open");
        // A fixed clock gives every run the same creation time.
        for id in ["r2", "r1", "r3"] {
            store
                .insert_creation_run(NewCreationRun {
                    created_at: DAY_START + 5_000,
                    ..run(id)
                })
                .await
                .expect("insert");
        }
        let mut seen: Vec<String> = Vec::new();
        let mut cursor: Option<RunCursor> = None;
        loop {
            let page = store
                .list_creation_runs(CreationQuery {
                    before: cursor.clone(),
                    limit: 1,
                    ..CreationQuery::default()
                })
                .await
                .expect("list");
            let Some(last) = page.last() else {
                break;
            };
            assert_eq!(page.len(), 1, "the limit holds");
            cursor = Some(RunCursor {
                created_at: last.created_at,
                id: last.id.clone(),
            });
            seen.push(last.id.clone());
            assert!(seen.len() <= 3, "the pages end: {seen:?}");
        }
        assert_eq!(
            seen,
            ["r3", "r2", "r1"],
            "each run once, in created_at DESC, id DESC order"
        );
    }

    #[tokio::test]
    async fn live_runs_are_the_five_working_states() {
        let store = Store::open_in_memory().expect("open");
        for (n, s) in CreationStatus::ALL.iter().enumerate() {
            let id = s.as_str();
            store
                .insert_creation_run(NewCreationRun {
                    created_at: DAY_START + i64::try_from(n).expect("small"),
                    ..run(id)
                })
                .await
                .expect("insert");
            store
                .update_creation_run(id, status(*s))
                .await
                .expect("status");
        }
        let live: HashSet<CreationStatus> = store
            .live_creation_runs()
            .await
            .expect("live")
            .into_iter()
            .map(|r| r.status)
            .collect();
        assert_eq!(
            live,
            HashSet::from([
                CreationStatus::Queued,
                CreationStatus::Sending,
                CreationStatus::Waiting,
                CreationStatus::Running,
                CreationStatus::Downloading,
            ])
        );
        let unfinished: BTreeSet<String> = store
            .unfinished_creation_run_ids()
            .await
            .expect("unfinished")
            .into_iter()
            .collect();
        assert_eq!(
            unfinished,
            BTreeSet::from(["failed", "blocked", "cancelled", "interrupted"].map(String::from))
        );

        // A stub is never live or unfinished, whatever its status.
        store
            .delete_creation_runs(vec!["queued".into(), "failed".into()], DAY_START)
            .await
            .expect("delete");
        assert_eq!(store.live_creation_runs().await.expect("live").len(), 4);
        assert_eq!(
            store
                .unfinished_creation_run_ids()
                .await
                .expect("unfinished")
                .len(),
            3
        );
    }

    #[tokio::test]
    async fn a_local_run_has_no_endpoint_and_keeps_its_engine_facts() {
        let store = Store::open_in_memory().expect("open");
        store
            .insert_creation_run(NewCreationRun {
                runs_on: RunsOn::Local,
                provider_id: None,
                provider_name: "This computer".into(),
                provider_host: String::new(),
                model: "flux2-klein-4b-q4".into(),
                estimate_usd: Some(0.0),
                gen_model_id: Some("flux2-klein-4b-q4".into()),
                engine_build: Some(1106),
                device: Some("vulkan0".into()),
                ..run("local")
            })
            .await
            .expect("insert");
        store
            .update_creation_run(
                "local",
                CreationPatch {
                    status: Some(CreationStatus::Running),
                    elapsed_ms: Some(Some(228_000)),
                    device: Some(Some("Radeon RX 7600 (Vulkan)".into())),
                    ..CreationPatch::default()
                },
            )
            .await
            .expect("update");
        let r = store
            .get_creation_run("local")
            .await
            .expect("get")
            .expect("exists");
        assert_eq!(r.runs_on, RunsOn::Local);
        assert_eq!(r.provider_id, None);
        assert_eq!(
            (r.provider_name.as_str(), r.provider_host.as_str()),
            ("This computer", "")
        );
        assert_eq!(r.gen_model_id.as_deref(), Some("flux2-klein-4b-q4"));
        assert_eq!(r.engine_build, Some(1106));
        assert_eq!(r.elapsed_ms, Some(228_000));
        assert_eq!(r.device.as_deref(), Some("Radeon RX 7600 (Vulkan)"));
        assert_eq!(r.status, CreationStatus::Running);
        assert_eq!(
            store.live_creation_runs().await.expect("live").len(),
            1,
            "running is a working state"
        );
    }

    #[tokio::test]
    async fn the_last_output_takes_its_finished_run_along() {
        let store = Store::open_in_memory().expect("open");
        insert_done(&store, run("done"), 2).await;

        let d = store
            .delete_creation_outputs(vec!["done-0".into()], i64::MAX)
            .await
            .expect("delete one");
        assert_eq!(d.outputs.len(), 1);
        assert_eq!(d.outputs[0].id, "done-0");
        assert!(d.runs.is_empty(), "the run still has a result");
        let r = store
            .get_creation_run("done")
            .await
            .expect("get")
            .expect("kept");
        assert_eq!(
            r.outputs.iter().map(|o| o.id.as_str()).collect::<Vec<_>>(),
            ["done-1"]
        );

        let d = store
            .delete_creation_outputs(vec!["done-1".into()], i64::MAX)
            .await
            .expect("delete the other");
        assert_eq!(d.outputs.len(), 1);
        assert_eq!(d.runs.len(), 1, "returned for its sidecar and refs");
        assert_eq!(d.runs[0].id, "done");
        assert_eq!(d.runs[0].prompt, "a lighthouse at dusk", "as it was");
        assert!(store.get_creation_run("done").await.expect("get").is_none());

        // A run still working, or one that ended without finishing, keeps its card.
        for (id, s) in [
            ("working", CreationStatus::Downloading),
            ("failed", CreationStatus::Failed),
        ] {
            store.insert_creation_run(run(id)).await.expect("insert");
            store
                .insert_creation_output(output(&format!("{id}-0"), id, 0))
                .await
                .expect("output");
            store
                .update_creation_run(id, status(s))
                .await
                .expect("status");
            let d = store
                .delete_creation_outputs(vec![format!("{id}-0")], i64::MAX)
                .await
                .expect("delete");
            assert_eq!((d.outputs.len(), d.runs.len()), (1, 0), "{id}");
            let r = store
                .get_creation_run(id)
                .await
                .expect("get")
                .expect("kept");
            assert!(r.outputs.is_empty(), "{id}");
        }
    }

    #[tokio::test]
    async fn deleting_todays_run_keeps_a_spending_stub_without_the_prompt() {
        let store = Store::open_in_memory().expect("open");
        let day_start = DAY_START;
        insert_done(
            &store,
            NewCreationRun {
                refs: json!([{"sha256": "ab".repeat(32), "role": "reference"}]),
                created_at: day_start + 3_600_000,
                ..run("today")
            },
            1,
        )
        .await;
        store
            .update_creation_run(
                "today",
                CreationPatch {
                    cost_usd: Some(Some(9.5)),
                    error: Some(Some(json!({"charged": "yes"}))),
                    audit_id: Some(Some(AuditId(3))),
                    upstream: Some(Some("Acme Cloud".into())),
                    finished_at: Some(Some(day_start + 3_700_000)),
                    ..CreationPatch::default()
                },
            )
            .await
            .expect("cost");
        insert_done(
            &store,
            NewCreationRun {
                created_at: day_start - 3_600_000,
                ..run("yesterday")
            },
            1,
        )
        .await;

        let d = store
            .delete_creation_runs(vec!["today".into(), "yesterday".into()], day_start)
            .await
            .unwrap();
        assert_eq!(d.runs.len(), 2);
        assert_eq!(d.outputs.len(), 2);
        let today = d.runs.iter().find(|r| r.id == "today").expect("today");
        assert_eq!(
            (today.prompt.as_str(), today.refs[0]["role"].as_str()),
            ("a lighthouse at dusk", Some("reference")),
            "returned as it was before it became a stub"
        );
        assert_eq!(today.created_at, day_start + 3_600_000);

        let since = store.creation_runs_since(day_start).await.unwrap();
        assert_eq!(since.len(), 1);
        assert_eq!(
            (since[0].prompt.as_str(), since[0].cost_usd),
            ("", Some(9.5))
        );
        let stub = &since[0];
        assert_eq!(stub.id, "today");
        assert_eq!(
            (stub.params.clone(), stub.refs.clone()),
            (json!({}), json!([]))
        );
        assert_eq!(stub.upstream, None);
        assert!(stub.deleted_at.is_some());
        assert!(stub.outputs.is_empty(), "a stub's outputs are deleted");
        // What spending needs is kept.
        assert_eq!(stub.status, CreationStatus::Done);
        assert_eq!(stub.kind, CreationKind::Image);
        assert_eq!(stub.estimate_usd, Some(0.04));
        assert_eq!(stub.error, Some(json!({"charged": "yes"})));
        assert_eq!(stub.audit_id, Some(AuditId(3)));
        assert_eq!(
            (
                stub.provider_name.as_str(),
                stub.model.as_str(),
                stub.model_label.as_str()
            ),
            ("OpenRouter", "acme/picture-1", "Picture One")
        );
        assert_eq!(
            (stub.created_at, stub.finished_at),
            (day_start + 3_600_000, Some(day_start + 3_700_000))
        );

        assert!(store.get_creation_run("today").await.unwrap().is_none());
        assert!(
            store
                .list_creation_runs(CreationQuery::default())
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .get_creation_output("today-0")
                .await
                .unwrap()
                .is_none(),
            "the stub's output row is gone"
        );
        assert!(
            store.creation_ref_hashes().await.unwrap().is_empty(),
            "a stub uses no added pictures"
        );
        assert_eq!(
            store.creation_usage().await.unwrap(),
            CreationUsage::default()
        );
        // A stub is not deleted twice and cannot gain outputs.
        let again = store
            .delete_creation_runs(vec!["today".into()], day_start)
            .await
            .unwrap();
        assert_eq!((again.runs.len(), again.outputs.len()), (0, 0));
        assert!(matches!(
            store
                .insert_creation_output(output("today-1", "today", 1))
                .await,
            Err(StoreError::InvalidInput(_))
        ));

        assert_eq!(
            store.purge_creation_stubs(day_start).await.unwrap(),
            0,
            "still today"
        );
        assert_eq!(
            store
                .purge_creation_stubs(day_start + DAY_MS)
                .await
                .unwrap(),
            1
        );
        assert!(store.creation_runs_since(0).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_local_stub_keeps_where_it_ran_and_drops_the_engine_facts() {
        let store = Store::open_in_memory().expect("open");
        insert_done(
            &store,
            NewCreationRun {
                runs_on: RunsOn::Local,
                provider_name: "This computer".into(),
                provider_host: String::new(),
                gen_model_id: Some("flux2-klein-4b-q4".into()),
                engine_build: Some(1106),
                device: Some("Processor".into()),
                ..run("local")
            },
            1,
        )
        .await;
        store
            .update_creation_run(
                "local",
                CreationPatch {
                    elapsed_ms: Some(Some(228_000)),
                    cost_usd: Some(Some(0.0)),
                    ..CreationPatch::default()
                },
            )
            .await
            .expect("update");
        store
            .delete_creation_runs(vec!["local".into()], DAY_START)
            .await
            .expect("delete");
        let since = store.creation_runs_since(DAY_START).await.expect("since");
        assert_eq!(since.len(), 1);
        let stub = &since[0];
        assert_eq!(stub.runs_on, RunsOn::Local);
        assert_eq!(
            (
                stub.gen_model_id.as_deref(),
                stub.device.as_deref(),
                stub.elapsed_ms
            ),
            (None, None, None)
        );
        assert_eq!(stub.cost_usd, Some(0.0));
    }

    #[tokio::test]
    async fn a_patch_to_a_stub_takes_the_cost_and_keeps_the_cleared_fields_cleared() {
        let store = Store::open_in_memory().expect("open");
        insert_done(
            &store,
            NewCreationRun {
                device: Some("Processor".into()),
                ..run("r1")
            },
            1,
        )
        .await;
        store
            .update_creation_run(
                "r1",
                CreationPatch {
                    upstream: Some(Some("Acme Cloud".into())),
                    elapsed_ms: Some(Some(1_000)),
                    ..CreationPatch::default()
                },
            )
            .await
            .expect("update");
        store
            .delete_creation_runs(vec!["r1".into()], DAY_START)
            .await
            .expect("delete");

        // A late answer arrives after the delete.
        store
            .update_creation_run(
                "r1",
                CreationPatch {
                    status: Some(CreationStatus::Failed),
                    cost_usd: Some(Some(0.07)),
                    upstream: Some(Some("Other Cloud".into())),
                    device: Some(Some("Radeon RX 7600 (Vulkan)".into())),
                    elapsed_ms: Some(Some(228_000)),
                    ..CreationPatch::default()
                },
            )
            .await
            .expect("a stub can still be patched");
        let since = store.creation_runs_since(DAY_START).await.expect("since");
        assert_eq!(since.len(), 1);
        let stub = &since[0];
        assert!(stub.deleted_at.is_some(), "still a stub");
        assert_eq!(
            (stub.status, stub.cost_usd),
            (CreationStatus::Failed, Some(0.07)),
            "spending sees the late cost and status"
        );
        assert_eq!(
            (
                stub.upstream.as_deref(),
                stub.device.as_deref(),
                stub.elapsed_ms
            ),
            (None, None, None),
            "a stub keeps these cleared"
        );

        // A patch of only those fields is accepted and changes nothing.
        store
            .update_creation_run(
                "r1",
                CreationPatch {
                    upstream: Some(Some("Other Cloud".into())),
                    ..CreationPatch::default()
                },
            )
            .await
            .expect("accepted");
        let stub = store
            .creation_runs_since(DAY_START)
            .await
            .expect("since")
            .pop()
            .expect("stub");
        assert_eq!(stub.upstream, None);
    }

    #[tokio::test]
    async fn outputs_can_be_starred_and_given_a_thumbnail() {
        let store = Store::open_in_memory().expect("open");
        store.insert_creation_run(run("r1")).await.expect("insert");
        store
            .insert_creation_output(NewCreationOutput {
                mime: "video/webm".into(),
                duration_ms: Some(2_000),
                seed: Some(u64::from(u32::MAX) + 7),
                ..output("o1", "r1", 0)
            })
            .await
            .expect("output");
        let o = store
            .get_creation_output("o1")
            .await
            .expect("get")
            .expect("exists");
        assert_eq!(
            (o.run_id.as_str(), o.idx, o.mime.as_str(), o.size_bytes),
            ("r1", 0, "video/webm", 1_000)
        );
        assert_eq!(
            (o.width, o.height, o.duration_ms),
            (Some(1024), Some(1024), Some(2_000))
        );
        assert_eq!(o.seed, Some(u64::from(u32::MAX) + 7));
        assert_eq!((o.starred, o.thumb_path), (false, None));
        assert_eq!(o.created_at, DAY_START + 2_000);

        store.set_creation_starred("o1", true).await.expect("star");
        store
            .set_creation_thumb("o1", "/g/.thumbs/o1.jpg")
            .await
            .expect("thumb");
        let o = store
            .get_creation_output("o1")
            .await
            .expect("get")
            .expect("exists");
        assert!(o.starred);
        assert_eq!(o.thumb_path.as_deref(), Some("/g/.thumbs/o1.jpg"));
        store
            .set_creation_starred("o1", false)
            .await
            .expect("unstar");
        assert!(
            !store
                .get_creation_output("o1")
                .await
                .expect("get")
                .expect("exists")
                .starred
        );

        assert!(matches!(
            store.set_creation_starred("nope", true).await,
            Err(StoreError::InvalidInput(_))
        ));
        assert!(matches!(
            store.set_creation_thumb("nope", "/t.jpg").await,
            Err(StoreError::InvalidInput(_))
        ));
        assert!(matches!(
            store
                .insert_creation_output(output("o2", "no-such-run", 0))
                .await,
            Err(StoreError::InvalidInput(_))
        ));
        assert!(matches!(
            store.insert_creation_output(output("o1", "r1", 1)).await,
            Err(StoreError::Conflict(_))
        ));
    }

    #[tokio::test]
    async fn usage_and_ref_hashes_count_what_runs_still_hold() {
        let store = Store::open_in_memory().expect("open");
        let a = "aa".repeat(32);
        let b = "bb".repeat(32);
        insert_done(
            &store,
            NewCreationRun {
                refs: json!([{"sha256": a, "role": "reference"}, {"sha256": b, "role": "first"}]),
                ..run("pics")
            },
            2,
        )
        .await;
        store
            .insert_creation_run(NewCreationRun {
                kind: CreationKind::Video,
                refs: json!([{"sha256": a, "role": "first"}, {"role": "no hash"}, "odd"]),
                ..run("vid")
            })
            .await
            .expect("insert");
        store
            .insert_creation_output(NewCreationOutput {
                mime: "video/mp4".into(),
                size_bytes: 5_000,
                ..output("vid-0", "vid", 0)
            })
            .await
            .expect("output");

        assert_eq!(
            store.creation_usage().await.expect("usage"),
            CreationUsage {
                pictures: 2,
                videos: 1,
                bytes: 7_000
            }
        );
        assert_eq!(
            store.creation_ref_hashes().await.expect("refs"),
            BTreeSet::from([a.clone(), b.clone()])
        );

        store
            .delete_creation_runs(vec!["pics".into()], i64::MAX)
            .await
            .expect("delete");
        assert_eq!(
            store.creation_ref_hashes().await.expect("refs"),
            BTreeSet::from([a])
        );
        assert_eq!(
            store.creation_usage().await.expect("usage"),
            CreationUsage {
                pictures: 0,
                videos: 1,
                bytes: 5_000
            }
        );
    }

    #[tokio::test]
    async fn the_cap_is_twenty_thousand_runs() {
        assert_eq!(MAX_CREATION_RUNS, 20_000);
        let store = Store::open_in_memory().expect("open");
        store
            .call(|c| {
                Ok(c.execute(
                    "WITH RECURSIVE n(i) AS \
                     (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 20000) \
                     INSERT INTO creation_runs (id, kind, provider_name, provider_host, model, \
                         model_label, prompt, params_json, status, created_at) \
                     SELECT 'bulk-' || i, 'image', 'p', 'h', 'm', 'M', '', '{}', 'done', i FROM n",
                    [],
                )?)
            })
            .await
            .expect("bulk insert");
        assert!(matches!(
            store.insert_creation_run(run("one-more")).await,
            Err(StoreError::TooManyCreations)
        ));
        assert!(
            store
                .get_creation_run("one-more")
                .await
                .expect("get")
                .is_none()
        );

        // A spending stub does not count toward the cap.
        store
            .delete_creation_runs(vec!["bulk-1".into()], 0)
            .await
            .expect("stub");
        store
            .insert_creation_run(run("one-more"))
            .await
            .expect("room again");
    }
}
