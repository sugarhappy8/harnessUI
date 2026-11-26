//! LoRAs for the Studio engine (app 0.2.0).
//!
//! - A **LoRA** is one row of `gen_loras`: a file in the models folder's `@loras` folder,
//!   downloaded from a Hugging Face search (pinned by its repository, revision, path, size
//!   and SHA-256) or imported from this computer.
//! - Its **name** (`rel_name`) is its path relative to that folder, with `/` separators
//!   and no extension: the `NAME` of the engine's `<lora:NAME:W>` tag with
//!   `--lora-model-dir` set to the folder. No two LoRAs have the same name.
//! - Its file counts as used by [`Store::gen_model_files`], so deleting a picture or video
//!   model never removes it.

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use crate::Store;
use crate::error::StoreError;
use crate::ids::{now_ms, sql_enum, sql_u64};

sql_enum!(
    /// How a LoRA file stores its weights, as the engine reads them.
    LoraFileFormat, "LoRA file format" {
        /// Kohya's `lora_unet_…` / `lora_te_…` names.
        Kohya => "kohya",
        /// PEFT's (diffusers') `lora_A` / `lora_B` names.
        Peft => "peft",
        /// LyCORIS (LoHa, LoKr and the like).
        Lycoris => "lycoris",
        /// A full weight difference.
        Diff => "diff",
    }
);

sql_enum!(
    /// Where a LoRA came from.
    LoraSource, "LoRA source" {
        /// A Hugging Face search.
        Search => "search",
        /// A file imported from this computer.
        Import => "import",
    }
);

/// Input for [`Store::insert_gen_lora`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewGenLora {
    /// `hf:<owner>/<repo>:<path>` for a searched LoRA, `local:<sha256>` for an imported one.
    pub id: String,
    /// The name as shown.
    pub name: String,
    /// The absolute path of its file, under `<models>/@loras`.
    pub file: String,
    /// Its path relative to `<models>/@loras`, with `/` separators and no extension: the
    /// name its engine tag carries. Unique.
    pub rel_name: String,
    /// The Hugging Face repository it was downloaded from; `None` when imported.
    pub repo: Option<String>,
    /// The commit it was downloaded at; `None` when imported.
    pub revision: Option<String>,
    /// Its path in the repository; `None` when imported.
    pub path: Option<String>,
    /// Its file's size.
    pub size_bytes: u64,
    /// Hex SHA-256 of its file.
    pub sha256: String,
    /// The engine version word of the models it was made for, when known.
    pub base_hint: Option<String>,
    /// How it stores its weights.
    pub format: LoraFileFormat,
    /// A Hugging Face search or an imported file.
    pub source: LoraSource,
}

/// A row of `gen_loras`: a LoRA Studio's engine can apply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenLora {
    /// `hf:<owner>/<repo>:<path>` for a searched LoRA, `local:<sha256>` for an imported one.
    pub id: String,
    /// The name as shown.
    pub name: String,
    /// The absolute path of its file, under `<models>/@loras`.
    pub file: String,
    /// Its path relative to `<models>/@loras`, with `/` separators and no extension: the
    /// name its engine tag carries. Unique.
    pub rel_name: String,
    /// The Hugging Face repository it was downloaded from; `None` when imported.
    pub repo: Option<String>,
    /// The commit it was downloaded at; `None` when imported.
    pub revision: Option<String>,
    /// Its path in the repository; `None` when imported.
    pub path: Option<String>,
    /// Its file's size.
    pub size_bytes: u64,
    /// Hex SHA-256 of its file.
    pub sha256: String,
    /// The engine version word of the models it was made for, when known.
    pub base_hint: Option<String>,
    /// How it stores its weights.
    pub format: LoraFileFormat,
    /// A Hugging Face search or an imported file.
    pub source: LoraSource,
    /// Unix ms when it was added (or last added again).
    pub added_at: i64,
}

const GEN_LORA_COLUMNS: &str = "id, name, file, rel_name, repo, revision, path, size_bytes, \
     sha256, base_hint, format, source, added_at";

/// Characters the engine's `<lora:NAME:W>` tag cannot carry in a name (correction 14).
/// `runtime::sd::args::valid_lora_name` is the authority on LoRA names (it also refuses `\`,
/// control characters, an absolute name and `.` or `..` parts) and checks every name again
/// before a run; keep these four in step with it, and with the app's
/// `studio::loras::sanitize_lora_component`, which makes the names. (This crate does not
/// depend on `runtime`, so it cannot call it.)
const NOT_IN_A_NAME: [char; 4] = [':', '<', '>', '|'];

fn gen_lora_from_row(r: &Row<'_>) -> rusqlite::Result<GenLora> {
    Ok(GenLora {
        id: r.get(0)?,
        name: r.get(1)?,
        file: r.get(2)?,
        rel_name: r.get(3)?,
        repo: r.get(4)?,
        revision: r.get(5)?,
        path: r.get(6)?,
        size_bytes: r.get(7)?,
        sha256: r.get(8)?,
        base_hint: r.get(9)?,
        format: r.get(10)?,
        source: r.get(11)?,
        added_at: r.get(12)?,
    })
}

/// Refuses a LoRA without an id or a file, or whose name its engine tag could not carry.
fn check_lora(l: &NewGenLora) -> Result<(), StoreError> {
    if l.id.is_empty() || l.file.is_empty() {
        return Err(StoreError::InvalidInput(
            "a LoRA needs an id and a file".into(),
        ));
    }
    if l.rel_name.is_empty() || l.rel_name.contains(NOT_IN_A_NAME) {
        return Err(StoreError::InvalidInput(format!(
            "a LoRA's name must not be empty or contain : < > or |: {:?}",
            l.rel_name
        )));
    }
    Ok(())
}

fn insert_gen_lora(conn: &mut Connection, l: NewGenLora) -> Result<GenLora, StoreError> {
    check_lora(&l)?;
    // The row of the same id goes first, so a LoRA added again is the newest; a name
    // another LoRA has is refused by the UNIQUE rule (REPLACE would delete that LoRA).
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM gen_loras WHERE id = ?1", [&l.id])?;
    let inserted = tx.execute(
        "INSERT INTO gen_loras (id, name, file, rel_name, repo, revision, path, size_bytes, \
         sha256, base_hint, format, source, added_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            l.id,
            l.name,
            l.file,
            l.rel_name,
            l.repo,
            l.revision,
            l.path,
            sql_u64("size_bytes", l.size_bytes)?,
            l.sha256,
            l.base_hint,
            l.format,
            l.source,
            now_ms()
        ],
    );
    match inserted {
        Ok(_) => {}
        Err(e) if StoreError::is_unique_violation(&e) => {
            return Err(StoreError::Conflict(format!(
                "another LoRA is already named {:?}",
                l.rel_name
            )));
        }
        Err(e) => return Err(e.into()),
    }
    let lora = get_gen_lora(&tx, &l.id)?
        .ok_or_else(|| StoreError::Corrupt("a LoRA just added cannot be read".into()))?;
    tx.commit()?;
    Ok(lora)
}

fn get_gen_lora(conn: &Connection, id: &str) -> Result<Option<GenLora>, StoreError> {
    let sql = format!("SELECT {GEN_LORA_COLUMNS} FROM gen_loras WHERE id = ?1");
    Ok(conn.query_row(&sql, [id], gen_lora_from_row).optional()?)
}

fn list_gen_loras(conn: &Connection) -> Result<Vec<GenLora>, StoreError> {
    // The rowid breaks a tie between LoRAs added in the same millisecond.
    let sql =
        format!("SELECT {GEN_LORA_COLUMNS} FROM gen_loras ORDER BY added_at DESC, rowid DESC");
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map([], gen_lora_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn delete_gen_lora(conn: &mut Connection, id: &str) -> Result<Option<GenLora>, StoreError> {
    let tx = conn.transaction()?;
    let lora = get_gen_lora(&tx, id)?;
    if lora.is_some() {
        tx.execute("DELETE FROM gen_loras WHERE id = ?1", [id])?;
    }
    tx.commit()?;
    Ok(lora)
}

impl Store {
    /// Records a LoRA, replacing the row with the same id (a LoRA added again, which
    /// becomes the newest), and returns it as stored.
    ///
    /// An empty id or file, or a name that is empty or has `:`, `<`, `>` or `|` in it, is
    /// [`StoreError::InvalidInput`]; a name another LoRA has is [`StoreError::Conflict`],
    /// and changes nothing.
    pub async fn insert_gen_lora(&self, l: NewGenLora) -> Result<GenLora, StoreError> {
        self.call(move |c| insert_gen_lora(c, l)).await
    }

    /// Every LoRA, newest first.
    pub async fn list_gen_loras(&self) -> Result<Vec<GenLora>, StoreError> {
        self.call(|c| list_gen_loras(c)).await
    }

    /// LoRA `id`; `None` when there is none.
    pub async fn get_gen_lora(&self, id: &str) -> Result<Option<GenLora>, StoreError> {
        let id = id.to_owned();
        self.call(move |c| get_gen_lora(c, &id)).await
    }

    /// Forgets LoRA `id` and returns it as it was, so the caller can remove its file when
    /// nothing else uses it; `None` when there was none. The file stays on disk.
    pub async fn delete_gen_lora(&self, id: &str) -> Result<Option<GenLora>, StoreError> {
        let id = id.to_owned();
        self.call(move |c| delete_gen_lora(c, &id)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GenSource, NewGenModel, Store, StoreError};
    use serde_json::json;

    /// An imported LoRA as `id`, at `rel_name` under the LoRA folder.
    fn lora(id: &str, rel_name: &str) -> NewGenLora {
        NewGenLora {
            id: id.into(),
            name: rel_name.rsplit('/').next().unwrap_or(rel_name).into(),
            file: format!("/m/@loras/{rel_name}.safetensors"),
            rel_name: rel_name.into(),
            repo: None,
            revision: None,
            path: None,
            size_bytes: 1_000,
            sha256: "ab".repeat(32),
            base_hint: Some("sdxl".into()),
            format: LoraFileFormat::Kohya,
            source: LoraSource::Import,
        }
    }

    /// An imported LoRA whose file is `file`, under `/m/@loras/`.
    fn lora_at(file: &str) -> NewGenLora {
        let rel = file
            .strip_prefix("/m/@loras/")
            .expect("under the LoRA folder");
        let rel = rel.rsplit_once('.').map_or(rel, |(stem, _)| stem);
        NewGenLora {
            file: file.into(),
            ..lora(&format!("local:{rel}"), rel)
        }
    }

    #[tokio::test]
    async fn loras_round_trip_and_rel_names_are_unique() {
        let s = Store::open_in_memory().unwrap();
        s.insert_gen_lora(lora("local:1", "local/pixel art ü"))
            .await
            .unwrap();
        assert!(
            s.insert_gen_lora(lora("local:2", "local/pixel art ü"))
                .await
                .is_err()
        );
        assert_eq!(
            s.list_gen_loras().await.unwrap()[0].rel_name,
            "local/pixel art ü"
        );
    }

    /// Every field reads back as written; the same id again replaces the row and is the
    /// newest; a name another LoRA has is a conflict that changes nothing; delete returns
    /// what it removed.
    #[tokio::test]
    async fn a_lora_reads_back_is_replaced_by_its_id_and_deleted() {
        let s = Store::open_in_memory().unwrap();
        let searched = NewGenLora {
            repo: Some("someone/loras".into()),
            revision: Some("0123456789abcdef0123456789abcdef01234567".into()),
            path: Some("styles/日本語 スタイル.safetensors".into()),
            base_hint: None,
            format: LoraFileFormat::Lycoris,
            source: LoraSource::Search,
            ..lora(
                "hf:someone/loras:styles/日本語 スタイル.safetensors",
                "someone/loras/styles/日本語 スタイル",
            )
        };
        let written = s.insert_gen_lora(searched.clone()).await.unwrap();
        let back = s.get_gen_lora(&searched.id).await.unwrap().unwrap();
        assert_eq!(back, written);
        assert!(back.added_at > 0, "the store's clock");
        assert_eq!(
            NewGenLora {
                id: back.id,
                name: back.name,
                file: back.file,
                rel_name: back.rel_name,
                repo: back.repo,
                revision: back.revision,
                path: back.path,
                size_bytes: back.size_bytes,
                sha256: back.sha256,
                base_hint: back.base_hint,
                format: back.format,
                source: back.source,
            },
            searched
        );
        assert_eq!(s.get_gen_lora("local:none").await.unwrap(), None);

        s.insert_gen_lora(lora("local:1", "local/a")).await.unwrap();
        let ids = |loras: Vec<GenLora>| loras.into_iter().map(|l| l.id).collect::<Vec<_>>();
        assert_eq!(
            ids(s.list_gen_loras().await.unwrap()),
            ["local:1", searched.id.as_str()],
            "newest first"
        );

        let renamed = NewGenLora {
            format: LoraFileFormat::Peft,
            ..lora(
                "hf:someone/loras:styles/日本語 スタイル.safetensors",
                "local/b",
            )
        };
        s.insert_gen_lora(renamed.clone()).await.unwrap();
        let listed = s.list_gen_loras().await.unwrap();
        assert_eq!(listed.len(), 2, "replaced, not added");
        assert_eq!(
            (listed[0].id.as_str(), listed[0].rel_name.as_str()),
            (renamed.id.as_str(), "local/b"),
            "the newest"
        );
        assert!(matches!(
            s.insert_gen_lora(lora("local:3", "local/a")).await,
            Err(StoreError::Conflict(_))
        ));
        assert!(matches!(
            s.insert_gen_lora(lora("local:1", "local/b")).await,
            Err(StoreError::Conflict(_))
        ));
        assert_eq!(
            s.get_gen_lora("local:1").await.unwrap().unwrap().rel_name,
            "local/a"
        );
        assert_eq!(
            s.list_gen_loras().await.unwrap().len(),
            2,
            "nothing changed"
        );

        let gone = s.delete_gen_lora("local:1").await.unwrap().unwrap();
        assert_eq!(gone.file, "/m/@loras/local/a.safetensors");
        assert_eq!(s.delete_gen_lora("local:1").await.unwrap(), None);
        assert_eq!(ids(s.list_gen_loras().await.unwrap()), [renamed.id]);
    }

    /// A LoRA needs an id, a file and a name the engine's `<lora:NAME:W>` tag can carry.
    #[tokio::test]
    async fn a_lora_needs_an_id_a_file_and_a_name_the_engine_reads() {
        let s = Store::open_in_memory().unwrap();
        for bad in [
            NewGenLora {
                id: String::new(),
                ..lora("x", "local/x")
            },
            NewGenLora {
                file: String::new(),
                ..lora("x", "local/x")
            },
            lora("x", ""),
            lora("x", "local/a:b"),
            lora("x", "local/a<b"),
            lora("x", "local/a>b"),
            lora("x", "local/a|b"),
        ] {
            assert!(
                matches!(
                    s.insert_gen_lora(bad.clone()).await,
                    Err(StoreError::InvalidInput(_))
                ),
                "{bad:?}"
            );
        }
        assert!(s.list_gen_loras().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn lora_files_count_as_used_files() {
        let s = Store::open_in_memory().unwrap();
        s.insert_gen_lora(lora_at("/m/@loras/local/x.safetensors"))
            .await
            .unwrap();
        assert!(
            s.gen_model_files()
                .await
                .unwrap()
                .contains("/m/@loras/local/x.safetensors")
        );
        // `file_uses` counts it as a use too (S1 minor), compared exactly as stored.
        assert_eq!(
            s.file_uses("/m/@loras/local/x.safetensors").await.unwrap(),
            1
        );
        assert_eq!(s.file_uses("/m/@loras/local/x").await.unwrap(), 0);

        // With a model's part files, each once; a deleted LoRA's file no longer counts.
        s.insert_gen_model(NewGenModel {
            id: "a".into(),
            catalog_id: None,
            name: "A".into(),
            kind: "image".into(),
            family: "sdxl".into(),
            recipe_id: Some("sdxl-checkpoint".into()),
            parts: json!([{"role": "model", "file": "/m/a.safetensors"}]),
            size_bytes: 1,
            source: GenSource::Import,
        })
        .await
        .unwrap();
        assert_eq!(
            s.gen_model_files().await.unwrap(),
            ["/m/@loras/local/x.safetensors", "/m/a.safetensors"]
                .map(str::to_owned)
                .into(),
        );
        s.delete_gen_lora("local:local/x").await.unwrap();
        assert_eq!(
            s.gen_model_files().await.unwrap(),
            ["/m/a.safetensors".to_owned()].into()
        );
        assert_eq!(
            s.file_uses("/m/@loras/local/x.safetensors").await.unwrap(),
            0
        );
    }
}
