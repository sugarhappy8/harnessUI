//! What the Studio manager reaches the world through, each behind a trait so its tests can
//! stand in for it: the endpoints and their keys ([`ProviderSource`]), the webview
//! ([`StudioSink`]), a company's file host ([`ForeignFetch`]), the clock ([`Clock`]) and, for
//! runs on this computer ([`LocalLane`]), the Studio engine (`LocalProvider`), the graphics
//! card's lease ([`CardLease`]) and the hardware ([`LocalHardware`]). The app's own versions
//! are [`StoreProviders`], [`AppSink`], [`GuardedFetch`], [`SystemClock`], [`AppLease`] and
//! [`AppHardware`].

use std::any::Any;
use std::num::NonZero;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use agent::media::{Company, MediaProvider, media_company, media_provider};
use async_trait::async_trait;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use models::fit::Target;
use search::FetchError;
use tauri::{AppHandle, Emitter, Manager};
use tokio_util::sync::CancellationToken;
use url::Url;

use super::errors;
use super::local::LocalProvider;
use super::prefs::GpuPolicy;
use super::spend::day_start_in;
use crate::dto::RunDto;
use crate::error::AppResult;
use crate::gpu_lease::{GpuLease, Grant, LeaseWait, Refused, StudioNeed};
use crate::hub::HardwareDto;
use crate::providers::{client_from, keychain_error, kind_of, secret_key};
use crate::runtime_manager::RuntimeManager;
use crate::state::AppState;

/// The webview event that carries a run's new state.
pub const PROGRESS_EVENT: &str = "studio-progress";

/// The longest a download from a company's file host may take, start to end.
pub const FILE_HOST_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// The media company behind an endpoint, with its key.
#[async_trait]
pub trait ProviderSource: Send + Sync {
    /// The media provider of endpoint `id` and what the run records about it. Refused with
    /// `studio_no_endpoint` when the endpoint is gone or makes no pictures or video, and with
    /// `studio_no_key` when it has no key. Reads the keychain; sends nothing.
    async fn media(&self, id: store::ProviderId) -> AppResult<(Arc<dyn MediaProvider>, Endpoint)>;

    /// The company behind endpoint `id`, without its key: `None` when the endpoint is gone or
    /// makes no pictures or video. Reads the store only (how a run that ended while the app
    /// was closed is worded).
    async fn company(&self, id: store::ProviderId) -> Option<Company>;
}

/// Where a run goes, as the run records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// The endpoint; `None` for this computer.
    pub id: Option<store::ProviderId>,
    /// Its name as shown.
    pub name: String,
    /// The host its requests go to (empty for this computer).
    pub host: String,
    /// The company behind it.
    pub company: Company,
}

/// Where the manager sends each run's new state.
pub trait StudioSink: Send + Sync {
    /// The run changed.
    fn progress(&self, run: &RunDto);
}

/// Downloads a result from a company's file host, without the key.
#[async_trait]
pub trait ForeignFetch: Send + Sync {
    /// Streams `url` to `dest`, at most `cap` bytes, and returns the media type it came as.
    /// The manager words a failure itself (`errors::from_fetch`).
    async fn fetch_to_file(&self, url: &Url, cap: u64, dest: &Path) -> Result<String, FetchError>;
}

/// The time, and where a local calendar day starts.
pub trait Clock: Send + Sync {
    /// Now, in Unix ms.
    fn now_ms(&self) -> i64;
    /// The start of the local calendar day that holds `at_ms`, in Unix ms.
    fn day_start_ms(&self, at_ms: i64) -> i64;
}

/// The graphics card for a local job (the app's is [`GpuLease`]): one tenant at a time, the
/// chat model or the job.
#[async_trait]
pub trait CardLease: Send + Sync {
    /// The card for one job, as `GpuLease::acquire_for_studio` decides it, with the hold: the
    /// job holds the card until the box is dropped. `on_wait` is told what the job waits for.
    async fn acquire(
        &self,
        need: &StudioNeed,
        policy: GpuPolicy,
        answered: Option<bool>,
        cancel: &CancellationToken,
        on_wait: &(dyn Fn(LeaseWait) + Send + Sync),
    ) -> Result<(Grant, Box<dyn Any + Send + Sync>), Refused>;

    /// "Reload it after Studio": loads the chat model at `path` again (once, when the local
    /// queue empties).
    async fn reload_chat(&self, path: &str);
}

/// This computer as a local run sees it.
#[async_trait]
pub trait LocalHardware: Send + Sync {
    /// Where models run, as probed now (with the chat model loaded, if one is), the Studio
    /// engine's backend there (`vulkan`, `metal` or `cpu`) and llama.cpp's description of the
    /// graphics device (`None` on the processor).
    async fn target(&self) -> (Target, String, Option<String>);

    /// The loaded (or loading) chat model's bytes on a graphics device; `None` when none is
    /// on one.
    fn chat_device_bytes(&self) -> Option<u64>;

    /// The processor's logical cores.
    fn logical_cores(&self) -> usize;
}

/// What runs on this computer go through: the Studio engine, the graphics card's lease and the
/// hardware.
#[derive(Clone)]
pub struct LocalLane {
    /// The installed models and one engine process per run.
    pub provider: Arc<dyn LocalProvider>,
    /// The graphics card, shared with the chat model.
    pub lease: Arc<dyn CardLease>,
    /// This computer.
    pub hardware: Arc<dyn LocalHardware>,
}

/// The app's lease: the one `GpuLease` the chat loads' gate asks too (Task L12).
pub struct AppLease(pub Arc<GpuLease>);

#[async_trait]
impl CardLease for AppLease {
    async fn acquire(
        &self,
        need: &StudioNeed,
        policy: GpuPolicy,
        answered: Option<bool>,
        cancel: &CancellationToken,
        on_wait: &(dyn Fn(LeaseWait) + Send + Sync),
    ) -> Result<(Grant, Box<dyn Any + Send + Sync>), Refused> {
        let (grant, hold) = self
            .0
            .acquire_for_studio(need, policy, answered, cancel, on_wait)
            .await?;
        Ok((grant, Box::new(hold)))
    }

    async fn reload_chat(&self, path: &str) {
        self.0.reload_chat(path).await;
    }
}

/// The app's hardware: the device the chat model's backends report (`hub::detect`), its free
/// memory with the chat model loaded now (`RuntimeManager::device_free_now`), the loaded chat
/// model's bytes on it, and the processor's cores.
pub struct AppHardware(pub AppHandle);

#[async_trait]
impl LocalHardware for AppHardware {
    async fn target(&self) -> (Target, String, Option<String>) {
        let Some(state) = self.0.try_state::<AppState>() else {
            let ram = models::hardware::system_memory().map_or(0, |m| m.available_bytes);
            return (
                Target::Cpu {
                    ram_free_bytes: ram,
                },
                "cpu".to_owned(),
                None,
            );
        };
        let (target, hardware) = crate::hub::detect(&state).await;
        // The probe behind it is cached and runs again only before a chat load: its free
        // memory is corrected for the chat model loaded, or unloaded, since.
        let total = device_total(&state.runtime, &hardware);
        let target = with_device_free(target, |probed| {
            state.runtime.device_free_now(probed, total)
        });
        // The Studio engine's own backend there: Metal on Apple silicon, Vulkan on a graphics
        // card whatever llama.cpp uses (the pinned engine has no other).
        let (backend, description) = match &target {
            Target::Gpu { .. } => ("vulkan", hardware.gpu_name),
            Target::AppleUnified { .. } => ("metal", hardware.gpu_name),
            Target::Cpu { .. } => ("cpu", None),
        };
        (target, backend.to_owned(), description)
    }

    fn chat_device_bytes(&self) -> Option<u64> {
        self.0.try_state::<AppState>()?.runtime.device_bytes()
    }

    fn logical_cores(&self) -> usize {
        std::thread::available_parallelism().map_or(1, NonZero::get)
    }
}

/// `target` with its graphics device's free memory (a card's free bytes, Apple silicon's
/// working set) changed by `free`; a processor's is left as it is.
pub(crate) fn with_device_free(target: Target, free: impl FnOnce(u64) -> u64) -> Target {
    match target {
        Target::Gpu {
            name,
            free_bytes,
            ram_free_bytes,
        } => Target::Gpu {
            name,
            free_bytes: free(free_bytes),
            ram_free_bytes,
        },
        Target::AppleUnified {
            working_set_bytes,
            ram_free_bytes,
        } => Target::AppleUnified {
            working_set_bytes: free(working_set_bytes),
            ram_free_bytes,
        },
        cpu @ Target::Cpu { .. } => cpu,
    }
}

/// The total memory of the graphics device `hub::detect` found (`hardware`), from the same
/// cached probe and picked the same way: `None` when it is not known, or while a probe runs.
fn device_total(manager: &RuntimeManager, hardware: &HardwareDto) -> Option<u64> {
    let backend = hardware.backend.as_deref()?;
    let description = hardware.gpu_name.as_deref()?;
    manager
        .cached_backends()?
        .iter()
        .filter(|(b, p)| b.kind.as_str() == backend && p.is_usable())
        .find_map(|(_, p)| runtime::pick_device(&p.devices))
        .filter(|d| d.description == description && d.total_mib > 0)
        .map(|d| d.total_mib.saturating_mul(1024 * 1024))
}

/// The endpoints in the store, with their keys from the keychain: the client is built as
/// `providers::client_for` builds it.
pub struct StoreProviders {
    /// Where the endpoints are kept.
    pub store: store::Store,
    /// Where their keys are kept.
    pub secrets: Arc<dyn store::secrets::SecretStore>,
}

#[async_trait]
impl ProviderSource for StoreProviders {
    async fn media(&self, id: store::ProviderId) -> AppResult<(Arc<dyn MediaProvider>, Endpoint)> {
        let provider = self
            .store
            .get_provider(id)
            .await?
            .ok_or_else(errors::no_endpoint)?;
        let company = media_company(kind_of(&provider.kind)).ok_or_else(errors::no_endpoint)?;
        let key = self
            .secrets
            .get(&secret_key(id))
            .map_err(keychain_error)?
            .ok_or_else(|| errors::no_key(&provider.name))?;
        let media =
            media_provider(client_from(&provider, Some(key))?).ok_or_else(errors::no_endpoint)?;
        let endpoint = Endpoint {
            id: Some(id),
            name: provider.name.clone(),
            host: agent::endpoint_host(&provider.base_url).unwrap_or_else(|| media.host()),
            company,
        };
        Ok((media, endpoint))
    }

    async fn company(&self, id: store::ProviderId) -> Option<Company> {
        let provider = self.store.get_provider(id).await.ok()??;
        media_company(kind_of(&provider.kind))
    }
}

/// The main window: `studio-progress` with the run.
pub struct AppSink(pub AppHandle);

impl StudioSink for AppSink {
    fn progress(&self, run: &RunDto) {
        if let Err(e) = self.0.emit_to(crate::MAIN_WINDOW, PROGRESS_EVENT, run) {
            tracing::debug!("a Studio progress event was not sent: {e}");
        }
    }
}

/// The guarded fetcher (addresses checked and pinned, private and loopback addresses
/// refused, redirects followed by hand), which never sends a key. Each download may take
/// [`FILE_HOST_TIMEOUT`], whatever the fetcher's own limit; connecting still takes at most
/// 10 s.
pub struct GuardedFetch(pub search::Fetcher);

#[async_trait]
impl ForeignFetch for GuardedFetch {
    async fn fetch_to_file(&self, url: &Url, cap: u64, dest: &Path) -> Result<String, FetchError> {
        let file = self
            .0
            .clone()
            .with_timeout(FILE_HOST_TIMEOUT)
            .fetch_to_file(url, cap, dest)
            .await?;
        Ok(file.content_type)
    }
}

/// This computer's clock and time zone.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        Timestamp::now().as_millisecond()
    }

    fn day_start_ms(&self, at_ms: i64) -> i64 {
        day_start_in(&TimeZone::system(), at_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn endpoint(store: &store::Store, kind: &str, base_url: &str) -> store::ProviderId {
        store
            .upsert_provider(store::ProviderInput {
                id: None,
                name: "My endpoint".into(),
                kind: kind.into(),
                base_url: base_url.into(),
                secret_ref: None,
                enabled: true,
            })
            .await
            .unwrap()
    }

    fn source(store: &store::Store, secrets: store::MemorySecretStore) -> StoreProviders {
        StoreProviders {
            store: store.clone(),
            secrets: Arc::new(secrets),
        }
    }

    #[tokio::test]
    async fn an_endpoint_with_its_key_gives_its_company() {
        let store = store::Store::open_in_memory().unwrap();
        let id = endpoint(&store, "openrouter", "https://openrouter.ai/api/v1").await;
        let secrets = store::MemorySecretStore::new();
        store::SecretStore::set(&secrets, &secret_key(id), "sk-or-test").unwrap();
        let (media, at) = source(&store, secrets).media(id).await.unwrap();
        assert_eq!(media.company(), Company::OpenRouter);
        assert_eq!(
            at,
            Endpoint {
                id: Some(id),
                name: "My endpoint".into(),
                host: "openrouter.ai".into(),
                company: Company::OpenRouter,
            }
        );
    }

    #[tokio::test]
    async fn no_key_no_endpoint_and_no_pictures_are_refused() {
        let store = store::Store::open_in_memory().unwrap();
        let openai = endpoint(&store, "openai", "https://api.openai.com/v1").await;
        let err = source(&store, store::MemorySecretStore::new())
            .media(openai)
            .await
            .err()
            .unwrap();
        assert_eq!(err.code, "studio_no_key");
        assert_eq!(err.message, "My endpoint has no API key.");

        let gone = store::ProviderId(9_999);
        let err = source(&store, store::MemorySecretStore::new())
            .media(gone)
            .await
            .err()
            .unwrap();
        assert_eq!(err.code, "studio_no_endpoint");

        let ollama = endpoint(&store, "ollama", "http://127.0.0.1:11434/v1").await;
        let err = source(&store, store::MemorySecretStore::new())
            .media(ollama)
            .await
            .err()
            .unwrap();
        assert_eq!(err.code, "studio_no_endpoint");
    }

    #[tokio::test]
    async fn an_endpoint_gives_its_company_without_its_key() {
        let store = store::Store::open_in_memory().unwrap();
        let openai = endpoint(&store, "openai", "https://api.openai.com/v1").await;
        let ollama = endpoint(&store, "ollama", "http://127.0.0.1:11434/v1").await;
        let source = source(&store, store::MemorySecretStore::new());
        assert_eq!(source.company(openai).await, Some(Company::OpenAi));
        assert_eq!(source.company(ollama).await, None);
        assert_eq!(source.company(store::ProviderId(9_999)).await, None);
    }

    #[tokio::test]
    async fn a_result_link_to_this_computer_is_refused_before_connecting() {
        let fetch = GuardedFetch(search::Fetcher::new("test"));
        let dir = std::env::temp_dir().join(format!("studio-fetch-{}", uuid::Uuid::new_v4()));
        let url = Url::parse("http://127.0.0.1:9/result.mp4").unwrap();
        let err = fetch
            .fetch_to_file(&url, 1024, &dir.join("out"))
            .await
            .unwrap_err();
        assert!(matches!(err, search::FetchError::Blocked { .. }), "{err:?}");
        assert!(!dir.exists());
    }

    /// L15 review fix: the probed free memory is corrected on the graphics device only.
    #[test]
    fn the_free_memory_is_corrected_on_the_graphics_device_only() {
        const GB: u64 = 1_000_000_000;
        let less = |free: u64| free - 5 * GB;
        let card = Target::Gpu {
            name: "Card".into(),
            free_bytes: 8 * GB,
            ram_free_bytes: Some(32 * GB),
        };
        assert_eq!(
            with_device_free(card, less),
            Target::Gpu {
                name: "Card".into(),
                free_bytes: 3 * GB,
                ram_free_bytes: Some(32 * GB),
            }
        );
        let mac = Target::AppleUnified {
            working_set_bytes: 16 * GB,
            ram_free_bytes: None,
        };
        assert_eq!(
            with_device_free(mac, less),
            Target::AppleUnified {
                working_set_bytes: 11 * GB,
                ram_free_bytes: None,
            }
        );
        let cpu = Target::Cpu {
            ram_free_bytes: 16 * GB,
        };
        assert_eq!(with_device_free(cpu.clone(), less), cpu);
    }

    #[test]
    fn the_system_day_starts_at_local_midnight() {
        let noon = jiff::civil::date(2026, 11, 17)
            .at(12, 0, 0, 0)
            .to_zoned(TimeZone::system())
            .unwrap();
        let midnight = jiff::civil::date(2026, 11, 17)
            .at(0, 0, 0, 0)
            .to_zoned(TimeZone::system())
            .unwrap();
        assert_eq!(
            SystemClock.day_start_ms(noon.timestamp().as_millisecond()),
            midnight.timestamp().as_millisecond()
        );
        assert!(SystemClock.now_ms() > noon.timestamp().as_millisecond() - 86_400_000 * 365);
    }
}
