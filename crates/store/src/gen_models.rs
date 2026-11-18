//! Picture and video models installed for the Studio engine, and the model licences the
//! user accepted (app 0.1.0).
//!
//! - An **installed model** is one row of `gen_models`: an entry of the signed catalog's
//!   recommended list, or a Hugging Face search result, whose every part (the diffusion
//!   model, its text encoder, its decoder) is downloaded and verified. Each part is a file
//!   in the models folder. These rows never go into `models`, which feeds the chat picker.
//! - **Shared files.** One file can serve several models: two picture models with the same
//!   text encoder, or a chat model whose GGUF is also a picture model's encoder.
//!   [`Store::file_uses`] counts every use in both tables, so a delete removes a file only
//!   when nothing else uses it.
//! - A **licence acceptance** is kept once per licence text (its SHA-256), whichever model
//!   it was accepted for. A local download is refused until every licence of every part is
//!   accepted.

use std::collections::BTreeSet;

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Store;
use crate::error::StoreError;
use crate::ids::{JsonColumn, json_text, now_ms, sql_enum, sql_u64};

sql_enum!(
    /// Where an installed picture or video model came from.
    GenSource, "gen model source" {
        /// The signed catalog's recommended list.
        Catalog => "catalog",
        /// A Hugging Face search.
        Search => "search",
    }
);

/// Input for [`Store::insert_gen_model`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewGenModel {
    /// The catalog id, or `hf:<owner>/<repo>:<file>` for a searched model.
    pub id: String,
    /// The signed catalog's entry; `None` for a searched model.
    pub catalog_id: Option<String>,
    /// The name as shown.
    pub name: String,
    /// What it makes: `image`, `image-edit` or `video`.
    pub kind: String,
    /// Its family in the known-good family table.
    pub family: String,
    /// Its files: a JSON array of `{role, repo, revision, path, file, size, sha256,
    /// license}`, where `file` is the absolute path on disk.
    pub parts: Value,
    /// The parts' total size.
    pub size_bytes: u64,
    /// The recommended list or a Hugging Face search.
    pub source: GenSource,
}

/// A row of `gen_models`: an installed picture or video model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GenModel {
    /// The catalog id, or `hf:<owner>/<repo>:<file>` for a searched model.
    pub id: String,
    /// The signed catalog's entry; `None` for a searched model.
    pub catalog_id: Option<String>,
    /// The name as shown.
    pub name: String,
    /// What it makes: `image`, `image-edit` or `video`.
    pub kind: String,
    /// Its family in the known-good family table.
    pub family: String,
    /// Its files: a JSON array of `{role, repo, revision, path, file, size, sha256,
    /// license}`, where `file` is the absolute path on disk.
    pub parts: Value,
    /// The parts' total size.
    pub size_bytes: u64,
    /// The recommended list or a Hugging Face search.
    pub source: GenSource,
    /// Unix ms when it was installed (or last installed again).
    pub added_at: i64,
}

/// Input for [`Store::record_license_acceptance`]: one model licence the user accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LicenseAcceptance {
    /// Hex SHA-256 of the licence's canonical text: one acceptance per text.
    pub sha256: String,
    /// The licence's id, for example `apache-2.0`.
    pub license_id: String,
    /// The catalog entry it was first accepted for; `None` for a searched model or the
    /// engine.
    pub catalog_id: Option<String>,
    /// Unix ms, from the caller's clock.
    pub accepted_at: i64,
}

const GEN_MODEL_COLUMNS: &str =
    "id, catalog_id, name, kind, family, parts_json, size_bytes, source, added_at";

/// Every part's `file`, one row per part of every installed model (NULL for a part without
/// one). Reading `file` through the whole array's path keeps a part that is not an object
/// from failing the query.
const PART_FILES: &str = "SELECT json_extract(g.parts_json, p.fullkey || '.file') AS file \
     FROM gen_models g, json_each(g.parts_json) p";

fn gen_model_from_row(r: &Row<'_>) -> rusqlite::Result<GenModel> {
    Ok(GenModel {
        id: r.get(0)?,
        catalog_id: r.get(1)?,
        name: r.get(2)?,
        kind: r.get(3)?,
        family: r.get(4)?,
        parts: r.get::<_, JsonColumn>(5)?.0,
        size_bytes: r.get(6)?,
        source: r.get(7)?,
        added_at: r.get(8)?,
    })
}

/// Refuses parts that are not an array of objects each naming its file: a part without its
/// file would not be counted by [`Store::file_uses`], so a delete could remove a file this
/// model still uses.
fn check_parts(parts: &Value) -> Result<(), StoreError> {
    let Value::Array(items) = parts else {
        return Err(StoreError::InvalidInput(
            "an installed model's parts must be a JSON array".into(),
        ));
    };
    let names_its_file = |item: &Value| {
        item.get("file")
            .and_then(Value::as_str)
            .is_some_and(|f| !f.is_empty())
    };
    if !items.iter().all(names_its_file) {
        return Err(StoreError::InvalidInput(
            "each part of an installed model must name its file".into(),
        ));
    }
    Ok(())
}

fn insert_gen_model(conn: &Connection, m: NewGenModel) -> Result<(), StoreError> {
    if m.id.is_empty() {
        return Err(StoreError::InvalidInput(
            "an installed model needs an id".into(),
        ));
    }
    check_parts(&m.parts)?;
    // REPLACE deletes the old row and inserts a new one, so a model installed again is
    // the newest.
    conn.execute(
        "INSERT OR REPLACE INTO gen_models (id, catalog_id, name, kind, family, parts_json, \
         size_bytes, source, added_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            m.id,
            m.catalog_id,
            m.name,
            m.kind,
            m.family,
            json_text(&m.parts)?,
            sql_u64("size_bytes", m.size_bytes)?,
            m.source,
            now_ms()
        ],
    )?;
    Ok(())
}

fn get_gen_model(conn: &Connection, id: &str) -> Result<Option<GenModel>, StoreError> {
    let sql = format!("SELECT {GEN_MODEL_COLUMNS} FROM gen_models WHERE id = ?1");
    Ok(conn.query_row(&sql, [id], gen_model_from_row).optional()?)
}

fn list_gen_models(conn: &Connection) -> Result<Vec<GenModel>, StoreError> {
    // The rowid breaks a tie between models installed in the same millisecond.
    let sql =
        format!("SELECT {GEN_MODEL_COLUMNS} FROM gen_models ORDER BY added_at DESC, rowid DESC");
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map([], gen_model_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn delete_gen_model(conn: &mut Connection, id: &str) -> Result<Option<GenModel>, StoreError> {
    let tx = conn.transaction()?;
    let model = get_gen_model(&tx, id)?;
    if model.is_some() {
        tx.execute("DELETE FROM gen_models WHERE id = ?1", [id])?;
    }
    tx.commit()?;
    Ok(model)
}

fn file_uses(conn: &Connection, path: &str) -> Result<u32, StoreError> {
    let sql = format!(
        "SELECT (SELECT count(*) FROM models WHERE path = ?1) \
         + (SELECT count(*) FROM ({PART_FILES}) WHERE file = ?1)"
    );
    Ok(conn.query_row(&sql, [path], |r| r.get(0))?)
}

fn gen_model_files(conn: &Connection) -> Result<BTreeSet<String>, StoreError> {
    let sql = format!("SELECT DISTINCT file FROM ({PART_FILES}) WHERE typeof(file) = 'text'");
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map([], |r| r.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn accepted_license_hashes(conn: &Connection) -> Result<BTreeSet<String>, StoreError> {
    let mut stmt = conn.prepare_cached("SELECT sha256 FROM license_acceptances")?;
    let rows = stmt.query_map([], |r| r.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

impl Store {
    /// Records an installed picture or video model, replacing a row with the same id (a
    /// model installed again, which becomes the newest).
    ///
    /// An empty id, or parts that are not a JSON array of objects each with a non-empty
    /// `file`, is [`StoreError::InvalidInput`]; a kind other than `image`, `image-edit` or
    /// `video` is refused by the database.
    pub async fn insert_gen_model(&self, m: NewGenModel) -> Result<(), StoreError> {
        self.call(move |c| insert_gen_model(c, m)).await
    }

    /// Every installed picture and video model, newest first.
    pub async fn list_gen_models(&self) -> Result<Vec<GenModel>, StoreError> {
        self.call(|c| list_gen_models(c)).await
    }

    /// Installed model `id`; `None` when it is not installed.
    pub async fn get_gen_model(&self, id: &str) -> Result<Option<GenModel>, StoreError> {
        let id = id.to_owned();
        self.call(move |c| get_gen_model(c, &id)).await
    }

    /// Forgets installed model `id` and returns it as it was, so the caller can remove the
    /// part files nothing else uses; `None` when it was not installed. The files stay on
    /// disk.
    pub async fn delete_gen_model(&self, id: &str) -> Result<Option<GenModel>, StoreError> {
        let id = id.to_owned();
        self.call(move |c| delete_gen_model(c, &id)).await
    }

    /// How many installed models use the file at `path`: chat models (`models.path`) and
    /// every part of every picture or video model. The path is compared exactly, as stored.
    pub async fn file_uses(&self, path: &str) -> Result<u32, StoreError> {
        let path = path.to_owned();
        self.call(move |c| file_uses(c, &path)).await
    }

    /// Every part file of every installed picture or video model (deleting a chat model
    /// keeps these).
    pub async fn gen_model_files(&self) -> Result<BTreeSet<String>, StoreError> {
        self.call(|c| gen_model_files(c)).await
    }

    /// Records that the user accepted a model licence. One row is kept per licence text:
    /// accepting the same SHA-256 again, for this model or another, is no error and keeps
    /// the first acceptance.
    pub async fn record_license_acceptance(&self, a: LicenseAcceptance) -> Result<(), StoreError> {
        self.call(move |c| {
            c.execute(
                "INSERT OR IGNORE INTO license_acceptances (sha256, license_id, catalog_id, \
                 accepted_at) VALUES (?1, ?2, ?3, ?4)",
                params![a.sha256, a.license_id, a.catalog_id, a.accepted_at],
            )?;
            Ok(())
        })
        .await
    }

    /// The SHA-256 of every model licence text the user accepted.
    pub async fn accepted_license_hashes(&self) -> Result<BTreeSet<String>, StoreError> {
        self.call(|c| accepted_license_hashes(c)).await
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::{
        CreationKind, CreationPatch, CreationStatus, ModelInput, ModelSource, NewCreationRun,
        RunsOn, Store, StoreError,
    };
    use serde_json::json;

    /// FLUX.2 klein's text encoder, which is also a common chat download.
    const ENCODER: &str = "/m/unsloth/Qwen3-4B-GGUF/q.gguf";
    const REVISION: &str = "0123456789abcdef0123456789abcdef01234567";
    const CARD: &str = "Radeon RX 7600 (Vulkan)";

    fn part(role: &str, file: &str) -> Value {
        json!({
            "role": role,
            "repo": "acme/models",
            "revision": REVISION,
            "path": file.rsplit('/').next(),
            "file": file,
            "size": 1_000,
            "sha256": "ab".repeat(32),
            "license": "apache-2.0",
        })
    }

    fn gen_model(id: &str) -> NewGenModel {
        NewGenModel {
            id: id.into(),
            catalog_id: Some(id.into()),
            name: format!("Model {id}"),
            kind: "image".into(),
            family: "flux2-klein-4b".into(),
            parts: json!([
                part("diffusion", &format!("/m/acme/{id}-GGUF/{id}.gguf")),
                part("llm", ENCODER),
                part("vae", "/m/acme/vae/vae.safetensors"),
            ]),
            size_bytes: 3_000,
            source: GenSource::Catalog,
        }
    }

    fn chat_model(path: &str) -> ModelInput {
        ModelInput {
            path: path.into(),
            repo: Some("unsloth/Qwen3-4B-GGUF".into()),
            file_name: "q.gguf".into(),
            size_bytes: 1_000,
            sha256: None,
            gguf_summary: json!({"architecture": "qwen3"}),
            source: ModelSource::Download,
        }
    }

    #[tokio::test]
    async fn an_installed_model_reads_back_with_its_parts() {
        let store = Store::open_in_memory().expect("open");
        store.insert_gen_model(gen_model("a")).await.expect("a");
        store
            .insert_gen_model(NewGenModel {
                catalog_id: None,
                kind: "video".into(),
                source: GenSource::Search,
                ..gen_model("hf:acme/b-GGUF:b.gguf")
            })
            .await
            .expect("b");

        let a = store
            .get_gen_model("a")
            .await
            .expect("get")
            .expect("installed");
        let want = gen_model("a");
        assert_eq!(
            (
                a.id.as_str(),
                a.catalog_id.as_deref(),
                a.name.as_str(),
                a.kind.as_str(),
                a.family.as_str(),
                a.size_bytes,
                a.source
            ),
            (
                "a",
                Some("a"),
                "Model a",
                "image",
                "flux2-klein-4b",
                3_000,
                GenSource::Catalog
            )
        );
        assert_eq!(a.parts, want.parts, "the parts read back as written");
        assert!(a.added_at > 0, "the store's clock");
        assert_eq!(store.get_gen_model("nothing").await.expect("get"), None);

        let ids = |models: Vec<GenModel>| models.into_iter().map(|m| m.id).collect::<Vec<_>>();
        assert_eq!(
            ids(store.list_gen_models().await.expect("list")),
            ["hf:acme/b-GGUF:b.gguf", "a"],
            "newest first"
        );

        // The same id again replaces the row, and it is now the newest.
        store
            .insert_gen_model(NewGenModel {
                name: "Model a, again".into(),
                ..gen_model("a")
            })
            .await
            .expect("again");
        let listed = store.list_gen_models().await.expect("list");
        assert_eq!(listed.len(), 2, "replaced, not added");
        assert_eq!(listed[0].id, "a");
        assert_eq!(listed[0].name, "Model a, again");

        let gone = store
            .delete_gen_model("a")
            .await
            .expect("delete")
            .expect("it was installed");
        assert_eq!(gone.name, "Model a, again");
        assert_eq!(
            gone.parts, want.parts,
            "what was deleted, for removing files"
        );
        assert_eq!(store.get_gen_model("a").await.expect("get"), None);
        assert_eq!(store.delete_gen_model("a").await.expect("again"), None);
        assert_eq!(
            ids(store.list_gen_models().await.expect("list")),
            ["hf:acme/b-GGUF:b.gguf"]
        );
    }

    #[tokio::test]
    async fn an_installed_model_needs_an_id_and_parts_with_files() {
        let store = Store::open_in_memory().expect("open");
        let refused = |m: NewGenModel| {
            let store = store.clone();
            async move { store.insert_gen_model(m).await }
        };
        assert!(matches!(
            refused(gen_model("")).await,
            Err(StoreError::InvalidInput(_))
        ));
        for parts in [
            json!({"file": ENCODER}),
            json!(["/m/a.gguf"]),
            json!([{"role": "diffusion"}]),
            json!([{"role": "diffusion", "file": ""}]),
            json!([{"role": "diffusion", "file": 7}]),
        ] {
            assert!(
                matches!(
                    refused(NewGenModel {
                        parts: parts.clone(),
                        ..gen_model("a")
                    })
                    .await,
                    Err(StoreError::InvalidInput(_))
                ),
                "{parts}"
            );
        }
        assert!(
            refused(NewGenModel {
                kind: "audio".into(),
                ..gen_model("a")
            })
            .await
            .is_err(),
            "audio is not a kind"
        );
        assert!(store.list_gen_models().await.expect("list").is_empty());
    }

    #[tokio::test]
    async fn file_uses_counts_both_tables() {
        let store = Store::open_in_memory().expect("open");
        // Two picture models share the encoder, and a chat model is the same file.
        store.insert_gen_model(gen_model("a")).await.expect("a");
        store.insert_gen_model(gen_model("b")).await.expect("b");
        store.upsert_model(chat_model(ENCODER)).await.expect("chat");

        assert_eq!(store.file_uses(ENCODER).await.unwrap(), 3);
        assert_eq!(
            store.file_uses("/m/acme/a-GGUF/a.gguf").await.unwrap(),
            1,
            "a model's own diffusion file"
        );
        assert_eq!(store.file_uses("/m/other.gguf").await.unwrap(), 0);
        assert_eq!(
            store.file_uses("q.gguf").await.unwrap(),
            0,
            "the whole path, not the repository path or the name"
        );

        store.delete_gen_model("a").await.unwrap();
        assert_eq!(store.file_uses(ENCODER).await.unwrap(), 2);
        assert_eq!(store.file_uses("/m/acme/a-GGUF/a.gguf").await.unwrap(), 0);
        assert!(store.gen_model_files().await.unwrap().contains(ENCODER));
        assert_eq!(
            store.gen_model_files().await.unwrap(),
            BTreeSet::from(
                [
                    "/m/acme/b-GGUF/b.gguf",
                    ENCODER,
                    "/m/acme/vae/vae.safetensors"
                ]
                .map(str::to_owned)
            ),
            "every part file once; the chat model's file is not a part"
        );

        store.delete_gen_model("b").await.unwrap();
        assert_eq!(store.file_uses(ENCODER).await.unwrap(), 1, "the chat model");
        assert!(store.gen_model_files().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn acceptances_are_kept_by_licence_hash() {
        let store = Store::open_in_memory().expect("open");
        assert!(store.accepted_license_hashes().await.unwrap().is_empty());
        let apache = LicenseAcceptance {
            sha256: "aa".repeat(32),
            license_id: "apache-2.0".into(),
            catalog_id: Some("flux2-klein-4b-q4".into()),
            accepted_at: 1_000,
        };
        let other = LicenseAcceptance {
            sha256: "bb".repeat(32),
            license_id: "flux-non-commercial".into(),
            catalog_id: None,
            accepted_at: 2_000,
        };
        store
            .record_license_acceptance(apache.clone())
            .await
            .unwrap();
        store
            .record_license_acceptance(other.clone())
            .await
            .unwrap();
        let both = BTreeSet::from([apache.sha256.clone(), other.sha256.clone()]);
        assert_eq!(store.accepted_license_hashes().await.unwrap(), both);

        // The same text accepted again, for another model or none, is no error.
        store
            .record_license_acceptance(LicenseAcceptance {
                catalog_id: Some("z-image-turbo".into()),
                accepted_at: 3_000,
                ..apache.clone()
            })
            .await
            .expect("accepted again");
        store
            .record_license_acceptance(other)
            .await
            .expect("a searched model's licence accepted again");
        assert_eq!(store.accepted_license_hashes().await.unwrap(), both);
    }

    fn local_run(id: &str, gen_model_id: &str, device: &str, created_at: i64) -> NewCreationRun {
        NewCreationRun {
            id: id.into(),
            kind: CreationKind::Image,
            runs_on: RunsOn::Local,
            provider_id: None,
            provider_name: "This computer".into(),
            provider_host: String::new(),
            model: gen_model_id.into(),
            model_label: format!("Model {gen_model_id}"),
            prompt: "a lighthouse at dusk".into(),
            params: json!({"size": "1024x1024", "steps": 4, "n": 1, "run": id}),
            refs: json!([]),
            estimate_usd: Some(0.0),
            gen_model_id: Some(gen_model_id.into()),
            engine_build: Some(1106),
            device: Some(device.into()),
            created_at,
        }
    }

    async fn ended(store: &Store, run: NewCreationRun, status: CreationStatus, elapsed_ms: u64) {
        let id = run.id.clone();
        store.insert_creation_run(run).await.expect("insert");
        store
            .update_creation_run(
                &id,
                CreationPatch {
                    status: Some(status),
                    elapsed_ms: Some(Some(elapsed_ms)),
                    ..CreationPatch::default()
                },
            )
            .await
            .expect("end");
    }

    #[tokio::test]
    async fn measured_times_are_this_models_finished_local_runs_on_this_device() {
        use CreationStatus::{Done, Failed};

        let store = Store::open_in_memory().expect("open");
        ended(&store, local_run("r1", "a", CARD, 10), Done, 30_000).await;
        ended(&store, local_run("r2", "a", "Processor", 20), Done, 90_000).await;
        ended(&store, local_run("r3", "a", CARD, 30), Failed, 5_000).await;
        // A cloud run never counts, even one carrying the same model and device.
        ended(
            &store,
            NewCreationRun {
                runs_on: RunsOn::Cloud,
                provider_name: "OpenRouter".into(),
                provider_host: "openrouter.ai".into(),
                ..local_run("r4", "a", CARD, 40)
            },
            Done,
            7_000,
        )
        .await;
        ended(&store, local_run("r5", "b", CARD, 50), Done, 60_000).await;
        // Finished, but with no time taken recorded.
        store
            .insert_creation_run(local_run("r6", "a", CARD, 60))
            .await
            .expect("insert");
        store
            .update_creation_run(
                "r6",
                CreationPatch {
                    status: Some(Done),
                    ..CreationPatch::default()
                },
            )
            .await
            .expect("done");

        assert_eq!(
            store
                .local_run_times("a", CARD, 10)
                .await
                .unwrap()
                .iter()
                .map(|r| r.elapsed_ms)
                .collect::<Vec<_>>(),
            [30_000]
        );
        let measured = store.local_run_times("a", CARD, 10).await.unwrap();
        assert_eq!(measured[0].params["run"], "r1", "the run's settings");
        assert_eq!(measured[0].created_at, 10);
        assert_eq!(
            store
                .local_run_times("a", "Processor", 10)
                .await
                .unwrap()
                .iter()
                .map(|r| r.elapsed_ms)
                .collect::<Vec<_>>(),
            [90_000]
        );

        // Newest first, at most `limit`.
        ended(&store, local_run("r7", "a", CARD, 70), Done, 31_000).await;
        ended(&store, local_run("r8", "a", CARD, 80), Done, 32_000).await;
        let times = |limit| {
            let store = store.clone();
            async move {
                store
                    .local_run_times("a", CARD, limit)
                    .await
                    .unwrap()
                    .iter()
                    .map(|r| r.elapsed_ms)
                    .collect::<Vec<_>>()
            }
        };
        assert_eq!(times(10).await, [32_000, 31_000, 30_000]);
        assert_eq!(times(2).await, [32_000, 31_000]);

        // A deleted run (a spending stub) is not measured.
        store
            .delete_creation_runs(vec!["r8".into()], 0)
            .await
            .expect("delete");
        assert_eq!(times(10).await, [31_000, 30_000]);
    }
}
