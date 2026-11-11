//! Secrets: provider API keys, the Hugging Face token, git host tokens and MCP env and
//! header values (ARCHITECTURE.md section 12). SQLite stores only a [`SecretRef`].
//!
//! Layers:
//! - [`SecretStore`]: the trait every backend implements. Calls block (a keychain call can
//!   wait on an OS prompt), so async code runs them with `tokio::task::spawn_blocking`.
//! - [`KeychainSecretStore`]: Windows Credential Manager or the macOS login keychain via
//!   `keyring-core` and the native store crates. Every operation is serialised through
//!   one process-wide mutex, because Credential Manager can lose concurrent updates and
//!   the published `windows-native-keyring-store` 1.1.0 has no lock of its own
//!   (docs/research/platform-crates.recheck.md). Windows entries use
//!   `persistence = Local` so they never roam with a domain profile.
//! - [`ChunkedSecretStore`]: splits values longer than about 2,000 bytes into numbered
//!   entries, because a Credential Manager blob holds at most 2,560 bytes. Its lock is per
//!   instance, so the app should create one store (e.g. [`platform_default`]) and share it.
//! - [`MemorySecretStore`]: in-process only, for tests and Linux development.
//!
//! Only one app instance should write secrets at a time; the single-instance plugin in the
//! shell provides that, since nothing here locks across processes.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

use rusqlite::types::{FromSql, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use serde::{Deserialize, Serialize};

/// Errors from a [`SecretStore`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SecretError {
    /// The key is empty, too long, holds control characters or (for the chunked layer)
    /// contains the reserved `#`.
    #[error("invalid secret key {key:?}: {reason}")]
    InvalidKey {
        /// The key as given.
        key: String,
        /// What is wrong with it.
        reason: String,
    },
    /// The value is larger than the backend (or the chunked layer) can hold.
    #[error("secret too long: {0}")]
    TooLong(String),
    /// The OS refused access (keychain locked, the user clicked Deny, missing entitlement).
    #[error("access to the credential store was refused: {0}")]
    NoAccess(String),
    /// No OS credential store is available on this platform or it failed to initialise.
    #[error("no credential store is available: {0}")]
    Unavailable(String),
    /// A stored value is not valid UTF-8 or its chunks do not fit together.
    #[error("stored secret is corrupt: {0}")]
    Corrupt(String),
    /// Any other failure reported by the platform store.
    #[error("credential store error: {0}")]
    Platform(String),
}

/// A place to keep secret strings by key.
///
/// Implementations must be safe to call from several threads. Calls may block.
pub trait SecretStore: Send + Sync {
    /// Returns the value for `key`, or `None` if there is none.
    fn get(&self, key: &str) -> Result<Option<String>, SecretError>;
    /// Stores `value` under `key`, replacing any earlier value.
    fn set(&self, key: &str, value: &str) -> Result<(), SecretError>;
    /// Removes `key`. Removing a key that does not exist succeeds.
    fn delete(&self, key: &str) -> Result<(), SecretError>;

    /// Removes `key` and reports whether it existed.
    ///
    /// The default reads the value first. Backends that can tell from the delete itself
    /// override it, so cleaning up never has to read a secret (on macOS a read of an item
    /// written by an earlier build can raise a keychain password prompt).
    fn remove(&self, key: &str) -> Result<bool, SecretError> {
        let existed = self.get(key)?.is_some();
        self.delete(key)?;
        Ok(existed)
    }
}

impl<T: SecretStore + ?Sized> SecretStore for Box<T> {
    fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        (**self).get(key)
    }
    fn set(&self, key: &str, value: &str) -> Result<(), SecretError> {
        (**self).set(key, value)
    }
    fn delete(&self, key: &str) -> Result<(), SecretError> {
        (**self).delete(key)
    }
    fn remove(&self, key: &str) -> Result<bool, SecretError> {
        (**self).remove(key)
    }
}

impl<T: SecretStore + ?Sized> SecretStore for Arc<T> {
    fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        (**self).get(key)
    }
    fn set(&self, key: &str, value: &str) -> Result<(), SecretError> {
        (**self).set(key, value)
    }
    fn delete(&self, key: &str) -> Result<(), SecretError> {
        (**self).delete(key)
    }
    fn remove(&self, key: &str) -> Result<bool, SecretError> {
        (**self).remove(key)
    }
}

/// The key of a secret, as stored in SQLite (e.g. `providers.secret_ref`). Never the value.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretRef(String);

impl SecretRef {
    /// Wraps a key.
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    /// The key.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consumes the reference and returns the key.
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Debug for SecretRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SecretRef").field(&self.0).finish()
    }
}

impl fmt::Display for SecretRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl ToSql for SecretRef {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.0.as_str()))
    }
}

impl FromSql for SecretRef {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        String::column_result(value).map(Self)
    }
}

/// Longest key accepted, in bytes. Windows allows 513 characters for the user name the key
/// becomes; this leaves room for the chunk suffix.
pub const MAX_KEY_BYTES: usize = 256;

/// Checks the rules every store applies to a key.
fn validate_key(key: &str) -> Result<(), SecretError> {
    let reason = if key.is_empty() {
        "empty"
    } else if key.len() > MAX_KEY_BYTES + 8 {
        "too long"
    } else if key.chars().any(char::is_control) {
        "contains a control character"
    } else {
        return Ok(());
    };
    Err(SecretError::InvalidKey {
        key: key.to_owned(),
        reason: reason.to_owned(),
    })
}

/// Locks a mutex, recovering the guard if another thread panicked while holding it.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

// ---------------------------------------------------------------------------------------
// Memory store

/// Keeps secrets in process memory. They are gone when the app quits.
#[derive(Default)]
pub struct MemorySecretStore {
    values: Mutex<HashMap<String, String>>,
    max_value_bytes: Option<usize>,
}

/// Shows only the entry count: a derived `Debug` would print every secret value.
impl fmt::Debug for MemorySecretStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemorySecretStore")
            .field("entries", &self.len())
            .field("max_value_bytes", &self.max_value_bytes)
            .finish()
    }
}

impl MemorySecretStore {
    /// An empty store with no size limit.
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty store that refuses values longer than `max` bytes, like Credential Manager.
    pub fn with_max_value_bytes(max: usize) -> Self {
        Self {
            values: Mutex::default(),
            max_value_bytes: Some(max),
        }
    }

    /// Number of stored entries (chunks count separately).
    pub fn len(&self) -> usize {
        lock(&self.values).len()
    }

    /// True when nothing is stored.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl SecretStore for MemorySecretStore {
    fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        validate_key(key)?;
        Ok(lock(&self.values).get(key).cloned())
    }

    fn set(&self, key: &str, value: &str) -> Result<(), SecretError> {
        validate_key(key)?;
        if let Some(max) = self.max_value_bytes.filter(|max| value.len() > *max) {
            return Err(SecretError::TooLong(format!(
                "{} bytes; this store holds at most {max}",
                value.len()
            )));
        }
        lock(&self.values).insert(key.to_owned(), value.to_owned());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), SecretError> {
        self.remove(key).map(|_| ())
    }

    fn remove(&self, key: &str) -> Result<bool, SecretError> {
        validate_key(key)?;
        Ok(lock(&self.values).remove(key).is_some())
    }
}

// ---------------------------------------------------------------------------------------
// Chunking layer

/// Default payload per entry. With the header the first entry stays under 2,100 bytes,
/// well inside Credential Manager's 2,560-byte blob limit.
pub const DEFAULT_CHUNK_BYTES: usize = 2000;

/// Most entries one value may use (about 128 KB at the default chunk size).
pub const MAX_CHUNKS: usize = 64;

/// Marks a head entry. Starts with NUL so no ordinary secret collides with it; a value that
/// does start with it is stored in chunked form, which keeps the encoding unambiguous.
const MAGIC: &str = "\u{0}hsc1:";

/// Splits long values across numbered entries of an inner [`SecretStore`].
///
/// A value of at most `chunk_bytes` is stored as is under `key`. A longer one is cut (on
/// character boundaries) into `n` parts: `key` holds a header with `n` and a checksum
/// followed by part 0, and `key#1` … `key#(n-1)` hold the rest. Writes go tail-first and
/// head-last, and [`get`](SecretStore::get) verifies the checksum, so an interrupted write
/// reads as an error, never as a mix of old and new parts. Keys may not contain `#`.
#[derive(Debug)]
pub struct ChunkedSecretStore<S> {
    inner: S,
    chunk_bytes: usize,
    lock: Mutex<()>,
}

impl<S: SecretStore> ChunkedSecretStore<S> {
    /// Wraps `inner` with the default chunk size.
    pub fn new(inner: S) -> Self {
        Self::with_chunk_size(inner, DEFAULT_CHUNK_BYTES)
    }

    /// Wraps `inner` with a custom chunk size (at least 16 bytes).
    pub fn with_chunk_size(inner: S, chunk_bytes: usize) -> Self {
        Self {
            inner,
            chunk_bytes: chunk_bytes.max(16),
            lock: Mutex::new(()),
        }
    }

    /// The wrapped store.
    pub fn inner(&self) -> &S {
        &self.inner
    }

    /// Removes `key#start`, `key#start+1`, … until one is missing. Uses
    /// [`SecretStore::remove`], so no part is read just to find out whether it exists.
    ///
    /// Parts are only ever written in ascending order, so they form one run from `#1`. If
    /// the process dies part-way through this loop, the parts after the gap stay behind as
    /// unreferenced entries: never returned by `get` (the head's count and checksum decide
    /// what is read), and removed by the next write that reaches them.
    fn delete_parts_from(&self, key: &str, start: usize) -> Result<(), SecretError> {
        for i in start..=MAX_CHUNKS {
            if !self.inner.remove(&part_key(key, i))? {
                break;
            }
        }
        Ok(())
    }
}

fn part_key(key: &str, index: usize) -> String {
    format!("{key}#{index}")
}

/// 64-bit FNV-1a: detects parts from different writes; not a security measure.
fn checksum(value: &str) -> u64 {
    value.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, b| {
        (hash ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// Cuts `value` into pieces of at most `max` bytes on character boundaries.
fn split_parts(value: &str, max: usize) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut rest = value;
    while !rest.is_empty() {
        let mut end = rest.len().min(max);
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        let (head, tail) = rest.split_at(end);
        parts.push(head);
        rest = tail;
    }
    if parts.is_empty() {
        parts.push("");
    }
    parts
}

/// Parses a head entry: `(count, checksum, part 0)`.
fn parse_head(head: &str) -> Result<(usize, u64, &str), SecretError> {
    let corrupt = |why: &str| SecretError::Corrupt(format!("chunk header {why}"));
    let rest = head.strip_prefix(MAGIC).ok_or_else(|| corrupt("missing"))?;
    let (header, first) = rest
        .split_once('\n')
        .ok_or_else(|| corrupt("unterminated"))?;
    let (count, sum) = header.split_once(':').ok_or_else(|| corrupt("malformed"))?;
    let count: usize = count.parse().map_err(|_| corrupt("has a bad count"))?;
    if count == 0 || count > MAX_CHUNKS {
        return Err(corrupt("count out of range"));
    }
    let sum = u64::from_str_radix(sum, 16).map_err(|_| corrupt("has a bad checksum"))?;
    Ok((count, sum, first))
}

impl<S: SecretStore> SecretStore for ChunkedSecretStore<S> {
    fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        validate_chunk_key(key)?;
        let _guard = lock(&self.lock);
        let Some(head) = self.inner.get(key)? else {
            return Ok(None);
        };
        if !head.starts_with(MAGIC) {
            return Ok(Some(head));
        }
        let (count, sum, first) = parse_head(&head)?;
        // Sized from what was actually read, not from `chunk_bytes`, which the caller
        // chooses and which can make `count * chunk_bytes` overflow.
        let mut value = String::with_capacity(first.len().saturating_mul(count));
        value.push_str(first);
        for i in 1..count {
            let part = self
                .inner
                .get(&part_key(key, i))?
                .ok_or_else(|| SecretError::Corrupt(format!("part {i} of {count} is missing")))?;
            value.push_str(&part);
        }
        if checksum(&value) != sum {
            return Err(SecretError::Corrupt(
                "parts do not match the header checksum".into(),
            ));
        }
        Ok(Some(value))
    }

    fn set(&self, key: &str, value: &str) -> Result<(), SecretError> {
        validate_chunk_key(key)?;
        let _guard = lock(&self.lock);
        if value.len() <= self.chunk_bytes && !value.starts_with(MAGIC) {
            self.inner.set(key, value)?;
            return self.delete_parts_from(key, 1);
        }
        let parts = split_parts(value, self.chunk_bytes);
        if parts.len() > MAX_CHUNKS {
            return Err(SecretError::TooLong(format!(
                "{} bytes; at most {} bytes fit in {MAX_CHUNKS} entries",
                value.len(),
                MAX_CHUNKS.saturating_mul(self.chunk_bytes)
            )));
        }
        for (i, part) in parts.iter().enumerate().skip(1) {
            self.inner.set(&part_key(key, i), part)?;
        }
        let head = format!(
            "{MAGIC}{}:{:016x}\n{}",
            parts.len(),
            checksum(value),
            parts[0]
        );
        self.inner.set(key, &head)?;
        self.delete_parts_from(key, parts.len())
    }

    fn delete(&self, key: &str) -> Result<(), SecretError> {
        self.remove(key).map(|_| ())
    }

    /// Removes the value and all its parts without reading them, so a corrupt value can
    /// always be removed. Reports whether the head entry existed.
    fn remove(&self, key: &str) -> Result<bool, SecretError> {
        validate_chunk_key(key)?;
        let _guard = lock(&self.lock);
        // Head first: an interrupted delete leaves no head pointing at missing parts.
        let existed = self.inner.remove(key)?;
        self.delete_parts_from(key, 1)?;
        Ok(existed)
    }
}

fn validate_chunk_key(key: &str) -> Result<(), SecretError> {
    validate_key(key)?;
    if key.len() > MAX_KEY_BYTES {
        return Err(SecretError::InvalidKey {
            key: key.to_owned(),
            reason: "too long".into(),
        });
    }
    if key.contains('#') {
        return Err(SecretError::InvalidKey {
            key: key.to_owned(),
            reason: "'#' is reserved for chunk entries".into(),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// OS keychain

/// Windows Credential Manager or the macOS login keychain, through `keyring-core`.
///
/// Each secret is one generic credential with service = the app identifier given at
/// construction (decision D17) and user = the key. Values are stored as UTF-8 bytes
/// (`set_secret`), which on Windows allows the full 2,560-byte blob rather than 1,280
/// UTF-16 units. Wrap it in [`ChunkedSecretStore`] for longer values.
///
/// Every call, from every instance in the process, is serialised through one process-wide
/// mutex (ARCHITECTURE.md section 12): Credential Manager can lose concurrent updates and
/// the published `windows-native-keyring-store` 1.1.0 has no lock of its own.
///
/// On macOS, an unsigned or ad-hoc-signed build is expected to trigger a login-password
/// prompt per entry after every rebuild (keychain ACLs follow the code signature). A Deny
/// or Cancel there (`errSecUserCanceled`, `errSecAuthFailed`), or a keychain that cannot
/// show a prompt (`errSecInteractionNotAllowed`), surfaces as [`SecretError::NoAccess`].
/// Not verified on real hardware yet.
pub struct KeychainSecretStore {
    service: String,
    backend: Result<Arc<keyring_core::CredentialStore>, String>,
    modifiers: &'static [(&'static str, &'static str)],
}

/// Serialises every keychain call in the process, whichever instance makes it.
static KEYCHAIN_LOCK: Mutex<()> = Mutex::new(());

impl fmt::Debug for KeychainSecretStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeychainSecretStore")
            .field("service", &self.service)
            .field("available", &self.backend.is_ok())
            .finish_non_exhaustive()
    }
}

/// Entry modifiers for the platform store: local (non-roaming) persistence on Windows.
#[cfg(windows)]
const PLATFORM_MODIFIERS: &[(&str, &str)] = &[("persistence", "Local")];
#[cfg(not(windows))]
const PLATFORM_MODIFIERS: &[(&str, &str)] = &[];

#[cfg(windows)]
fn native_store() -> Result<Arc<keyring_core::CredentialStore>, String> {
    windows_native_keyring_store::Store::new()
        .map(|s| s as Arc<keyring_core::CredentialStore>)
        .map_err(|e| e.to_string())
}

#[cfg(target_os = "macos")]
fn native_store() -> Result<Arc<keyring_core::CredentialStore>, String> {
    // The legacy (file-based) keychain works without a provisioning profile; the
    // `protected` store would fail with errSecMissingEntitlement (-34018).
    apple_native_keyring_store::keychain::Store::new()
        .map(|s| s as Arc<keyring_core::CredentialStore>)
        .map_err(|e| e.to_string())
}

#[cfg(not(any(windows, target_os = "macos")))]
fn native_store() -> Result<Arc<keyring_core::CredentialStore>, String> {
    Err("this build has no OS keychain support on this platform".into())
}

/// True when a platform failure means the user (or the OS on their behalf) refused access.
///
/// keyring-core passes these through as `PlatformFailure`, so the macOS status code is
/// read from the wrapped `security_framework` error.
#[cfg(target_os = "macos")]
fn access_refused(err: &(dyn std::error::Error + Send + Sync + 'static)) -> bool {
    /// errSecUserCanceled, errSecAuthFailed, errSecInteractionNotAllowed,
    /// errSecInteractionRequired.
    const REFUSED: [i32; 4] = [-128, -25293, -25308, -25315];
    err.downcast_ref::<security_framework::base::Error>()
        .is_some_and(|e| REFUSED.contains(&e.code()))
}

#[cfg(not(target_os = "macos"))]
fn access_refused(_err: &(dyn std::error::Error + Send + Sync + 'static)) -> bool {
    // Windows reports refusals as NoStorageAccess already, and has no prompts.
    false
}

impl KeychainSecretStore {
    /// The platform's keychain, with entries under `service` (the app identifier).
    ///
    /// Never fails: if the platform store cannot be created (or the platform has none, or
    /// `service` is empty), every call returns [`SecretError::Unavailable`].
    pub fn new(service: &str) -> Self {
        let backend = if service.is_empty() {
            Err("the keychain service name is empty".to_owned())
        } else {
            native_store()
        };
        Self {
            service: service.to_owned(),
            backend,
            modifiers: PLATFORM_MODIFIERS,
        }
    }

    /// Uses the given `keyring-core` store (for example its mock store in tests), with no
    /// entry modifiers.
    pub fn with_credential_store(service: &str, store: Arc<keyring_core::CredentialStore>) -> Self {
        Self {
            service: service.to_owned(),
            backend: Ok(store),
            modifiers: &[],
        }
    }

    /// True when a platform store was created.
    pub fn is_available(&self) -> bool {
        self.backend.is_ok()
    }

    fn entry(&self, key: &str) -> Result<keyring_core::Entry, SecretError> {
        validate_key(key)?;
        let store = self
            .backend
            .as_ref()
            .map_err(|e| SecretError::Unavailable(e.clone()))?;
        let modifiers: HashMap<&str, &str> = self.modifiers.iter().copied().collect();
        let modifiers = (!modifiers.is_empty()).then_some(&modifiers);
        store
            .build(&self.service, key, modifiers)
            .map_err(|e| map_keyring_error(key, e))
    }
}

/// Translates a keyring-core error for an operation on `key`. `NoEntry` is handled by the
/// callers. No variant's text carries the secret value.
fn map_keyring_error(key: &str, err: keyring_core::Error) -> SecretError {
    use keyring_core::Error as E;
    match err {
        E::NoStorageAccess(e) => SecretError::NoAccess(e.to_string()),
        E::PlatformFailure(e) if access_refused(e.as_ref()) => SecretError::NoAccess(e.to_string()),
        E::TooLong(attr, limit) => {
            SecretError::TooLong(format!("{attr} exceeds the platform limit of {limit}"))
        }
        E::Invalid(attr, why) => SecretError::InvalidKey {
            key: key.to_owned(),
            reason: format!("{attr} {why}"),
        },
        E::BadEncoding(_) => SecretError::Corrupt("the stored value is not UTF-8".into()),
        E::NoDefaultStore => SecretError::Unavailable("no credential store configured".into()),
        other => SecretError::Platform(other.to_string()),
    }
}

impl SecretStore for KeychainSecretStore {
    fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        let _guard = lock(&KEYCHAIN_LOCK);
        match self.entry(key)?.get_secret() {
            Ok(bytes) => String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| SecretError::Corrupt("the stored value is not UTF-8".into())),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(e) => Err(map_keyring_error(key, e)),
        }
    }

    fn set(&self, key: &str, value: &str) -> Result<(), SecretError> {
        let _guard = lock(&KEYCHAIN_LOCK);
        self.entry(key)?
            .set_secret(value.as_bytes())
            .map_err(|e| map_keyring_error(key, e))
    }

    fn delete(&self, key: &str) -> Result<(), SecretError> {
        self.remove(key).map(|_| ())
    }

    fn remove(&self, key: &str) -> Result<bool, SecretError> {
        let _guard = lock(&KEYCHAIN_LOCK);
        match self.entry(key)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring_core::Error::NoEntry) => Ok(false),
            Err(e) => Err(map_keyring_error(key, e)),
        }
    }
}

/// The secret store the app should use: the OS keychain (chunked) on Windows and macOS.
///
/// Elsewhere (Linux is a development platform only) it returns a chunked in-memory store
/// and logs a warning, because secrets will not survive a restart.
pub fn platform_default(service: &str) -> Box<dyn SecretStore> {
    if cfg!(any(windows, target_os = "macos")) {
        Box::new(ChunkedSecretStore::new(KeychainSecretStore::new(service)))
    } else {
        tracing::warn!(
            "no OS keychain on this platform: secrets are kept in memory and lost when the app quits"
        );
        Box::new(ChunkedSecretStore::new(MemorySecretStore::new()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn big_value(len: usize) -> String {
        // Mixed 1-, 2-, 3- and 4-byte characters so chunk cuts land inside multi-byte ones.
        let pattern = "aé京🦀";
        pattern
            .chars()
            .cycle()
            .scan(0, |n, c| {
                *n += c.len_utf8();
                (*n <= len).then_some(c)
            })
            .collect()
    }

    #[test]
    fn ten_kb_value_round_trips_through_a_limited_store() {
        let limited = MemorySecretStore::with_max_value_bytes(2560);
        let value = big_value(10 * 1024);
        assert!(value.len() > 10_000);
        assert!(matches!(
            limited.set("token", &value),
            Err(SecretError::TooLong(_))
        ));

        let store = ChunkedSecretStore::new(limited);
        store.set("token", &value).expect("set");
        assert_eq!(
            store.get("token").expect("get").as_deref(),
            Some(value.as_str())
        );
        let parts = store.inner().len();
        assert_eq!(parts, split_parts(&value, DEFAULT_CHUNK_BYTES).len());
        assert!(parts >= 6, "{parts} entries");
        let head = store.inner().get("token").expect("raw").expect("head");
        assert!(head.starts_with(MAGIC));

        store.delete("token").expect("delete");
        assert!(store.inner().is_empty(), "every part removed");
        assert_eq!(store.get("token").expect("get"), None);
    }

    #[test]
    fn shrinking_a_value_removes_stale_parts() {
        let store = ChunkedSecretStore::with_chunk_size(MemorySecretStore::new(), 16);
        store.set("k", &"x".repeat(100)).expect("long");
        assert_eq!(store.inner().len(), 7);
        store.set("k", &"y".repeat(40)).expect("shorter");
        assert_eq!(store.inner().len(), 3);
        assert_eq!(store.get("k").expect("get"), Some("y".repeat(40)));
        store.set("k", "short").expect("plain");
        assert_eq!(store.inner().len(), 1);
        assert_eq!(
            store.inner().get("k").expect("raw").as_deref(),
            Some("short")
        );
        assert_eq!(store.get("k").expect("get").as_deref(), Some("short"));
    }

    #[test]
    fn values_that_look_like_headers_are_escaped() {
        let store = ChunkedSecretStore::new(MemorySecretStore::new());
        let tricky = format!("{MAGIC}3:00\nnot really");
        store.set("k", &tricky).expect("set");
        assert_eq!(store.get("k").expect("get"), Some(tricky));
        store.set("empty", "").expect("empty");
        assert_eq!(store.get("empty").expect("get").as_deref(), Some(""));
    }

    #[test]
    fn mixed_generations_are_detected() {
        let store = ChunkedSecretStore::with_chunk_size(MemorySecretStore::new(), 16);
        store.set("k", &"a".repeat(48)).expect("first");
        // Simulate a crash after a new write replaced a tail part but not the head.
        store.inner().set("k#1", &"b".repeat(16)).expect("tamper");
        assert!(matches!(store.get("k"), Err(SecretError::Corrupt(_))));
        store.inner().delete("k#2").expect("lose a part");
        assert!(matches!(store.get("k"), Err(SecretError::Corrupt(_))));
    }

    #[test]
    fn too_many_chunks_is_refused() {
        let store = ChunkedSecretStore::with_chunk_size(MemorySecretStore::new(), 16);
        let err = store
            .set("k", &"z".repeat(16 * MAX_CHUNKS + 1))
            .expect_err("too long");
        assert!(matches!(err, SecretError::TooLong(_)));
        assert!(store.inner().is_empty(), "nothing written");
    }

    #[test]
    fn keys_are_validated() {
        let store = ChunkedSecretStore::new(MemorySecretStore::new());
        for bad in ["", "a#1", "line\nbreak", &"k".repeat(MAX_KEY_BYTES + 1)] {
            assert!(
                matches!(store.set(bad, "v"), Err(SecretError::InvalidKey { .. })),
                "{bad:?}"
            );
        }
        store.set("provider.3.api_key", "v").expect("dots are fine");
    }

    #[test]
    fn split_respects_char_boundaries() {
        let value = big_value(1000);
        for max in [16, 17, 18, 19] {
            let parts = split_parts(&value, max);
            assert!(parts.iter().all(|p| p.len() <= max && !p.is_empty()));
            assert_eq!(parts.concat(), value);
        }
        assert_eq!(split_parts("", 16), vec![""]);
    }

    #[test]
    fn keychain_store_over_keyring_mock() {
        let mock = keyring_core::mock::Store::new().expect("mock");
        let keychain =
            KeychainSecretStore::with_credential_store("io.github.sugarhappy8.harnessui", mock);
        assert!(keychain.is_available());
        assert_eq!(keychain.get("hf_token").expect("get"), None);
        keychain
            .delete("hf_token")
            .expect("deleting nothing is fine");
        keychain.set("hf_token", "hf_abc").expect("set");
        assert_eq!(
            keychain.get("hf_token").expect("get").as_deref(),
            Some("hf_abc")
        );

        let chunked = ChunkedSecretStore::new(keychain);
        let value = big_value(10 * 1024);
        chunked
            .set("mcp.github.env.TOKEN", &value)
            .expect("set long");
        assert_eq!(
            chunked.get("mcp.github.env.TOKEN").expect("get").as_deref(),
            Some(value.as_str())
        );
        chunked.delete("mcp.github.env.TOKEN").expect("delete");
        assert_eq!(chunked.get("mcp.github.env.TOKEN").expect("get"), None);
        assert_eq!(
            chunked.inner().get("mcp.github.env.TOKEN#1").expect("part"),
            None
        );
    }

    /// Counts reads, to prove the chunked layer's cleanup never reads a secret.
    #[derive(Default)]
    struct CountingStore {
        inner: MemorySecretStore,
        gets: std::sync::atomic::AtomicUsize,
    }

    impl SecretStore for CountingStore {
        fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
            self.gets.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.inner.get(key)
        }
        fn set(&self, key: &str, value: &str) -> Result<(), SecretError> {
            self.inner.set(key, value)
        }
        fn delete(&self, key: &str) -> Result<(), SecretError> {
            self.inner.delete(key)
        }
        fn remove(&self, key: &str) -> Result<bool, SecretError> {
            self.inner.remove(key)
        }
    }

    #[test]
    fn writes_and_deletes_never_read_stored_parts() {
        let store = ChunkedSecretStore::with_chunk_size(CountingStore::default(), 16);
        store.set("k", &"x".repeat(100)).expect("7 parts");
        store.set("k", &"y".repeat(40)).expect("shrink to 3 parts");
        store.set("k", "short").expect("plain");
        store.set("k", &"z".repeat(64)).expect("4 parts");
        store.delete("k").expect("delete");
        assert_eq!(
            store.inner().gets.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert!(store.inner().inner.is_empty(), "every part removed");
    }

    #[test]
    fn a_corrupt_value_can_still_be_removed() {
        let store = ChunkedSecretStore::with_chunk_size(MemorySecretStore::new(), 16);
        store.set("k", &"a".repeat(48)).expect("set");
        store.inner().set("k#1", &"b".repeat(16)).expect("tamper");
        assert!(matches!(store.get("k"), Err(SecretError::Corrupt(_))));
        assert!(store.remove("k").expect("remove"), "the head existed");
        assert!(store.inner().is_empty());
        assert!(!store.remove("k").expect("remove again"));
        assert_eq!(store.get("k").expect("get"), None);
    }

    #[test]
    fn a_huge_chunk_size_does_not_overflow() {
        let store = ChunkedSecretStore::with_chunk_size(MemorySecretStore::new(), usize::MAX);
        store.set("k", "value").expect("set");
        assert_eq!(store.get("k").expect("get").as_deref(), Some("value"));
        // A head written with a small chunk size reads back through a huge one.
        let small = ChunkedSecretStore::with_chunk_size(MemorySecretStore::new(), 16);
        small.set("k", &"q".repeat(100)).expect("chunked");
        let huge = ChunkedSecretStore::with_chunk_size(small.inner, usize::MAX);
        assert_eq!(huge.get("k").expect("get"), Some("q".repeat(100)));
    }

    #[test]
    fn default_remove_reports_existence() {
        /// Implements only the three required methods.
        struct Plain(MemorySecretStore);
        impl SecretStore for Plain {
            fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
                self.0.get(key)
            }
            fn set(&self, key: &str, value: &str) -> Result<(), SecretError> {
                self.0.set(key, value)
            }
            fn delete(&self, key: &str) -> Result<(), SecretError> {
                self.0.delete(key)
            }
        }
        let store = Plain(MemorySecretStore::new());
        store.set("k", "v").expect("set");
        assert!(store.remove("k").expect("remove"));
        assert!(!store.remove("k").expect("remove again"));
        let boxed: Box<dyn SecretStore> = Box::new(ChunkedSecretStore::new(store));
        boxed.set("k", "v").expect("set through box");
        assert!(boxed.remove("k").expect("remove through box"));
    }

    #[test]
    fn keychain_remove_and_error_mapping_over_the_mock() {
        let mock = keyring_core::mock::Store::new().expect("mock");
        let keychain = KeychainSecretStore::with_credential_store("svc", mock.clone());
        keychain.set("hf_token", "v").expect("set");
        assert!(keychain.remove("hf_token").expect("remove"));
        assert!(!keychain.remove("hf_token").expect("remove again"));

        // The mock hands out the same credential for the same service and user, so an error
        // injected here is returned by the store's next call on that key.
        let inject = |err: keyring_core::Error| {
            use keyring_core::api::CredentialStoreApi as _;
            let entry = mock.build("svc", "hf_token", None).expect("entry");
            let cred: &keyring_core::mock::Cred =
                entry.as_any().downcast_ref().expect("mock credential");
            cred.set_error(err);
        };
        inject(keyring_core::Error::NoStorageAccess(Box::new(
            std::io::Error::other("locked"),
        )));
        assert!(matches!(
            keychain.get("hf_token"),
            Err(SecretError::NoAccess(_))
        ));
        inject(keyring_core::Error::Invalid(
            "user".into(),
            "is not allowed".into(),
        ));
        match keychain.set("hf_token", "v") {
            Err(SecretError::InvalidKey { key, reason }) => {
                assert_eq!(key, "hf_token", "names the key, not the attribute");
                assert!(reason.contains("is not allowed"), "{reason}");
            }
            other => panic!("expected InvalidKey, got {other:?}"),
        }
        inject(keyring_core::Error::TooLong("secret".into(), 2560));
        assert!(matches!(
            keychain.set("hf_token", "v"),
            Err(SecretError::TooLong(_))
        ));
        inject(keyring_core::Error::BadEncoding(b"secret bytes".to_vec()));
        match keychain.get("hf_token") {
            Err(SecretError::Corrupt(why)) => assert!(!why.contains("secret bytes"), "{why}"),
            other => panic!("expected Corrupt, got {other:?}"),
        }
        inject(keyring_core::Error::PlatformFailure(Box::new(
            std::io::Error::other("boom"),
        )));
        assert!(matches!(
            keychain.remove("hf_token"),
            Err(SecretError::Platform(_))
        ));
        // Errors are one-shot in the mock; the store works again afterwards.
        keychain.set("hf_token", "after").expect("set");
        assert_eq!(
            keychain.get("hf_token").expect("get").as_deref(),
            Some("after")
        );
    }

    #[test]
    fn keychain_calls_from_many_threads_and_instances() {
        let mock = keyring_core::mock::Store::new().expect("mock");
        let threads: Vec<_> = (0..8)
            .map(|t| {
                let store = KeychainSecretStore::with_credential_store("svc", mock.clone());
                std::thread::spawn(move || {
                    let chunked = ChunkedSecretStore::with_chunk_size(store, 16);
                    for i in 0..20 {
                        let key = format!("t{t}.k{i}");
                        let value = format!("{t}-{i}-").repeat(10);
                        chunked.set(&key, &value).expect("set");
                        assert_eq!(chunked.get(&key).expect("get"), Some(value));
                        chunked.delete(&key).expect("delete");
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().expect("thread finished without panicking");
        }
    }

    #[test]
    fn empty_service_is_unavailable() {
        let store = KeychainSecretStore::new("");
        assert!(!store.is_available());
        assert!(matches!(store.get("k"), Err(SecretError::Unavailable(_))));
    }

    #[test]
    fn unavailable_keychain_reports_errors() {
        let store = KeychainSecretStore {
            service: "svc".into(),
            backend: Err("none here".into()),
            modifiers: &[],
        };
        assert!(!store.is_available());
        assert!(matches!(store.get("k"), Err(SecretError::Unavailable(_))));
        assert!(matches!(
            store.set("k", "v"),
            Err(SecretError::Unavailable(_))
        ));
    }

    #[test]
    fn platform_default_works_here() {
        let store = platform_default("io.github.sugarhappy8.harnessui.test");
        if cfg!(not(any(windows, target_os = "macos"))) {
            store.set("k", "v").expect("set");
            assert_eq!(store.get("k").expect("get").as_deref(), Some("v"));
            store.delete("k").expect("delete");
        }
    }

    #[test]
    fn debug_output_never_shows_values() {
        let store = ChunkedSecretStore::new(MemorySecretStore::new());
        store.set("hf_token", "hf_SUPERSECRET").expect("set");
        let shown = format!("{store:?}");
        assert!(!shown.contains("SUPERSECRET"), "{shown}");
        assert!(shown.contains("entries: 1"), "{shown}");
        let keychain = KeychainSecretStore::with_credential_store(
            "svc",
            keyring_core::mock::Store::new().expect("mock"),
        );
        keychain.set("k", "hf_SUPERSECRET").expect("set");
        assert!(!format!("{keychain:?}").contains("SUPERSECRET"));
    }

    #[test]
    fn secret_ref_serialises_as_a_plain_string() {
        let r = SecretRef::new("provider.1.api_key");
        assert_eq!(
            serde_json::to_string(&r).expect("json"),
            "\"provider.1.api_key\""
        );
        assert_eq!(r.to_string(), "provider.1.api_key");
        assert_eq!(r.clone().into_inner(), "provider.1.api_key");
    }
}
