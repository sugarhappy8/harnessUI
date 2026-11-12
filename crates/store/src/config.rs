//! Configuration tables: settings, providers, approved folders, MCP servers and their tool
//! preferences, and skills. None of them holds a secret value; providers and MCP configs
//! refer to keychain entries through [`SecretRef`].

use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Store;
use crate::chats::ensure_chat;
use crate::error::StoreError;
use crate::ids::{
    ChatId, FolderId, JsonColumn, McpServerId, ProviderId, SkillId, json_text, now_ms,
    sql_opt_text, update_columns,
};
use crate::secrets::SecretRef;

// ---------------------------------------------------------------------------------------
// Settings

fn get_setting_text(conn: &Connection, key: &str) -> Result<Option<String>, StoreError> {
    Ok(conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
            r.get(0)
        })
        .optional()?)
}

/// The most rows [`Store::settings_with_prefix`] returns.
pub const MAX_PREFIX_ROWS: usize = 1000;

/// Settings whose key starts with `prefix`, by key, as stored text. `substr` compares the
/// start of the key exactly: `%` and `_` in a prefix are ordinary characters.
fn settings_with_prefix_text(
    conn: &Connection,
    prefix: &str,
) -> Result<Vec<(String, String)>, StoreError> {
    if prefix.is_empty() {
        return Err(StoreError::InvalidInput(
            "a settings prefix cannot be empty".into(),
        ));
    }
    let mut stmt = conn.prepare(
        "SELECT key, value FROM settings WHERE substr(key, 1, length(?1)) = ?1 \
         ORDER BY key LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![prefix, MAX_PREFIX_ROWS as i64], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?
        .collect::<rusqlite::Result<Vec<(String, String)>>>()?;
    Ok(rows)
}

fn set_setting_text(conn: &Connection, key: &str, value: &str) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        params![key, value, now_ms()],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------------------
// Providers

/// Input for [`Store::upsert_provider`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderInput {
    /// `None` inserts a new provider; `Some` replaces that provider's fields.
    pub id: Option<ProviderId>,
    /// Display name.
    pub name: String,
    /// Kind of endpoint, e.g. "openai_compatible", "ollama", "lm_studio".
    pub kind: String,
    /// Base URL; requests go only here.
    pub base_url: String,
    /// Keychain entry holding the API key, if any.
    pub secret_ref: Option<SecretRef>,
    /// Offered in the model picker.
    pub enabled: bool,
}

/// A row of `providers`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provider {
    /// Row id.
    pub id: ProviderId,
    /// Display name.
    pub name: String,
    /// Kind of endpoint.
    pub kind: String,
    /// Base URL.
    pub base_url: String,
    /// Keychain entry holding the API key.
    pub secret_ref: Option<SecretRef>,
    /// Offered in the model picker.
    pub enabled: bool,
    /// Unix ms.
    pub created_at: i64,
}

const PROVIDER_COLUMNS: &str = "id, name, kind, base_url, secret_ref, enabled, created_at";

fn provider_from_row(r: &Row<'_>) -> rusqlite::Result<Provider> {
    Ok(Provider {
        id: r.get(0)?,
        name: r.get(1)?,
        kind: r.get(2)?,
        base_url: r.get(3)?,
        secret_ref: r.get(4)?,
        enabled: r.get(5)?,
        created_at: r.get(6)?,
    })
}

fn upsert_provider(conn: &Connection, input: ProviderInput) -> Result<ProviderId, StoreError> {
    match input.id {
        None => {
            conn.execute(
                "INSERT INTO providers (name, kind, base_url, secret_ref, enabled, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    input.name,
                    input.kind,
                    input.base_url,
                    input.secret_ref,
                    input.enabled,
                    now_ms()
                ],
            )?;
            Ok(ProviderId(conn.last_insert_rowid()))
        }
        Some(id) => {
            let n = conn.execute(
                "UPDATE providers SET name = ?1, kind = ?2, base_url = ?3, secret_ref = ?4, \
                 enabled = ?5 WHERE id = ?6",
                params![
                    input.name,
                    input.kind,
                    input.base_url,
                    input.secret_ref,
                    input.enabled,
                    id
                ],
            )?;
            if n == 0 {
                Err(StoreError::not_found("provider", id.0))
            } else {
                Ok(id)
            }
        }
    }
}

fn list_providers(conn: &Connection) -> Result<Vec<Provider>, StoreError> {
    let sql = format!("SELECT {PROVIDER_COLUMNS} FROM providers ORDER BY id");
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map([], provider_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn get_provider(conn: &Connection, id: ProviderId) -> Result<Option<Provider>, StoreError> {
    let sql = format!("SELECT {PROVIDER_COLUMNS} FROM providers WHERE id = ?1");
    Ok(conn.query_row(&sql, [id], provider_from_row).optional()?)
}

/// Deletes a row by id and returns whether it existed.
fn delete_by_id(conn: &Connection, table: &'static str, id: i64) -> Result<bool, StoreError> {
    Ok(conn.execute(&format!("DELETE FROM {table} WHERE id = ?1"), [id])? > 0)
}

// ---------------------------------------------------------------------------------------
// Approved folders

/// A row of `approved_folders`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovedFolder {
    /// Row id.
    pub id: FolderId,
    /// The folder, as the user approved it (canonicalised by the caller).
    pub path: String,
    /// `None` for a global folder, otherwise the chat it is approved for.
    pub chat_id: Option<ChatId>,
    /// Unix ms.
    pub created_at: i64,
}

fn folder_from_row(r: &Row<'_>) -> rusqlite::Result<ApprovedFolder> {
    Ok(ApprovedFolder {
        id: r.get(0)?,
        path: r.get(1)?,
        chat_id: r.get(2)?,
        created_at: r.get(3)?,
    })
}

fn add_approved_folder(
    conn: &Connection,
    path: String,
    chat: Option<ChatId>,
) -> Result<FolderId, StoreError> {
    if path.is_empty() {
        return Err(StoreError::InvalidInput("folder path is empty".into()));
    }
    if let Some(chat) = chat {
        ensure_chat(conn, chat)?;
    }
    let existing: Option<FolderId> = conn
        .query_row(
            "SELECT id FROM approved_folders WHERE path = ?1 AND chat_id IS ?2",
            params![path, chat],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = existing {
        return Ok(id);
    }
    conn.execute(
        "INSERT INTO approved_folders (path, chat_id, created_at) VALUES (?1, ?2, ?3)",
        params![path, chat, now_ms()],
    )?;
    Ok(FolderId(conn.last_insert_rowid()))
}

fn list_approved_folders(conn: &Connection) -> Result<Vec<ApprovedFolder>, StoreError> {
    let mut stmt = conn
        .prepare_cached("SELECT id, path, chat_id, created_at FROM approved_folders ORDER BY id")?;
    let rows = stmt.query_map([], folder_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn approved_folders_for_chat(
    conn: &Connection,
    chat: ChatId,
) -> Result<Vec<ApprovedFolder>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, path, chat_id, created_at FROM approved_folders \
         WHERE chat_id IS NULL OR chat_id = ?1 ORDER BY id",
    )?;
    let rows = stmt.query_map([chat], folder_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

// ---------------------------------------------------------------------------------------
// MCP servers and tool preferences

/// Input for [`Store::upsert_mcp_server`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpServerInput {
    /// Unique server name; the upsert key.
    pub name: String,
    /// Launch or connection config (shape owned by the mcp-host crate; no secret values).
    pub config: Value,
    /// Started with the app.
    pub enabled: bool,
}

/// A row of `mcp_servers`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpServer {
    /// Row id.
    pub id: McpServerId,
    /// Unique name.
    pub name: String,
    /// Launch or connection config.
    pub config: Value,
    /// Started with the app.
    pub enabled: bool,
    /// Unix ms when the user acknowledged the "runs as you" warning for this config.
    pub acknowledged_at: Option<i64>,
    /// Protocol revision family negotiated last time, if known.
    pub protocol_era: Option<String>,
    /// Unix ms.
    pub created_at: i64,
}

const MCP_COLUMNS: &str = "id, name, config, enabled, acknowledged_at, protocol_era, created_at";

fn mcp_from_row(r: &Row<'_>) -> rusqlite::Result<McpServer> {
    Ok(McpServer {
        id: r.get(0)?,
        name: r.get(1)?,
        config: r.get::<_, JsonColumn>(2)?.0,
        enabled: r.get(3)?,
        acknowledged_at: r.get(4)?,
        protocol_era: r.get(5)?,
        created_at: r.get(6)?,
    })
}

fn upsert_mcp_server(conn: &Connection, input: McpServerInput) -> Result<McpServerId, StoreError> {
    if input.name.is_empty() {
        return Err(StoreError::InvalidInput("MCP server name is empty".into()));
    }
    // A changed config clears the acknowledgement: the user must re-confirm what runs.
    conn.execute(
        "INSERT INTO mcp_servers (name, config, enabled, created_at) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(name) DO UPDATE SET \
             enabled = excluded.enabled, \
             acknowledged_at = CASE WHEN config IS excluded.config THEN acknowledged_at ELSE NULL END, \
             config = excluded.config",
        params![input.name, json_text(&input.config)?, input.enabled, now_ms()],
    )?;
    Ok(conn.query_row(
        "SELECT id FROM mcp_servers WHERE name = ?1",
        [&input.name],
        |r| r.get(0),
    )?)
}

fn list_mcp_servers(conn: &Connection) -> Result<Vec<McpServer>, StoreError> {
    let sql = format!("SELECT {MCP_COLUMNS} FROM mcp_servers ORDER BY name");
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map([], mcp_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn get_mcp_server(conn: &Connection, id: McpServerId) -> Result<Option<McpServer>, StoreError> {
    let sql = format!("SELECT {MCP_COLUMNS} FROM mcp_servers WHERE id = ?1");
    Ok(conn.query_row(&sql, [id], mcp_from_row).optional()?)
}

fn set_mcp_server_field(
    conn: &Connection,
    id: McpServerId,
    column: &'static str,
    value: SqlValue,
) -> Result<(), StoreError> {
    if update_columns(conn, "mcp_servers", "id", id.0, vec![(column, value)])? == 0 {
        return Err(StoreError::not_found("MCP server", id.0));
    }
    Ok(())
}

/// Records the acknowledgement only if the server's config is still the one the user saw.
fn acknowledge_mcp_server(
    conn: &Connection,
    id: McpServerId,
    shown_config: &Value,
    at: i64,
) -> Result<(), StoreError> {
    let current: Option<JsonColumn> = conn
        .query_row("SELECT config FROM mcp_servers WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .optional()?;
    let Some(JsonColumn(current)) = current else {
        return Err(StoreError::not_found("MCP server", id.0));
    };
    if &current != shown_config {
        return Err(StoreError::Conflict(format!(
            "MCP server {id} changed since its warning was shown; show it again"
        )));
    }
    set_mcp_server_field(conn, id, "acknowledged_at", SqlValue::Integer(at))
}

/// A row of `mcp_tool_prefs`: the user's switches for one external tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpToolPref {
    /// The server.
    pub server_id: McpServerId,
    /// Tool name as the server reports it.
    pub tool_name: String,
    /// Offered to the model (tools of a new server start off, decision D10).
    pub enabled: bool,
    /// "Allow any arguments" for chat grants (decision D6).
    pub allow_any_args: bool,
    /// Always ask, whatever was granted.
    pub always_ask: bool,
    /// Hash of the tool's description and schema when the user last reviewed it.
    pub fingerprint: Option<String>,
}

fn upsert_mcp_tool_pref(conn: &Connection, pref: McpToolPref) -> Result<(), StoreError> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM mcp_servers WHERE id = ?1)",
        [pref.server_id],
        |r| r.get(0),
    )?;
    if !exists {
        return Err(StoreError::not_found("MCP server", pref.server_id.0));
    }
    conn.execute(
        "INSERT INTO mcp_tool_prefs (server_id, tool_name, enabled, allow_any_args, always_ask, fingerprint) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
         ON CONFLICT(server_id, tool_name) DO UPDATE SET enabled = excluded.enabled, \
             allow_any_args = excluded.allow_any_args, always_ask = excluded.always_ask, \
             fingerprint = excluded.fingerprint",
        params![
            pref.server_id,
            pref.tool_name,
            pref.enabled,
            pref.allow_any_args,
            pref.always_ask,
            pref.fingerprint
        ],
    )?;
    Ok(())
}

fn mcp_tool_prefs(conn: &Connection, server: McpServerId) -> Result<Vec<McpToolPref>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT server_id, tool_name, enabled, allow_any_args, always_ask, fingerprint \
         FROM mcp_tool_prefs WHERE server_id = ?1 ORDER BY tool_name",
    )?;
    let rows = stmt.query_map([server], |r| {
        Ok(McpToolPref {
            server_id: r.get(0)?,
            tool_name: r.get(1)?,
            enabled: r.get(2)?,
            allow_any_args: r.get(3)?,
            always_ask: r.get(4)?,
            fingerprint: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

// ---------------------------------------------------------------------------------------
// Skills

/// Input for [`Store::upsert_skill`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillInput {
    /// Unique skill name; the upsert key.
    pub name: String,
    /// Folder holding the skill's files.
    pub dir: String,
    /// Offered to the model (imported skills start disabled).
    pub enabled: bool,
}

/// A row of `skills`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skill {
    /// Row id.
    pub id: SkillId,
    /// Unique name.
    pub name: String,
    /// Folder.
    pub dir: String,
    /// Offered to the model.
    pub enabled: bool,
    /// Unix ms of the first import.
    pub imported_at: i64,
}

fn upsert_skill(conn: &Connection, input: SkillInput) -> Result<SkillId, StoreError> {
    if input.name.is_empty() {
        return Err(StoreError::InvalidInput("skill name is empty".into()));
    }
    conn.execute(
        "INSERT INTO skills (name, dir, enabled, imported_at) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(name) DO UPDATE SET dir = excluded.dir, enabled = excluded.enabled",
        params![input.name, input.dir, input.enabled, now_ms()],
    )?;
    Ok(conn.query_row(
        "SELECT id FROM skills WHERE name = ?1",
        [&input.name],
        |r| r.get(0),
    )?)
}

fn list_skills(conn: &Connection) -> Result<Vec<Skill>, StoreError> {
    let mut stmt = conn
        .prepare_cached("SELECT id, name, dir, enabled, imported_at FROM skills ORDER BY name")?;
    let rows = stmt.query_map([], |r| {
        Ok(Skill {
            id: r.get(0)?,
            name: r.get(1)?,
            dir: r.get(2)?,
            enabled: r.get(3)?,
            imported_at: r.get(4)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

impl Store {
    /// Reads a setting and deserialises it; `None` if unset.
    pub async fn get_setting<T: DeserializeOwned>(
        &self,
        key: &str,
    ) -> Result<Option<T>, StoreError> {
        let key = key.to_owned();
        let text = self.call(move |c| get_setting_text(c, &key)).await?;
        text.map(|t| serde_json::from_str(&t).map_err(StoreError::from))
            .transpose()
    }

    /// Stores a setting as JSON, replacing any earlier value.
    pub async fn set_setting<T: Serialize + ?Sized>(
        &self,
        key: &str,
        value: &T,
    ) -> Result<(), StoreError> {
        let key = key.to_owned();
        let text = serde_json::to_string(value)?;
        self.call(move |c| set_setting_text(c, &key, &text)).await
    }

    /// 0.0.6: every setting whose key starts with `prefix` (non-empty), in key order, at
    /// most [`MAX_PREFIX_ROWS`]. Callers check each value's shape themselves (the table
    /// holds JSON only; a row that still failed to parse would be left out).
    pub async fn settings_with_prefix(
        &self,
        prefix: &str,
    ) -> Result<Vec<(String, Value)>, StoreError> {
        let prefix = prefix.to_owned();
        let rows = self
            .call(move |c| settings_with_prefix_text(c, &prefix))
            .await?;
        Ok(rows
            .into_iter()
            .filter_map(|(k, v)| serde_json::from_str(&v).ok().map(|v| (k, v)))
            .collect())
    }

    /// Removes a setting; returns whether it existed.
    pub async fn delete_setting(&self, key: &str) -> Result<bool, StoreError> {
        let key = key.to_owned();
        self.call(move |c| Ok(c.execute("DELETE FROM settings WHERE key = ?1", [key])? > 0))
            .await
    }

    /// Inserts (`input.id == None`) or replaces a provider.
    pub async fn upsert_provider(&self, input: ProviderInput) -> Result<ProviderId, StoreError> {
        self.call(move |c| upsert_provider(c, input)).await
    }

    /// All providers, oldest first.
    pub async fn list_providers(&self) -> Result<Vec<Provider>, StoreError> {
        self.call(|c| list_providers(c)).await
    }

    /// Returns the provider, or `None`.
    pub async fn get_provider(&self, id: ProviderId) -> Result<Option<Provider>, StoreError> {
        self.call(move |c| get_provider(c, id)).await
    }

    /// Deletes a provider; returns whether it existed. The caller deletes its keychain entry.
    pub async fn delete_provider(&self, id: ProviderId) -> Result<bool, StoreError> {
        self.call(move |c| delete_by_id(c, "providers", id.0)).await
    }

    /// Approves a folder globally (`chat_id == None`) or for one chat. Idempotent: approving
    /// the same path for the same scope returns the existing id.
    pub async fn add_approved_folder(
        &self,
        path: String,
        chat_id: Option<ChatId>,
    ) -> Result<FolderId, StoreError> {
        self.call(move |c| add_approved_folder(c, path, chat_id))
            .await
    }

    /// Every approved folder, global and per chat.
    pub async fn list_approved_folders(&self) -> Result<Vec<ApprovedFolder>, StoreError> {
        self.call(|c| list_approved_folders(c)).await
    }

    /// The folders that apply to a chat: the global ones plus the chat's own.
    pub async fn approved_folders_for_chat(
        &self,
        chat_id: ChatId,
    ) -> Result<Vec<ApprovedFolder>, StoreError> {
        self.call(move |c| approved_folders_for_chat(c, chat_id))
            .await
    }

    /// Revokes an approved folder; returns whether it existed.
    pub async fn remove_approved_folder(&self, id: FolderId) -> Result<bool, StoreError> {
        self.call(move |c| delete_by_id(c, "approved_folders", id.0))
            .await
    }

    /// Inserts an MCP server or updates the one with the same name. Changing the config
    /// clears `acknowledged_at`, so the user re-confirms what will run.
    pub async fn upsert_mcp_server(
        &self,
        input: McpServerInput,
    ) -> Result<McpServerId, StoreError> {
        self.call(move |c| upsert_mcp_server(c, input)).await
    }

    /// All MCP servers, by name.
    pub async fn list_mcp_servers(&self) -> Result<Vec<McpServer>, StoreError> {
        self.call(|c| list_mcp_servers(c)).await
    }

    /// Returns the MCP server, or `None`.
    pub async fn get_mcp_server(&self, id: McpServerId) -> Result<Option<McpServer>, StoreError> {
        self.call(move |c| get_mcp_server(c, id)).await
    }

    /// Records that the user acknowledged the "runs as you" warning at `at` (Unix ms) for
    /// `shown_config`, the config the warning displayed.
    ///
    /// Fails with [`StoreError::Conflict`] (and records nothing) if the stored config is no
    /// longer `shown_config`, so an acknowledgement never covers a config the user did not
    /// see. The check and the update run together on the database thread.
    pub async fn acknowledge_mcp_server(
        &self,
        id: McpServerId,
        shown_config: Value,
        at: i64,
    ) -> Result<(), StoreError> {
        self.call(move |c| acknowledge_mcp_server(c, id, &shown_config, at))
            .await
    }

    /// Records the protocol revision family the server negotiated.
    pub async fn set_mcp_protocol_era(
        &self,
        id: McpServerId,
        era: Option<String>,
    ) -> Result<(), StoreError> {
        self.call(move |c| set_mcp_server_field(c, id, "protocol_era", sql_opt_text(era)))
            .await
    }

    /// Deletes an MCP server and its tool preferences; returns whether it existed.
    pub async fn delete_mcp_server(&self, id: McpServerId) -> Result<bool, StoreError> {
        self.call(move |c| delete_by_id(c, "mcp_servers", id.0))
            .await
    }

    /// Inserts or replaces one tool's preferences.
    pub async fn upsert_mcp_tool_pref(&self, pref: McpToolPref) -> Result<(), StoreError> {
        self.call(move |c| upsert_mcp_tool_pref(c, pref)).await
    }

    /// The server's tool preferences, by tool name.
    pub async fn mcp_tool_prefs(
        &self,
        server_id: McpServerId,
    ) -> Result<Vec<McpToolPref>, StoreError> {
        self.call(move |c| mcp_tool_prefs(c, server_id)).await
    }

    /// Removes one tool's preferences; returns whether they existed.
    pub async fn delete_mcp_tool_pref(
        &self,
        server_id: McpServerId,
        tool_name: String,
    ) -> Result<bool, StoreError> {
        self.call(move |c| {
            Ok(c.execute(
                "DELETE FROM mcp_tool_prefs WHERE server_id = ?1 AND tool_name = ?2",
                params![server_id, tool_name],
            )? > 0)
        })
        .await
    }

    /// Inserts a skill or updates the one with the same name (keeping `imported_at`).
    pub async fn upsert_skill(&self, input: SkillInput) -> Result<SkillId, StoreError> {
        self.call(move |c| upsert_skill(c, input)).await
    }

    /// All skills, by name.
    pub async fn list_skills(&self) -> Result<Vec<Skill>, StoreError> {
        self.call(|c| list_skills(c)).await
    }

    /// Enables or disables a skill.
    pub async fn set_skill_enabled(&self, id: SkillId, enabled: bool) -> Result<(), StoreError> {
        self.call(move |c| {
            let n = c.execute(
                "UPDATE skills SET enabled = ?1 WHERE id = ?2",
                params![enabled, id],
            )?;
            if n == 0 {
                Err(StoreError::not_found("skill", id.0))
            } else {
                Ok(())
            }
        })
        .await
    }

    /// Deletes a skill row (not its folder); returns whether it existed.
    pub async fn delete_skill(&self, id: SkillId) -> Result<bool, StoreError> {
        self.call(move |c| delete_by_id(c, "skills", id.0)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chats::NewChat;
    use serde_json::json;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Prefs {
        theme: String,
        step_cap: u32,
    }

    #[tokio::test]
    async fn settings_round_trip() {
        let store = Store::open_in_memory().expect("open");
        assert_eq!(store.get_setting::<Prefs>("ui").await.expect("get"), None);
        let prefs = Prefs {
            theme: "dark".into(),
            step_cap: 25,
        };
        store.set_setting("ui", &prefs).await.expect("set");
        assert_eq!(
            store.get_setting::<Prefs>("ui").await.expect("get"),
            Some(prefs)
        );
        store
            .set_setting(
                "ui",
                &Prefs {
                    theme: "light".into(),
                    step_cap: 10,
                },
            )
            .await
            .expect("overwrite");
        let got: Prefs = store.get_setting("ui").await.expect("get").expect("set");
        assert_eq!(got.theme, "light");
        store.set_setting("n", "plain string").await.expect("str");
        assert_eq!(
            store
                .get_setting::<String>("n")
                .await
                .expect("get")
                .as_deref(),
            Some("plain string")
        );
        // Wrong type is an error, not a panic.
        assert!(matches!(
            store.get_setting::<u32>("n").await,
            Err(StoreError::Json(_))
        ));
        assert!(store.delete_setting("n").await.expect("delete"));
        assert!(!store.delete_setting("n").await.expect("delete again"));
    }

    /// 0.0.6: the prefix query matches the start of the key exactly.
    #[tokio::test]
    async fn settings_by_prefix() {
        let store = Store::open_in_memory().expect("open");
        for (k, v) in [
            ("modelProfile:bb", json!({"n": 2})),
            ("modelProfile:aa", json!({"n": 1})),
            ("modelProfiles", json!("not a profile")),
            ("xmodelProfile:cc", json!(3)),
            ("modelprofile:dd", json!(4)),
            ("model_Profile:ee", json!(5)),
            ("a%b:1", json!(6)),
            ("axb:2", json!(7)),
        ] {
            store.set_setting(k, &v).await.expect("set");
        }
        let got = store.settings_with_prefix("modelProfile:").await.expect("list");
        assert_eq!(
            got,
            vec![
                ("modelProfile:aa".to_string(), json!({"n": 1})),
                ("modelProfile:bb".to_string(), json!({"n": 2})),
            ]
        );
        // `%` and `_` are not wildcards; the comparison is case-sensitive.
        let got = store.settings_with_prefix("a%b").await.expect("list");
        assert_eq!(got, vec![("a%b:1".to_string(), json!(6))]);
        let got = store.settings_with_prefix("model_").await.expect("list");
        assert_eq!(got, vec![("model_Profile:ee".to_string(), json!(5))]);
        assert!(store.settings_with_prefix("nothing").await.expect("list").is_empty());
        assert!(matches!(
            store.settings_with_prefix("").await,
            Err(StoreError::InvalidInput(_))
        ));
        // The table refuses text that is not JSON (its CHECK), so every row parses.
        assert!(
            store
                .call(|c| set_setting_text(c, "modelProfile:zz", "{not json"))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn providers_crud() {
        let store = Store::open_in_memory().expect("open");
        let mut input = ProviderInput {
            id: None,
            name: "Ollama".into(),
            kind: "ollama".into(),
            base_url: "http://127.0.0.1:11434/v1".into(),
            secret_ref: None,
            enabled: true,
        };
        let id = store.upsert_provider(input.clone()).await.expect("insert");
        input.id = Some(id);
        input.secret_ref = Some(SecretRef::new("provider.1.api_key"));
        input.enabled = false;
        assert_eq!(store.upsert_provider(input).await.expect("update"), id);
        let p = store.get_provider(id).await.expect("get").expect("exists");
        assert_eq!(
            p.secret_ref.as_ref().map(SecretRef::as_str),
            Some("provider.1.api_key")
        );
        assert!(!p.enabled);
        assert_eq!(store.list_providers().await.expect("list").len(), 1);
        assert!(store.delete_provider(id).await.expect("delete"));
        assert!(store.list_providers().await.expect("list").is_empty());
        let missing = ProviderInput {
            id: Some(ProviderId(9)),
            name: "x".into(),
            kind: "x".into(),
            base_url: "x".into(),
            secret_ref: None,
            enabled: true,
        };
        assert!(matches!(
            store.upsert_provider(missing).await,
            Err(StoreError::NotFound { .. })
        ));
    }

    #[tokio::test]
    async fn approved_folders_scopes_and_cascade() {
        let store = Store::open_in_memory().expect("open");
        let chat = store
            .create_chat(NewChat {
                title: "c".into(),
                ..NewChat::default()
            })
            .await
            .expect("chat");
        let other = store
            .create_chat(NewChat {
                title: "o".into(),
                ..NewChat::default()
            })
            .await
            .expect("chat");
        let global = store
            .add_approved_folder("/home/me/projects".into(), None)
            .await
            .expect("global");
        assert_eq!(
            store
                .add_approved_folder("/home/me/projects".into(), None)
                .await
                .expect("again"),
            global
        );
        let scoped = store
            .add_approved_folder("/home/me/tmp".into(), Some(chat))
            .await
            .expect("scoped");
        store
            .add_approved_folder("/home/me/other".into(), Some(other))
            .await
            .expect("other");

        let for_chat = store
            .approved_folders_for_chat(chat)
            .await
            .expect("for chat");
        assert_eq!(
            for_chat.iter().map(|f| f.id).collect::<Vec<_>>(),
            vec![global, scoped]
        );
        assert_eq!(store.list_approved_folders().await.expect("all").len(), 3);

        store.delete_chat(chat).await.expect("delete chat");
        assert_eq!(store.list_approved_folders().await.expect("all").len(), 2);
        assert!(store.remove_approved_folder(global).await.expect("remove"));
        assert!(
            !store
                .remove_approved_folder(global)
                .await
                .expect("remove again")
        );
        assert!(matches!(
            store
                .add_approved_folder("/x".into(), Some(ChatId(404)))
                .await,
            Err(StoreError::NotFound { .. })
        ));
    }

    #[tokio::test]
    async fn mcp_servers_and_tool_prefs() {
        let store = Store::open_in_memory().expect("open");
        let input = McpServerInput {
            name: "fetch".into(),
            config: json!({ "command": "uvx", "args": ["mcp-server-fetch"] }),
            enabled: true,
        };
        let id = store
            .upsert_mcp_server(input.clone())
            .await
            .expect("insert");
        // An acknowledgement for a config the user did not see is refused.
        assert!(matches!(
            store
                .acknowledge_mcp_server(id, json!({ "command": "something else" }), 99)
                .await,
            Err(StoreError::Conflict(_))
        ));
        assert_eq!(
            store
                .get_mcp_server(id)
                .await
                .expect("get")
                .expect("exists")
                .acknowledged_at,
            None
        );
        store
            .acknowledge_mcp_server(id, input.config.clone(), 123)
            .await
            .expect("ack");
        store
            .set_mcp_protocol_era(id, Some("2026-07-28".into()))
            .await
            .expect("era");

        // Same config: acknowledgement kept.
        assert_eq!(
            store.upsert_mcp_server(input.clone()).await.expect("same"),
            id
        );
        let s = store
            .get_mcp_server(id)
            .await
            .expect("get")
            .expect("exists");
        assert_eq!(s.acknowledged_at, Some(123));
        assert_eq!(s.protocol_era.as_deref(), Some("2026-07-28"));

        // Changed config: acknowledgement cleared.
        let changed = McpServerInput {
            config: json!({ "command": "evil" }),
            ..input
        };
        assert_eq!(store.upsert_mcp_server(changed).await.expect("changed"), id);
        let s = store
            .get_mcp_server(id)
            .await
            .expect("get")
            .expect("exists");
        assert_eq!(s.acknowledged_at, None);
        assert_eq!(s.config, json!({ "command": "evil" }));

        let pref = McpToolPref {
            server_id: id,
            tool_name: "fetch".into(),
            enabled: true,
            allow_any_args: false,
            always_ask: true,
            fingerprint: Some("abc".into()),
        };
        store
            .upsert_mcp_tool_pref(pref.clone())
            .await
            .expect("pref");
        store
            .upsert_mcp_tool_pref(McpToolPref {
                allow_any_args: true,
                ..pref.clone()
            })
            .await
            .expect("pref update");
        let prefs = store.mcp_tool_prefs(id).await.expect("prefs");
        assert_eq!(prefs.len(), 1);
        assert!(prefs[0].allow_any_args && prefs[0].always_ask);
        assert!(matches!(
            store
                .upsert_mcp_tool_pref(McpToolPref {
                    server_id: McpServerId(77),
                    ..pref
                })
                .await,
            Err(StoreError::NotFound { .. })
        ));

        assert_eq!(store.list_mcp_servers().await.expect("list").len(), 1);
        assert!(store.delete_mcp_server(id).await.expect("delete"));
        assert!(
            store.mcp_tool_prefs(id).await.expect("prefs").is_empty(),
            "prefs cascade"
        );
        assert!(matches!(
            store.acknowledge_mcp_server(id, json!({}), 1).await,
            Err(StoreError::NotFound { .. })
        ));
    }

    #[tokio::test]
    async fn skills_crud() {
        let store = Store::open_in_memory().expect("open");
        let id = store
            .upsert_skill(SkillInput {
                name: "pdf".into(),
                dir: "/skills/pdf".into(),
                enabled: false,
            })
            .await
            .expect("insert");
        let first = store.list_skills().await.expect("list")[0].imported_at;
        let again = store
            .upsert_skill(SkillInput {
                name: "pdf".into(),
                dir: "/skills/pdf2".into(),
                enabled: false,
            })
            .await
            .expect("update");
        assert_eq!(again, id);
        store.set_skill_enabled(id, true).await.expect("enable");
        let skills = store.list_skills().await.expect("list");
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].dir, "/skills/pdf2");
        assert!(skills[0].enabled);
        assert_eq!(skills[0].imported_at, first);
        assert!(store.delete_skill(id).await.expect("delete"));
        assert!(matches!(
            store.set_skill_enabled(id, true).await,
            Err(StoreError::NotFound { .. })
        ));
    }
}
