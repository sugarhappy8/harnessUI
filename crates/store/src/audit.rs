//! The audit log (ARCHITECTURE.md section 13.7): one append-only row per tool call or
//! held-navigation decision, viewable with filters and exportable as JSON Lines or CSV.
//!
//! Append-only is enforced twice: this module offers no update or delete except
//! [`Store::complete_audit`], which fills the result of a row that has none, and SQLite
//! triggers refuse any other change. Both are properties of the app's own code; a process
//! the user allowed to run can still edit the database file (section 13.5).

use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, Row, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Store;
use crate::error::StoreError;
use crate::ids::{
    AuditId, ChatId, JsonColumn, json_text, now_ms, sql_enum, sql_limit, sql_opt_u64,
};

sql_enum!(
    /// How a call was decided (section 13.7).
    AuditDecision, "audit decision" {
        /// The user clicked Allow once.
        AllowOnce => "allow_once",
        /// The user clicked Allow for this chat (creating a grant).
        AllowForChat => "allow_for_chat",
        /// An existing grant covered the call.
        AllowedByGrant => "allowed_by_grant",
        /// The tool needs no approval (skill reads, decision D11).
        AllowedWithoutPrompt => "allowed_without_prompt",
        /// The chat was in Allow all and the action is not one that still asks (0.0.4).
        AllowedByMode => "allowed_by_mode",
        /// The user clicked Deny.
        Denied => "denied",
        /// The user pressed Stop, or the approval timed out.
        Cancelled => "cancelled",
        /// A hard limit refused the call without a prompt.
        RefusedByPolicy => "refused_by_policy",
    }
);

/// The outcome of a call, recorded once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditResult {
    /// E.g. "ok", "error", "cancelled", "not_run".
    pub status: String,
    /// Short result text.
    pub summary: Option<String>,
    /// How long the call ran.
    pub duration_ms: Option<u64>,
}

/// Input for [`Store::append_audit`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Unix ms; `None` means now.
    pub ts: Option<i64>,
    /// The chat the call came from, if any.
    pub chat_id: Option<ChatId>,
    /// MCP server name.
    pub server: String,
    /// Tool name.
    pub tool: String,
    /// Full arguments.
    pub arguments: Value,
    /// The action preview shown on the approval card.
    pub preview: String,
    /// Risk level as named by the policy engine.
    pub risk: String,
    /// The decision.
    pub decision: AuditDecision,
    /// Who decided, e.g. "user", "grant", "policy".
    pub decided_by: String,
    /// The user's deny note, if any.
    pub note: Option<String>,
    /// The call this decision belongs to (held navigations link to their call).
    pub parent_audit_id: Option<AuditId>,
    /// The result, when it is already known (e.g. a refused call that never runs).
    pub result: Option<AuditResult>,
}

/// A row of `audit_log`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditRow {
    /// Row id.
    pub id: AuditId,
    /// Unix ms.
    pub ts: i64,
    /// The chat, if any. Kept after the chat is deleted.
    pub chat_id: Option<ChatId>,
    /// MCP server name.
    pub server: String,
    /// Tool name.
    pub tool: String,
    /// Full arguments.
    pub arguments: Value,
    /// Action preview.
    pub preview: String,
    /// Risk level.
    pub risk: String,
    /// The decision.
    pub decision: AuditDecision,
    /// Who decided.
    pub decided_by: String,
    /// Deny note.
    pub note: Option<String>,
    /// Result status; `None` while the call runs.
    pub result_status: Option<String>,
    /// Result summary.
    pub result_summary: Option<String>,
    /// Duration of the call.
    pub duration_ms: Option<u64>,
    /// The originating call's row.
    pub parent_audit_id: Option<AuditId>,
}

/// Filters for [`Store::list_audit`] and the exports. Unset fields do not filter.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditQuery {
    /// Only this chat.
    pub chat_id: Option<ChatId>,
    /// Only this server.
    pub server: Option<String>,
    /// Only this tool.
    pub tool: Option<String>,
    /// Only this decision.
    pub decision: Option<AuditDecision>,
    /// Only rows at or after this Unix ms.
    pub since: Option<i64>,
    /// Only rows before this Unix ms.
    pub until: Option<i64>,
    /// Maximum rows (the newest ones); 0 means no limit.
    pub limit: u32,
}

const AUDIT_COLUMNS: &str = "id, ts, chat_id, server, tool, arguments, preview, risk, decision, \
     decided_by, note, result_status, result_summary, duration_ms, parent_audit_id";

fn audit_from_row(row: &Row<'_>) -> rusqlite::Result<AuditRow> {
    Ok(AuditRow {
        id: row.get(0)?,
        ts: row.get(1)?,
        chat_id: row.get(2)?,
        server: row.get(3)?,
        tool: row.get(4)?,
        arguments: row.get::<_, JsonColumn>(5)?.0,
        preview: row.get(6)?,
        risk: row.get(7)?,
        decision: row.get(8)?,
        decided_by: row.get(9)?,
        note: row.get(10)?,
        result_status: row.get(11)?,
        result_summary: row.get(12)?,
        duration_ms: row.get(13)?,
        parent_audit_id: row.get(14)?,
    })
}

fn append_audit(conn: &Connection, entry: AuditEntry) -> Result<AuditId, StoreError> {
    if let Some(parent) = entry.parent_audit_id {
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM audit_log WHERE id = ?1)",
            [parent],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(StoreError::not_found("audit row", parent.0));
        }
    }
    let (status, summary, duration) = match entry.result {
        Some(r) => (Some(r.status), r.summary, r.duration_ms),
        None => (None, None, None),
    };
    let duration = sql_opt_u64("duration_ms", duration)?;
    conn.execute(
        "INSERT INTO audit_log (ts, chat_id, server, tool, arguments, preview, risk, decision, \
         decided_by, note, result_status, result_summary, duration_ms, parent_audit_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            entry.ts.unwrap_or_else(now_ms),
            entry.chat_id,
            entry.server,
            entry.tool,
            json_text(&entry.arguments)?,
            entry.preview,
            entry.risk,
            entry.decision,
            entry.decided_by,
            entry.note,
            status,
            summary,
            duration,
            entry.parent_audit_id
        ],
    )?;
    Ok(AuditId(conn.last_insert_rowid()))
}

fn complete_audit(
    conn: &Connection,
    id: AuditId,
    status: String,
    summary: Option<String>,
    duration_ms: Option<u64>,
) -> Result<(), StoreError> {
    let duration_ms = sql_opt_u64("duration_ms", duration_ms)?;
    let n = conn.execute(
        "UPDATE audit_log SET result_status = ?1, \
         result_summary = coalesce(result_summary, ?2), duration_ms = coalesce(duration_ms, ?3) \
         WHERE id = ?4 AND result_status IS NULL",
        params![status, summary, duration_ms, id],
    )?;
    if n == 1 {
        return Ok(());
    }
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM audit_log WHERE id = ?1)",
        [id],
        |r| r.get(0),
    )?;
    if exists {
        Err(StoreError::AuditAlreadyCompleted(id))
    } else {
        Err(StoreError::not_found("audit row", id.0))
    }
}

fn list_audit(conn: &Connection, q: &AuditQuery) -> Result<Vec<AuditRow>, StoreError> {
    let mut sql = format!("SELECT {AUDIT_COLUMNS} FROM audit_log WHERE 1 = 1");
    let mut values: Vec<SqlValue> = Vec::new();
    if let Some(chat) = q.chat_id {
        sql.push_str(" AND chat_id = ?");
        values.push(SqlValue::Integer(chat.0));
    }
    if let Some(server) = &q.server {
        sql.push_str(" AND server = ?");
        values.push(SqlValue::Text(server.clone()));
    }
    if let Some(tool) = &q.tool {
        sql.push_str(" AND tool = ?");
        values.push(SqlValue::Text(tool.clone()));
    }
    if let Some(decision) = q.decision {
        sql.push_str(" AND decision = ?");
        values.push(SqlValue::Text(decision.as_str().to_owned()));
    }
    if let Some(since) = q.since {
        sql.push_str(" AND ts >= ?");
        values.push(SqlValue::Integer(since));
    }
    if let Some(until) = q.until {
        sql.push_str(" AND ts < ?");
        values.push(SqlValue::Integer(until));
    }
    sql.push_str(" ORDER BY ts DESC, id DESC LIMIT ?");
    values.push(SqlValue::Integer(sql_limit(q.limit)));
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(values), audit_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// RFC 3339 UTC time for a Unix ms timestamp; empty if out of range.
fn rfc3339(ts: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ts)
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .unwrap_or_default()
}

/// One JSON object per line, oldest first. `arguments` stays structured JSON; `time` is
/// `ts` as RFC 3339 UTC.
pub fn audit_rows_to_jsonl(rows: &[AuditRow]) -> Result<String, StoreError> {
    let mut out = String::new();
    for row in rows.iter().rev() {
        let mut obj = serde_json::to_value(row)?;
        if let Value::Object(map) = &mut obj {
            map.insert("time".into(), Value::String(rfc3339(row.ts)));
        }
        out.push_str(&serde_json::to_string(&obj)?);
        out.push('\n');
    }
    Ok(out)
}

/// The CSV header row, in column order.
pub const AUDIT_CSV_COLUMNS: &[&str] = &[
    "id",
    "ts",
    "time",
    "chat_id",
    "server",
    "tool",
    "arguments",
    "preview",
    "risk",
    "decision",
    "decided_by",
    "note",
    "result_status",
    "result_summary",
    "duration_ms",
    "parent_audit_id",
];

/// RFC 4180 CSV (CRLF line ends, header row), oldest first.
///
/// Fields holding a comma, quote, CR or LF are quoted with inner quotes doubled. Text
/// fields that a spreadsheet would read as a formula (leading `=`, `+`, `-`, `@`, tab or
/// CR) get a leading `'`, because tool arguments and previews are chosen by the model.
pub fn audit_rows_to_csv(rows: &[AuditRow]) -> Result<String, StoreError> {
    let mut out = String::new();
    push_record(&mut out, AUDIT_CSV_COLUMNS.iter().map(|c| (*c).to_owned()));
    for r in rows.iter().rev() {
        let num = |v: Option<i64>| v.map(|n| n.to_string()).unwrap_or_default();
        let text = |v: &str| defuse_formula(v);
        let opt = |v: &Option<String>| v.as_deref().map(defuse_formula).unwrap_or_default();
        push_record(
            &mut out,
            [
                r.id.0.to_string(),
                r.ts.to_string(),
                rfc3339(r.ts),
                num(r.chat_id.map(|c| c.0)),
                text(&r.server),
                text(&r.tool),
                text(&serde_json::to_string(&r.arguments)?),
                text(&r.preview),
                text(&r.risk),
                r.decision.as_str().to_owned(),
                text(&r.decided_by),
                opt(&r.note),
                opt(&r.result_status),
                opt(&r.result_summary),
                r.duration_ms.map(|d| d.to_string()).unwrap_or_default(),
                num(r.parent_audit_id.map(|p| p.0)),
            ]
            .into_iter(),
        );
    }
    Ok(out)
}

/// Prefixes `'` to text a spreadsheet would evaluate as a formula (OWASP CSV injection).
fn defuse_formula(value: &str) -> String {
    if value.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{value}")
    } else {
        value.to_owned()
    }
}

/// Appends one CSV record (quoted where needed) and a CRLF.
fn push_record(out: &mut String, fields: impl Iterator<Item = String>) {
    for (i, field) in fields.enumerate() {
        if i > 0 {
            out.push(',');
        }
        if field.contains([',', '"', '\n', '\r']) {
            out.push('"');
            out.push_str(&field.replace('"', "\"\""));
            out.push('"');
        } else {
            out.push_str(&field);
        }
    }
    out.push_str("\r\n");
}

impl Store {
    /// Appends an audit row and returns its id. If `entry.result` is `None`, the result is
    /// filled in later with [`Store::complete_audit`].
    pub async fn append_audit(&self, entry: AuditEntry) -> Result<AuditId, StoreError> {
        self.call(move |c| append_audit(c, entry)).await
    }

    /// Records the result of a call whose row has none yet. Summary and duration are set
    /// only where still empty. A row that already has a result is never changed:
    /// the call fails with [`StoreError::AuditAlreadyCompleted`].
    pub async fn complete_audit(
        &self,
        id: AuditId,
        result_status: String,
        result_summary: Option<String>,
        duration_ms: Option<u64>,
    ) -> Result<(), StoreError> {
        self.call(move |c| complete_audit(c, id, result_status, result_summary, duration_ms))
            .await
    }

    /// Audit rows matching `query`, newest first.
    pub async fn list_audit(&self, query: AuditQuery) -> Result<Vec<AuditRow>, StoreError> {
        self.call(move |c| list_audit(c, &query)).await
    }

    /// The rows matching `query` as JSON Lines, oldest first.
    pub async fn export_audit_jsonl(&self, query: AuditQuery) -> Result<String, StoreError> {
        let rows = self.list_audit(query).await?;
        audit_rows_to_jsonl(&rows)
    }

    /// The rows matching `query` as CSV, oldest first (see [`audit_rows_to_csv`]).
    pub async fn export_audit_csv(&self, query: AuditQuery) -> Result<String, StoreError> {
        let rows = self.list_audit(query).await?;
        audit_rows_to_csv(&rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(tool: &str, decision: AuditDecision) -> AuditEntry {
        AuditEntry {
            ts: None,
            chat_id: Some(ChatId(1)),
            server: "files".into(),
            tool: tool.into(),
            arguments: json!({ "path": "/home/me/a.txt" }),
            preview: "Read /home/me/a.txt".into(),
            risk: "read".into(),
            decision,
            decided_by: "user".into(),
            note: None,
            parent_audit_id: None,
            result: None,
        }
    }

    #[tokio::test]
    async fn append_complete_and_refuse_second_complete() {
        let store = Store::open_in_memory().expect("open");
        let id = store
            .append_audit(entry("read_file", AuditDecision::AllowOnce))
            .await
            .expect("append");
        store
            .complete_audit(id, "ok".into(), Some("3 lines".into()), Some(12))
            .await
            .expect("complete");
        let err = store
            .complete_audit(id, "error".into(), Some("rewritten".into()), Some(99))
            .await
            .expect_err("second complete");
        assert!(matches!(err, StoreError::AuditAlreadyCompleted(i) if i == id));
        let rows = store.list_audit(AuditQuery::default()).await.expect("list");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].result_status.as_deref(), Some("ok"));
        assert_eq!(rows[0].result_summary.as_deref(), Some("3 lines"));
        assert_eq!(rows[0].duration_ms, Some(12));

        let err = store
            .complete_audit(AuditId(999), "ok".into(), None, None)
            .await
            .expect_err("missing");
        assert!(matches!(err, StoreError::NotFound { .. }));
    }

    #[tokio::test]
    async fn oversized_durations_are_refused_and_leave_the_row_open() {
        let store = Store::open_in_memory().expect("open");
        let id = store
            .append_audit(entry("read_file", AuditDecision::AllowOnce))
            .await
            .expect("append");
        let err = store
            .complete_audit(id, "ok".into(), None, Some(u64::MAX))
            .await
            .expect_err("too large for SQLite");
        assert!(matches!(err, StoreError::InvalidInput(_)), "{err:?}");
        // The failed call did not consume the one permitted completion.
        store
            .complete_audit(id, "ok".into(), None, Some(5))
            .await
            .expect("complete");
        let mut e = entry("read_file", AuditDecision::RefusedByPolicy);
        e.result = Some(AuditResult {
            status: "not_run".into(),
            summary: None,
            duration_ms: Some(u64::MAX),
        });
        assert!(matches!(
            store.append_audit(e).await,
            Err(StoreError::InvalidInput(_))
        ));
    }

    #[tokio::test]
    async fn triggers_block_rewrites_and_deletes() {
        let store = Store::open_in_memory().expect("open");
        let id = store
            .append_audit(entry("write_file", AuditDecision::Denied))
            .await
            .expect("append");
        let (update, delete) = store
            .call(move |c| {
                let update = c.execute(
                    "UPDATE audit_log SET decision = 'allow_once' WHERE id = ?1",
                    [id],
                );
                let delete = c.execute("DELETE FROM audit_log WHERE id = ?1", [id]);
                // REPLACE deletes the conflicting row; with recursive_triggers (set at
                // open) that deletion hits the append-only trigger.
                let replace = c.execute(
                    "INSERT OR REPLACE INTO audit_log (id, ts, server, tool, arguments, preview, \
                     risk, decision, decided_by) \
                     VALUES (?1, 0, 'files', 'write_file', '{}', 'p', 'write', 'allow_once', 'user')",
                    [id],
                );
                Ok((update.is_err(), delete.is_err() && replace.is_err()))
            })
            .await
            .expect("raw sql");
        assert!(update, "rewriting a decision must fail");
        assert!(delete, "deleting or replacing must fail");
        let rows = store.list_audit(AuditQuery::default()).await.expect("list");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].decision, AuditDecision::Denied);
    }

    #[tokio::test]
    async fn result_known_at_append_and_parent_links() {
        let store = Store::open_in_memory().expect("open");
        let parent = store
            .append_audit(entry("browser_click", AuditDecision::AllowedByGrant))
            .await
            .expect("parent");
        let mut held = entry("navigation", AuditDecision::RefusedByPolicy);
        held.parent_audit_id = Some(parent);
        held.result = Some(AuditResult {
            status: "not_run".into(),
            summary: None,
            duration_ms: None,
        });
        let child = store.append_audit(held).await.expect("child");
        let err = store
            .complete_audit(child, "ok".into(), None, None)
            .await
            .expect_err("already has result");
        assert!(matches!(err, StoreError::AuditAlreadyCompleted(_)));
        let mut orphan = entry("navigation", AuditDecision::Denied);
        orphan.parent_audit_id = Some(AuditId(4242));
        assert!(matches!(
            store.append_audit(orphan).await,
            Err(StoreError::NotFound { .. })
        ));
        let rows = store.list_audit(AuditQuery::default()).await.expect("list");
        assert_eq!(rows[0].parent_audit_id, Some(parent));
    }

    #[tokio::test]
    async fn filters_and_order() {
        let store = Store::open_in_memory().expect("open");
        for (i, (tool, decision, chat)) in [
            ("read_file", AuditDecision::AllowOnce, 1),
            ("write_file", AuditDecision::Denied, 1),
            ("read_file", AuditDecision::AllowedByGrant, 2),
        ]
        .into_iter()
        .enumerate()
        {
            let mut e = entry(tool, decision);
            e.ts = Some(1_000 + i64::try_from(i).expect("small"));
            e.chat_id = Some(ChatId(chat));
            store.append_audit(e).await.expect("append");
        }
        let all = store.list_audit(AuditQuery::default()).await.expect("all");
        assert_eq!(
            all.iter().map(|r| r.ts).collect::<Vec<_>>(),
            vec![1002, 1001, 1000]
        );
        let reads = store
            .list_audit(AuditQuery {
                tool: Some("read_file".into()),
                ..AuditQuery::default()
            })
            .await
            .expect("tool");
        assert_eq!(reads.len(), 2);
        let denied = store
            .list_audit(AuditQuery {
                decision: Some(AuditDecision::Denied),
                ..AuditQuery::default()
            })
            .await
            .expect("decision");
        assert_eq!(denied.len(), 1);
        let chat2 = store
            .list_audit(AuditQuery {
                chat_id: Some(ChatId(2)),
                ..AuditQuery::default()
            })
            .await
            .expect("chat");
        assert_eq!(chat2.len(), 1);
        let recent = store
            .list_audit(AuditQuery {
                since: Some(1001),
                limit: 1,
                ..AuditQuery::default()
            })
            .await
            .expect("since");
        assert_eq!(recent.iter().map(|r| r.ts).collect::<Vec<_>>(), vec![1002]);
        let window = store
            .list_audit(AuditQuery {
                since: Some(1000),
                until: Some(1002),
                ..AuditQuery::default()
            })
            .await
            .expect("window");
        assert_eq!(window.len(), 2);
    }

    #[tokio::test]
    async fn csv_quotes_commas_quotes_and_newlines() {
        let store = Store::open_in_memory().expect("open");
        let mut e = entry("run_command", AuditDecision::AllowOnce);
        e.ts = Some(1_700_000_000_000);
        e.preview = "echo \"hi, there\"\nsecond line".into();
        e.note = Some("=HYPERLINK(\"http://x\")".into());
        e.arguments = json!({ "cmd": "a,b" });
        store.append_audit(e).await.expect("append");

        let csv = store
            .export_audit_csv(AuditQuery::default())
            .await
            .expect("csv");
        let mut lines = csv.split("\r\n");
        assert_eq!(lines.next(), Some(AUDIT_CSV_COLUMNS.join(",").as_str()));
        assert!(csv.ends_with("\r\n"));
        assert!(
            csv.contains("\"echo \"\"hi, there\"\"\nsecond line\""),
            "{csv}"
        );
        assert!(csv.contains("\"{\"\"cmd\"\":\"\"a,b\"\"}\""), "{csv}");
        assert!(csv.contains("\"'=HYPERLINK(\"\"http://x\"\")\""), "{csv}");
        assert!(csv.contains(",2023-11-14T22:13:20.000Z,"), "{csv}");

        // Parse it back with a minimal RFC 4180 reader: one data record of 16 fields.
        let records = parse_csv(&csv);
        assert_eq!(records.len(), 2);
        assert_eq!(records[1].len(), AUDIT_CSV_COLUMNS.len());
        assert_eq!(records[1][7], "echo \"hi, there\"\nsecond line");
        assert_eq!(records[1][6], "{\"cmd\":\"a,b\"}");
        assert_eq!(records[1][12], "", "no result yet");
    }

    #[tokio::test]
    async fn jsonl_has_one_object_per_line_oldest_first() {
        let store = Store::open_in_memory().expect("open");
        for ts in [5_000, 6_000] {
            let mut e = entry("read_file", AuditDecision::AllowOnce);
            e.ts = Some(ts);
            e.preview = "multi\nline".into();
            store.append_audit(e).await.expect("append");
        }
        let jsonl = store
            .export_audit_jsonl(AuditQuery::default())
            .await
            .expect("jsonl");
        let lines: Vec<&str> = jsonl.lines().collect();
        assert_eq!(lines.len(), 2);
        let first: Value = serde_json::from_str(lines[0]).expect("json");
        assert_eq!(first["ts"], json!(5_000));
        assert_eq!(first["arguments"]["path"], json!("/home/me/a.txt"));
        assert_eq!(first["decision"], json!("allow_once"));
        assert_eq!(first["time"], json!("1970-01-01T00:00:05.000Z"));
        assert_eq!(first["preview"], json!("multi\nline"));
    }

    /// Minimal RFC 4180 parser for the round-trip test.
    fn parse_csv(input: &str) -> Vec<Vec<String>> {
        let mut records = Vec::new();
        let mut record = Vec::new();
        let mut field = String::new();
        let mut chars = input.chars().peekable();
        let mut quoted = false;
        while let Some(c) = chars.next() {
            if quoted {
                if c == '"' {
                    if chars.peek() == Some(&'"') {
                        field.push('"');
                        chars.next();
                    } else {
                        quoted = false;
                    }
                } else {
                    field.push(c);
                }
            } else {
                match c {
                    '"' => quoted = true,
                    ',' => record.push(std::mem::take(&mut field)),
                    '\r' if chars.peek() == Some(&'\n') => {
                        chars.next();
                        record.push(std::mem::take(&mut field));
                        records.push(std::mem::take(&mut record));
                    }
                    _ => field.push(c),
                }
            }
        }
        records
    }

    #[test]
    fn formula_prefixes() {
        assert_eq!(defuse_formula("=1+1"), "'=1+1");
        assert_eq!(defuse_formula("-rf"), "'-rf");
        assert_eq!(defuse_formula("plain"), "plain");
        assert_eq!(defuse_formula(""), "");
    }
}
