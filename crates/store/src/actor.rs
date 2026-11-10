//! The database thread: one OS thread owns the only `rusqlite::Connection`, and every
//! [`Store`] handle sends it closures over a channel and awaits the answer on a oneshot.
//!
//! Why a hand-rolled actor rather than `tokio-rusqlite`: it is the same amount of code and
//! removes one crate that lags rusqlite releases (docs/research/platform-crates.recheck.md).
//! All writes are therefore serialised, which is what SQLite wants anyway, and no async
//! task ever blocks on disk.

use std::fmt;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use rusqlite::Connection;
use tokio::sync::oneshot;

use crate::error::StoreError;
use crate::schema::MIGRATIONS;

/// Work sent to the database thread.
type Job = Box<dyn FnOnce(&mut Connection) + Send + 'static>;

/// How long SQLite waits on a lock held by another connection before giving up.
const BUSY_TIMEOUT: Duration = Duration::from_millis(5000);

/// Handle to the app database. Cheap to clone; every clone talks to the same thread.
///
/// The thread stops, and the connection is closed, when the last clone is dropped.
#[derive(Clone)]
pub struct Store {
    tx: mpsc::Sender<Job>,
}

impl fmt::Debug for Store {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Store").finish_non_exhaustive()
    }
}

impl Store {
    /// Opens (creating if needed) the database file at `path`, sets the connection pragmas
    /// and applies pending migrations, all on the dedicated database thread.
    ///
    /// The parent folder is created if it does not exist. The path must be on a local disk:
    /// SQLite's WAL mode does not work over a network share (ARCHITECTURE.md section 12).
    pub async fn open(path: &Path) -> Result<Store, StoreError> {
        let path: PathBuf = path.to_path_buf();
        let (init_tx, init_rx) = oneshot::channel();
        let store = spawn(
            move || open_file(&path),
            move |result| {
                // The receiver only disappears if the caller's future was dropped.
                let _ = init_tx.send(result);
            },
        )?;
        init_rx.await.map_err(|_| StoreError::Closed)??;
        Ok(store)
    }

    /// Opens a fresh in-memory database with the full schema. For tests and previews.
    ///
    /// This waits (briefly, synchronously) for the database thread to finish setting up.
    pub fn open_in_memory() -> Result<Store, StoreError> {
        let (init_tx, init_rx) = mpsc::sync_channel(1);
        let store = spawn(
            || {
                let mut conn = Connection::open_in_memory()?;
                configure(&mut conn, false)?;
                Ok(conn)
            },
            move |result| {
                let _ = init_tx.send(result);
            },
        )?;
        init_rx.recv().map_err(|_| StoreError::Closed)??;
        Ok(store)
    }

    /// Runs `f` on the database thread and returns its result.
    ///
    /// A panic inside `f` is caught and reported as [`StoreError::Panicked`]; the thread
    /// keeps serving other calls. If the thread has stopped, the result is
    /// [`StoreError::Closed`].
    pub(crate) async fn call<T, F>(&self, f: F) -> Result<T, StoreError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, StoreError> + Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let job: Job = Box::new(move |conn: &mut Connection| {
            let result = match std::panic::catch_unwind(AssertUnwindSafe(|| f(conn))) {
                Ok(result) => result,
                Err(_) => {
                    roll_back_after_panic(conn);
                    Err(StoreError::Panicked)
                }
            };
            // The caller may have stopped waiting; the work is done either way.
            let _ = tx.send(result);
        });
        self.tx.send(job).map_err(|_| StoreError::Closed)?;
        rx.await.map_err(|_| StoreError::Closed)?
    }
}

/// After a job panicked: a `rusqlite::Transaction` rolls back when it is dropped during
/// unwinding, but a transaction opened with plain SQL would stay open, and every later
/// write would then sit uncommitted until the connection closed (and be lost).
fn roll_back_after_panic(conn: &Connection) {
    if !conn.is_autocommit()
        && let Err(err) = conn.execute_batch("ROLLBACK")
    {
        tracing::warn!(error = %err, "rolling back after a panicked query failed");
    }
}

/// Starts the database thread. `open` runs on the thread; its outcome goes to `report`.
fn spawn<O, R>(open: O, report: R) -> Result<Store, StoreError>
where
    O: FnOnce() -> Result<Connection, StoreError> + Send + 'static,
    R: FnOnce(Result<(), StoreError>) + Send + 'static,
{
    let (tx, rx) = mpsc::channel::<Job>();
    std::thread::Builder::new()
        .name("store-db".into())
        .spawn(move || {
            let mut conn = match open() {
                Ok(conn) => {
                    report(Ok(()));
                    conn
                }
                Err(err) => {
                    report(Err(err));
                    return;
                }
            };
            while let Ok(job) = rx.recv() {
                job(&mut conn);
            }
            if let Err((_, err)) = conn.close() {
                tracing::warn!(error = %err, "closing the database failed");
            }
        })?;
    Ok(Store { tx })
}

/// Opens the database file and prepares it.
fn open_file(path: &Path) -> Result<Connection, StoreError> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let mut conn = Connection::open(path)?;
    configure(&mut conn, true)?;
    Ok(conn)
}

/// Sets the connection pragmas and applies migrations.
fn configure(conn: &mut Connection, file_backed: bool) -> Result<(), StoreError> {
    conn.busy_timeout(BUSY_TIMEOUT)?;
    let mode: String =
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))?;
    if file_backed && !mode.eq_ignore_ascii_case("wal") {
        // Happens on file systems without shared-memory support (network shares).
        tracing::warn!(journal_mode = %mode, "SQLite refused WAL mode; continuing without it");
    }
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    // Makes the row deletions done by `INSERT OR REPLACE` fire DELETE triggers, so a
    // REPLACE cannot get round audit_log's append-only trigger. None of this schema's
    // triggers writes to a table that has triggers, so nothing recurses.
    conn.pragma_update(None, "recursive_triggers", "ON")?;
    MIGRATIONS.to_latest(conn)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn file_database_uses_wal_and_foreign_keys() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("app.sqlite3");
        let store = Store::open(&path).await.expect("open");
        let (mode, fk, sync, busy) = store
            .call(|conn| {
                let mode: String = conn.pragma_query_value(None, "journal_mode", |r| r.get(0))?;
                let fk: i64 = conn.pragma_query_value(None, "foreign_keys", |r| r.get(0))?;
                let sync: i64 = conn.pragma_query_value(None, "synchronous", |r| r.get(0))?;
                let busy: i64 = conn.pragma_query_value(None, "busy_timeout", |r| r.get(0))?;
                Ok((mode, fk, sync, busy))
            })
            .await
            .expect("pragmas");
        assert_eq!(mode.to_ascii_lowercase(), "wal");
        assert_eq!(fk, 1);
        assert_eq!(sync, 1, "synchronous=NORMAL is 1");
        assert_eq!(busy, 5000);
        assert!(path.exists());
        let recursive: i64 = store
            .call(|conn| Ok(conn.pragma_query_value(None, "recursive_triggers", |r| r.get(0))?))
            .await
            .expect("recursive_triggers");
        assert_eq!(recursive, 1);
    }

    #[tokio::test]
    async fn reopening_keeps_data_and_schema_version() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("app.sqlite3");
        {
            let store = Store::open(&path).await.expect("open");
            store.set_setting("k", &42_u32).await.expect("set");
        }
        // The first thread exits once its last handle is gone; reopening must still work
        // even if it is closing concurrently (busy_timeout covers the overlap).
        let store = Store::open(&path).await.expect("reopen");
        let v: Option<u32> = store.get_setting("k").await.expect("get");
        assert_eq!(v, Some(42));
        let version: i64 = store
            .call(|conn| Ok(conn.pragma_query_value(None, "user_version", |r| r.get(0))?))
            .await
            .expect("user_version");
        assert!(version >= 1, "migrations ran exactly once: {version}");
    }

    #[tokio::test]
    async fn panic_in_job_is_reported_and_thread_survives() {
        let store = Store::open_in_memory().expect("open");
        let err = store
            .call(|_conn| -> Result<(), StoreError> { panic!("boom") })
            .await
            .expect_err("panics are errors");
        assert!(matches!(err, StoreError::Panicked));
        store
            .set_setting("still", &true)
            .await
            .expect("thread alive");
    }

    #[tokio::test]
    async fn panic_inside_an_open_transaction_does_not_strand_later_writes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("app.sqlite3");
        {
            let store = Store::open(&path).await.expect("open");
            let err = store
                .call(|conn| -> Result<(), StoreError> {
                    conn.execute_batch("BEGIN")?;
                    conn.execute(
                        "INSERT INTO settings (key, value, updated_at) VALUES ('lost', '1', 0)",
                        [],
                    )?;
                    panic!("boom inside a transaction")
                })
                .await
                .expect_err("panics are errors");
            assert!(matches!(err, StoreError::Panicked));
            let autocommit = store
                .call(|conn| Ok(conn.is_autocommit()))
                .await
                .expect("autocommit");
            assert!(autocommit, "the half-done transaction was rolled back");
            store.set_setting("kept", &1_u8).await.expect("set");
        }
        // A fresh connection sees the later write (committed) and not the rolled-back one.
        let store = Store::open(&path).await.expect("reopen");
        assert_eq!(store.get_setting::<u8>("kept").await.expect("get"), Some(1));
        assert_eq!(store.get_setting::<u8>("lost").await.expect("get"), None);
    }

    #[tokio::test]
    async fn stopped_thread_is_an_error_not_a_panic() {
        // A handle whose thread is gone: the receiver is dropped.
        let (tx, rx) = mpsc::channel::<Job>();
        drop(rx);
        let store = Store { tx };
        let err = store.get_setting::<u32>("k").await.expect_err("closed");
        assert!(matches!(err, StoreError::Closed));

        // A thread that drops the job without answering.
        let (tx, rx) = mpsc::channel::<Job>();
        let store = Store { tx };
        let consumer = std::thread::spawn(move || {
            while let Ok(job) = rx.recv() {
                drop(job);
            }
        });
        let err = store.get_setting::<u32>("k").await.expect_err("unanswered");
        assert!(matches!(err, StoreError::Closed));
        drop(store);
        consumer
            .join()
            .expect("consumer exits when the last handle is dropped");
    }

    #[tokio::test]
    async fn database_from_newer_app_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("app.sqlite3");
        {
            let conn = Connection::open(&path).expect("open raw");
            conn.pragma_update(None, "user_version", 999).expect("bump");
        }
        let err = Store::open(&path).await.expect_err("too new");
        assert!(matches!(err, StoreError::Migration(_)), "{err:?}");
    }

    #[tokio::test]
    async fn open_fails_cleanly_on_a_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = Store::open(dir.path())
            .await
            .expect_err("a folder is not a database");
        assert!(
            matches!(err, StoreError::Sqlite(_) | StoreError::Io(_)),
            "{err:?}"
        );
    }
}
