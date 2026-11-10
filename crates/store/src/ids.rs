//! Row id newtypes, SQL-backed enums and small shared helpers.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use serde::{Deserialize, Serialize};

/// Declares an `i64` row-id newtype that converts to and from SQLite and JSON.
macro_rules! id_type {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub i64);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl From<$name> for i64 {
            fn from(value: $name) -> i64 {
                value.0
            }
        }

        impl ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
                Ok(ToSqlOutput::from(self.0))
            }
        }

        impl FromSql for $name {
            fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
                i64::column_result(value).map(Self)
            }
        }
    };
}

id_type!(
    /// Id of a row in `chats`.
    ChatId
);
id_type!(
    /// Id of a row in `messages`.
    MessageId
);
id_type!(
    /// Id of a row in `tool_calls`.
    ToolCallId
);
id_type!(
    /// Id of a row in `summaries`.
    SummaryId
);
id_type!(
    /// Id of a row in `attachments`.
    AttachmentId
);
id_type!(
    /// Id of a row in `audit_log`.
    AuditId
);
id_type!(
    /// Id of a row in `models`.
    ModelId
);
id_type!(
    /// Id of a row in `downloads`.
    DownloadId
);
id_type!(
    /// Id of a row in `providers`.
    ProviderId
);
id_type!(
    /// Id of a row in `mcp_servers`.
    McpServerId
);
id_type!(
    /// Id of a row in `skills`.
    SkillId
);
id_type!(
    /// Id of a row in `approved_folders`.
    FolderId
);

/// Declares a fieldless enum stored in SQLite as fixed lowercase text.
macro_rules! sql_enum {
    (
        $(#[$meta:meta])*
        $name:ident, $kind:literal {
            $($(#[$vmeta:meta])* $variant:ident => $text:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, ::serde::Serialize, ::serde::Deserialize)]
        pub enum $name {
            $($(#[$vmeta])* #[serde(rename = $text)] $variant),+
        }

        impl $name {
            /// Every variant, in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// The text stored in SQLite and used in JSON.
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),+
                }
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl std::str::FromStr for $name {
            type Err = $crate::ParseEnumError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($text => Ok(Self::$variant),)+
                    other => Err($crate::ParseEnumError { kind: $kind, value: other.to_owned() }),
                }
            }
        }

        impl ::rusqlite::types::ToSql for $name {
            fn to_sql(&self) -> ::rusqlite::Result<::rusqlite::types::ToSqlOutput<'_>> {
                Ok(::rusqlite::types::ToSqlOutput::from(self.as_str()))
            }
        }

        impl ::rusqlite::types::FromSql for $name {
            fn column_result(
                value: ::rusqlite::types::ValueRef<'_>,
            ) -> ::rusqlite::types::FromSqlResult<Self> {
                value
                    .as_str()?
                    .parse()
                    .map_err(|e: $crate::ParseEnumError| {
                        ::rusqlite::types::FromSqlError::Other(Box::new(e))
                    })
            }
        }
    };
}

pub(crate) use sql_enum;

/// Current time as Unix milliseconds. A clock before 1970 reads as 0.
pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Escapes `%`, `_` and `\` for a `LIKE ... ESCAPE '\'` pattern and wraps it in `%...%`.
pub(crate) fn like_contains(needle: &str) -> String {
    let mut out = String::with_capacity(needle.len() + 2);
    out.push('%');
    for c in needle.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

/// Serialises a JSON value to the text stored in a JSON column.
pub(crate) fn json_text(value: &serde_json::Value) -> Result<String, serde_json::Error> {
    serde_json::to_string(value)
}

/// A JSON column read from SQLite: the text is parsed on the way out.
pub(crate) struct JsonColumn(pub serde_json::Value);

impl FromSql for JsonColumn {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        serde_json::from_str(value.as_str()?)
            .map(JsonColumn)
            .map_err(|e| FromSqlError::Other(Box::new(e)))
    }
}

/// Converts a `u64` for an INTEGER column, refusing values SQLite cannot hold.
pub(crate) fn sql_u64(
    field: &str,
    value: u64,
) -> Result<rusqlite::types::Value, crate::StoreError> {
    i64::try_from(value)
        .map(rusqlite::types::Value::Integer)
        .map_err(|_| crate::StoreError::InvalidInput(format!("{field} is too large: {value}")))
}

/// Converts an optional `u64` for a nullable INTEGER column, refusing values SQLite cannot
/// hold.
pub(crate) fn sql_opt_u64(
    field: &str,
    value: Option<u64>,
) -> Result<rusqlite::types::Value, crate::StoreError> {
    value.map_or(Ok(rusqlite::types::Value::Null), |v| sql_u64(field, v))
}

/// Converts an optional string for a nullable TEXT column.
pub(crate) fn sql_opt_text(value: Option<String>) -> rusqlite::types::Value {
    value.map_or(rusqlite::types::Value::Null, rusqlite::types::Value::Text)
}

/// Runs `UPDATE <table> SET <col> = ?, ... WHERE <key> = ?` for the given columns.
///
/// `table`, `key` and the column names are compile-time constants from this crate, never
/// caller input. Returns the number of rows changed (0 when the id does not exist).
pub(crate) fn update_columns(
    conn: &rusqlite::Connection,
    table: &'static str,
    key: &'static str,
    id: i64,
    sets: Vec<(&'static str, rusqlite::types::Value)>,
) -> Result<usize, crate::StoreError> {
    let mut sql = format!("UPDATE {table} SET ");
    let mut values = Vec::with_capacity(sets.len() + 1);
    for (i, (column, value)) in sets.into_iter().enumerate() {
        if i > 0 {
            sql.push_str(", ");
        }
        sql.push_str(column);
        sql.push_str(" = ?");
        values.push(value);
    }
    sql.push_str(" WHERE ");
    sql.push_str(key);
    sql.push_str(" = ?");
    values.push(rusqlite::types::Value::Integer(id));
    Ok(conn.execute(&sql, rusqlite::params_from_iter(values))?)
}

/// Serde for patch fields of type `Option<Option<T>>`: a missing field is `None` (leave
/// unchanged), JSON `null` is `Some(None)` (clear) and a value is `Some(Some(v))`. Plain
/// serde collapses `null` to `None`, so a client could never clear the field.
///
/// Use with `#[serde(default, skip_serializing_if = "Option::is_none", with = "double_option")]`.
pub(crate) mod double_option {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    /// Writes `Some(None)` as `null` and `Some(Some(v))` as `v` (`None` is skipped).
    pub(crate) fn serialize<T: Serialize, S: Serializer>(
        value: &Option<Option<T>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(inner) => inner.serialize(serializer),
            None => serializer.serialize_none(),
        }
    }

    /// Reads a present field (including `null`) as `Some(..)`.
    pub(crate) fn deserialize<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Option<T>>, D::Error> {
        Option::<T>::deserialize(deserializer).map(Some)
    }
}

/// Maps a `limit` of 0 to SQLite's "no limit" (-1).
pub(crate) fn sql_limit(limit: u32) -> i64 {
    if limit == 0 { -1 } else { i64::from(limit) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_pattern_escapes_wildcards() {
        assert_eq!(like_contains("a%b_c\\d"), "%a\\%b\\_c\\\\d%");
        assert_eq!(like_contains(""), "%%");
    }

    #[test]
    fn patch_json_distinguishes_missing_null_and_value() {
        use crate::{ChatPatch, DownloadPatch};

        let missing: ChatPatch = serde_json::from_str("{}").expect("empty");
        assert_eq!(missing.working_folder, None, "missing leaves it unchanged");
        let clear: ChatPatch = serde_json::from_str(r#"{"working_folder": null}"#).expect("null");
        assert_eq!(clear.working_folder, Some(None), "null clears it");
        let set: ChatPatch =
            serde_json::from_str(r#"{"working_folder": "/w", "model_ref": null}"#).expect("set");
        assert_eq!(set.working_folder, Some(Some("/w".to_owned())));
        assert_eq!(set.model_ref, Some(None));
        assert_eq!(set.system_prompt, None);

        // Serialising and reading back keeps all three states.
        let json = serde_json::to_value(&set).expect("serialise");
        assert_eq!(json["working_folder"], serde_json::json!("/w"));
        assert!(json["model_ref"].is_null() && json.get("model_ref").is_some());
        assert!(
            json.get("system_prompt").is_none(),
            "unchanged fields are omitted"
        );
        let back: ChatPatch = serde_json::from_value(json).expect("round trip");
        assert_eq!(back, set);

        let d: DownloadPatch =
            serde_json::from_str(r#"{"error": null, "etag": "\"e\""}"#).expect("download");
        assert_eq!(d.error, Some(None));
        assert_eq!(d.etag, Some(Some("\"e\"".to_owned())));
    }

    #[test]
    fn now_is_after_2025() {
        assert!(now_ms() > 1_735_689_600_000);
    }
}
