//! Installed models and download state for resume (ARCHITECTURE.md sections 11 and 12).

use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Store;
use crate::error::StoreError;
use crate::ids::{
    DownloadId, JsonColumn, ModelId, json_text, now_ms, sql_enum, sql_opt_text, sql_u64,
    update_columns,
};

sql_enum!(
    /// How a model got onto the machine.
    ModelSource, "model source" {
        /// Downloaded by the app.
        Download => "download",
        /// An existing GGUF registered in place.
        Import => "import",
    }
);

sql_enum!(
    /// State of one file download.
    DownloadStatus, "download status" {
        /// Waiting to start.
        Queued => "queued",
        /// Transferring.
        Downloading => "downloading",
        /// Paused by the user or interrupted; can resume.
        Paused => "paused",
        /// Checking the SHA-256.
        Verifying => "verifying",
        /// Done and verified.
        Completed => "completed",
        /// Failed; see [`Download::error`].
        Failed => "failed",
        /// Cancelled by the user.
        Cancelled => "cancelled",
    }
);

/// Input for [`Store::upsert_model`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelInput {
    /// Absolute path of the GGUF file; the upsert key.
    pub path: String,
    /// Hugging Face repo it came from, if known.
    pub repo: Option<String>,
    /// File name.
    pub file_name: String,
    /// File size.
    pub size_bytes: u64,
    /// Hex SHA-256, if known.
    pub sha256: Option<String>,
    /// Summary from the GGUF header reader (shape owned by the models crate).
    pub gguf_summary: Value,
    /// Downloaded or imported.
    pub source: ModelSource,
}

/// A row of `models`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelRecord {
    /// Row id.
    pub id: ModelId,
    /// Absolute path.
    pub path: String,
    /// Hugging Face repo.
    pub repo: Option<String>,
    /// File name.
    pub file_name: String,
    /// File size.
    pub size_bytes: u64,
    /// Hex SHA-256.
    pub sha256: Option<String>,
    /// GGUF header summary.
    pub gguf_summary: Value,
    /// Unix ms when first registered.
    pub added_at: i64,
    /// Downloaded or imported.
    pub source: ModelSource,
}

const MODEL_COLUMNS: &str =
    "id, path, repo, file_name, size_bytes, sha256, gguf_summary, added_at, source";

fn model_from_row(r: &Row<'_>) -> rusqlite::Result<ModelRecord> {
    Ok(ModelRecord {
        id: r.get(0)?,
        path: r.get(1)?,
        repo: r.get(2)?,
        file_name: r.get(3)?,
        size_bytes: r.get(4)?,
        sha256: r.get(5)?,
        gguf_summary: r.get::<_, JsonColumn>(6)?.0,
        added_at: r.get(7)?,
        source: r.get(8)?,
    })
}

fn upsert_model(conn: &Connection, m: ModelInput) -> Result<ModelId, StoreError> {
    if m.path.is_empty() {
        return Err(StoreError::InvalidInput("model path is empty".into()));
    }
    let size = sql_u64("size_bytes", m.size_bytes)?;
    conn.execute(
        "INSERT INTO models (path, repo, file_name, size_bytes, sha256, gguf_summary, added_at, source) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
         ON CONFLICT(path) DO UPDATE SET repo = excluded.repo, file_name = excluded.file_name, \
             size_bytes = excluded.size_bytes, sha256 = excluded.sha256, \
             gguf_summary = excluded.gguf_summary, source = excluded.source",
        params![
            m.path,
            m.repo,
            m.file_name,
            size,
            m.sha256,
            json_text(&m.gguf_summary)?,
            now_ms(),
            m.source
        ],
    )?;
    Ok(
        conn.query_row("SELECT id FROM models WHERE path = ?1", [&m.path], |r| {
            r.get(0)
        })?,
    )
}

fn get_model(conn: &Connection, id: ModelId) -> Result<Option<ModelRecord>, StoreError> {
    let sql = format!("SELECT {MODEL_COLUMNS} FROM models WHERE id = ?1");
    Ok(conn.query_row(&sql, [id], model_from_row).optional()?)
}

fn model_by_path(conn: &Connection, path: &str) -> Result<Option<ModelRecord>, StoreError> {
    let sql = format!("SELECT {MODEL_COLUMNS} FROM models WHERE path = ?1");
    Ok(conn.query_row(&sql, [path], model_from_row).optional()?)
}

fn list_models(conn: &Connection) -> Result<Vec<ModelRecord>, StoreError> {
    let sql = format!("SELECT {MODEL_COLUMNS} FROM models ORDER BY file_name, id");
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map([], model_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Input for [`Store::insert_download`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewDownload {
    /// Groups the files of one model (e.g. a split GGUF plus its projector).
    pub group_id: String,
    /// Hugging Face repo.
    pub repo: String,
    /// File path inside the repo.
    pub file: String,
    /// The resolve URL (the signed CDN URL is re-resolved on every resume).
    pub url: String,
    /// Final destination on disk.
    pub dest_path: String,
    /// Expected size.
    pub size_bytes: u64,
    /// Expected hex SHA-256.
    pub sha256: String,
}

/// Input for [`Store::update_download`]. `None` leaves a field unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadPatch {
    /// Bytes on disk so far.
    pub bytes_done: Option<u64>,
    /// New state.
    pub status: Option<DownloadStatus>,
    /// ETag seen on the last response, for resume validation.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::ids::double_option"
    )]
    pub etag: Option<Option<String>>,
    /// Error text for a failed download; `Some(None)` clears it.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::ids::double_option"
    )]
    pub error: Option<Option<String>>,
}

/// A row of `downloads`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Download {
    /// Row id.
    pub id: DownloadId,
    /// Group of files for one model.
    pub group_id: String,
    /// Hugging Face repo.
    pub repo: String,
    /// File path inside the repo.
    pub file: String,
    /// Resolve URL.
    pub url: String,
    /// Final destination.
    pub dest_path: String,
    /// Expected size.
    pub size_bytes: u64,
    /// Expected hex SHA-256.
    pub sha256: String,
    /// Bytes on disk so far.
    pub bytes_done: u64,
    /// Current state.
    pub status: DownloadStatus,
    /// Last ETag.
    pub etag: Option<String>,
    /// Error text.
    pub error: Option<String>,
    /// Unix ms.
    pub created_at: i64,
    /// Unix ms of the last change.
    pub updated_at: i64,
}

const DOWNLOAD_COLUMNS: &str = "id, group_id, repo, file, url, dest_path, size_bytes, sha256, \
     bytes_done, status, etag, error, created_at, updated_at";

fn download_from_row(r: &Row<'_>) -> rusqlite::Result<Download> {
    Ok(Download {
        id: r.get(0)?,
        group_id: r.get(1)?,
        repo: r.get(2)?,
        file: r.get(3)?,
        url: r.get(4)?,
        dest_path: r.get(5)?,
        size_bytes: r.get(6)?,
        sha256: r.get(7)?,
        bytes_done: r.get(8)?,
        status: r.get(9)?,
        etag: r.get(10)?,
        error: r.get(11)?,
        created_at: r.get(12)?,
        updated_at: r.get(13)?,
    })
}

fn insert_download(conn: &Connection, d: NewDownload) -> Result<DownloadId, StoreError> {
    let now = now_ms();
    let size = sql_u64("size_bytes", d.size_bytes)?;
    conn.execute(
        "INSERT INTO downloads (group_id, repo, file, url, dest_path, size_bytes, sha256, \
         bytes_done, status, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8, ?9, ?9)",
        params![
            d.group_id,
            d.repo,
            d.file,
            d.url,
            d.dest_path,
            size,
            d.sha256,
            DownloadStatus::Queued,
            now
        ],
    )?;
    Ok(DownloadId(conn.last_insert_rowid()))
}

fn update_download(
    conn: &Connection,
    id: DownloadId,
    patch: DownloadPatch,
) -> Result<(), StoreError> {
    let mut sets = vec![("updated_at", SqlValue::Integer(now_ms()))];
    if let Some(v) = patch.bytes_done {
        sets.push(("bytes_done", sql_u64("bytes_done", v)?));
    }
    if let Some(v) = patch.status {
        sets.push(("status", SqlValue::Text(v.as_str().to_owned())));
    }
    if let Some(v) = patch.etag {
        sets.push(("etag", sql_opt_text(v)));
    }
    if let Some(v) = patch.error {
        sets.push(("error", sql_opt_text(v)));
    }
    if update_columns(conn, "downloads", "id", id.0, sets)? == 0 {
        return Err(StoreError::not_found("download", id.0));
    }
    Ok(())
}

fn get_download(conn: &Connection, id: DownloadId) -> Result<Option<Download>, StoreError> {
    let sql = format!("SELECT {DOWNLOAD_COLUMNS} FROM downloads WHERE id = ?1");
    Ok(conn.query_row(&sql, [id], download_from_row).optional()?)
}

fn list_downloads(conn: &Connection, group: Option<&str>) -> Result<Vec<Download>, StoreError> {
    let sql = format!(
        "SELECT {DOWNLOAD_COLUMNS} FROM downloads WHERE ?1 IS NULL OR group_id = ?1 ORDER BY id"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map([group], download_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

impl Store {
    /// Registers a model, or updates the one at the same path (keeping `added_at`).
    pub async fn upsert_model(&self, model: ModelInput) -> Result<ModelId, StoreError> {
        self.call(move |c| upsert_model(c, model)).await
    }

    /// Returns the model, or `None`.
    pub async fn get_model(&self, id: ModelId) -> Result<Option<ModelRecord>, StoreError> {
        self.call(move |c| get_model(c, id)).await
    }

    /// Returns the model registered at `path`, or `None`.
    pub async fn model_by_path(&self, path: &str) -> Result<Option<ModelRecord>, StoreError> {
        let path = path.to_owned();
        self.call(move |c| model_by_path(c, &path)).await
    }

    /// All registered models, by file name.
    pub async fn list_models(&self) -> Result<Vec<ModelRecord>, StoreError> {
        self.call(|c| list_models(c)).await
    }

    /// Forgets a model (the caller deletes the file); returns whether it existed.
    pub async fn delete_model(&self, id: ModelId) -> Result<bool, StoreError> {
        self.call(move |c| Ok(c.execute("DELETE FROM models WHERE id = ?1", [id])? > 0))
            .await
    }

    /// Records a new download in the `Queued` state.
    pub async fn insert_download(&self, download: NewDownload) -> Result<DownloadId, StoreError> {
        self.call(move |c| insert_download(c, download)).await
    }

    /// Updates progress, state, ETag or error, and bumps `updated_at`.
    pub async fn update_download(
        &self,
        id: DownloadId,
        patch: DownloadPatch,
    ) -> Result<(), StoreError> {
        self.call(move |c| update_download(c, id, patch)).await
    }

    /// Returns the download, or `None`.
    pub async fn get_download(&self, id: DownloadId) -> Result<Option<Download>, StoreError> {
        self.call(move |c| get_download(c, id)).await
    }

    /// Downloads, oldest first; all of them, or one group's.
    pub async fn list_downloads(
        &self,
        group_id: Option<String>,
    ) -> Result<Vec<Download>, StoreError> {
        self.call(move |c| list_downloads(c, group_id.as_deref()))
            .await
    }

    /// Deletes one download row; returns whether it existed.
    pub async fn delete_download(&self, id: DownloadId) -> Result<bool, StoreError> {
        self.call(move |c| Ok(c.execute("DELETE FROM downloads WHERE id = ?1", [id])? > 0))
            .await
    }

    /// Deletes every row of a group; returns how many were removed.
    pub async fn delete_download_group(&self, group_id: String) -> Result<usize, StoreError> {
        self.call(move |c| Ok(c.execute("DELETE FROM downloads WHERE group_id = ?1", [group_id])?))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn model(path: &str) -> ModelInput {
        ModelInput {
            path: path.into(),
            repo: Some("Qwen/Qwen3-0.6B-GGUF".into()),
            file_name: "Qwen3-0.6B-Q8_0.gguf".into(),
            size_bytes: 639_447_744,
            sha256: None,
            gguf_summary: json!({ "architecture": "qwen3", "context_length": 40960 }),
            source: ModelSource::Download,
        }
    }

    #[tokio::test]
    async fn models_upsert_by_path() {
        let store = Store::open_in_memory().expect("open");
        let id = store
            .upsert_model(model("/m/a.gguf"))
            .await
            .expect("insert");
        let mut again = model("/m/a.gguf");
        again.sha256 = Some("ff".repeat(32));
        again.source = ModelSource::Import;
        assert_eq!(store.upsert_model(again).await.expect("update"), id);
        let m = store.get_model(id).await.expect("get").expect("exists");
        assert_eq!(m.sha256.as_deref(), Some("ff".repeat(32).as_str()));
        assert_eq!(m.source, ModelSource::Import);
        assert_eq!(m.gguf_summary["architecture"], json!("qwen3"));
        assert_eq!(
            store
                .model_by_path("/m/a.gguf")
                .await
                .expect("by path")
                .map(|m| m.id),
            Some(id)
        );
        store
            .upsert_model(model("/m/b.gguf"))
            .await
            .expect("second");
        assert_eq!(store.list_models().await.expect("list").len(), 2);
        assert!(store.delete_model(id).await.expect("delete"));
        assert!(store.get_model(id).await.expect("get").is_none());
    }

    #[tokio::test]
    async fn downloads_progress_and_groups() {
        let store = Store::open_in_memory().expect("open");
        let new = |file: &str| NewDownload {
            group_id: "g1".into(),
            repo: "r".into(),
            file: file.into(),
            url: format!("https://huggingface.co/r/resolve/main/{file}"),
            dest_path: format!("/models/{file}"),
            size_bytes: 1000,
            sha256: "aa".repeat(32),
        };
        let a = store.insert_download(new("a.gguf")).await.expect("a");
        store.insert_download(new("b.gguf")).await.expect("b");
        let mut other = new("c.gguf");
        other.group_id = "g2".into();
        store.insert_download(other).await.expect("c");

        store
            .update_download(
                a,
                DownloadPatch {
                    bytes_done: Some(500),
                    status: Some(DownloadStatus::Downloading),
                    etag: Some(Some("\"etag-1\"".into())),
                    error: None,
                },
            )
            .await
            .expect("progress");
        let d = store.get_download(a).await.expect("get").expect("exists");
        assert_eq!((d.bytes_done, d.status), (500, DownloadStatus::Downloading));
        assert_eq!(d.etag.as_deref(), Some("\"etag-1\""));
        store
            .update_download(
                a,
                DownloadPatch {
                    status: Some(DownloadStatus::Failed),
                    error: Some(Some("checksum mismatch".into())),
                    ..DownloadPatch::default()
                },
            )
            .await
            .expect("fail");
        let d = store.get_download(a).await.expect("get").expect("exists");
        assert_eq!(d.error.as_deref(), Some("checksum mismatch"));
        assert_eq!(d.bytes_done, 500, "unchanged");

        assert_eq!(store.list_downloads(None).await.expect("all").len(), 3);
        assert_eq!(
            store
                .list_downloads(Some("g1".into()))
                .await
                .expect("g1")
                .len(),
            2
        );
        assert_eq!(
            store
                .delete_download_group("g1".into())
                .await
                .expect("group"),
            2
        );
        assert_eq!(store.list_downloads(None).await.expect("all").len(), 1);
        assert!(matches!(
            store.update_download(a, DownloadPatch::default()).await,
            Err(StoreError::NotFound { .. })
        ));
    }
}
