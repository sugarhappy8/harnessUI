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
use super::manager::device_name;
use super::prefs::GpuPolicy;
use super::spend::day_start_in;
use crate::dto::RunDto;
use crate::error::AppResult;
use crate::gpu_lease::{GpuLease, Grant, LeaseWait, Refused, StudioNeed};
use crate::hub::HardwareDto;
use crate::providers::{client_from, keychain_error, kind_of, secret_key};
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
/// memory with the chat model loaded now when it is on that card
/// (`RuntimeManager::device_free_now`, [`studio_target`]), the loaded chat model's bytes on a
/// device, and the processor's cores.
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
        // The probe `hub::detect` read, for the device's name and total: cached, so nothing
        // is probed here, and a probe running now is waited for rather than leaving the free
        // memory without its cap (ruling L15-total-await).
        let probes = state.runtime.backends(false).await;
        studio_target(target, hardware, &probes, |probed, total, on_device| {
            state.runtime.device_free_now(probed, total, on_device)
        })
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

/// What a local run sees of `target`, the graphics device `hub::detect` found in `probes`
/// (`hardware`): its free memory as `free_now` corrects it for the chat model loaded, or
/// unloaded, since the probe (`RuntimeManager::device_free_now` in the app: the probe is
/// cached and runs again only before a chat load), given the device's total and which
/// (backend, device name) is that card ([`same_card`]; nothing is corrected when the device
/// is not found in `probes`); the Studio engine's own backend there (Metal on Apple silicon,
/// Vulkan on a graphics card whatever llama.cpp uses: the pinned engine has no other) and
/// llama.cpp's description of the device.
pub(crate) fn studio_target(
    target: Target,
    hardware: HardwareDto,
    probes: &[(runtime::Backend, runtime::ProbeResult)],
    free_now: impl FnOnce(u64, Option<u64>, &dyn Fn(&str, &str) -> bool) -> u64,
) -> (Target, String, Option<String>) {
    let picked = picked_device(probes, &hardware);
    let total = picked
        .filter(|(_, d)| d.total_mib > 0)
        .map(|(_, d)| d.total_mib.saturating_mul(1024 * 1024));
    let on_card = |backend: &str, name: &str| {
        picked.is_some_and(|(kind, device)| same_card(probes, kind, device, backend, name))
    };
    let target = with_device_free(target, |probed| free_now(probed, total, &on_card));
    let (backend, description) = match &target {
        Target::Gpu { .. } => ("vulkan", hardware.gpu_name),
        Target::AppleUnified { .. } => ("metal", hardware.gpu_name),
        Target::Cpu { .. } => ("cpu", None),
    };
    (target, backend.to_owned(), description)
}

/// The graphics device `hub::detect` found (`hardware`), in `probes`: of the backend it
/// reported, picked as it picks one (`runtime::pick_device`), and matched by description.
fn picked_device<'a>(
    probes: &'a [(runtime::Backend, runtime::ProbeResult)],
    hardware: &HardwareDto,
) -> Option<(runtime::BackendKind, &'a runtime::Device)> {
    let backend = hardware.backend.as_deref()?;
    let description = hardware.gpu_name.as_deref()?;
    probes
        .iter()
        .filter(|(b, p)| b.kind.as_str() == backend && p.is_usable())
        .find_map(|(b, p)| runtime::pick_device(&p.devices).map(|d| (b.kind, d)))
        .filter(|(_, d)| d.description == description)
}

/// Whether a model on `backend`'s device `name` is on the card `picked` (a device of
/// `picked_kind`): the same backend and device name, as a load counts the memory it frees
/// (`Machine::from_probes`); or, on another backend (llama.cpp's ROCm or CUDA beside the
/// Vulkan device Studio judges), that device in `probes` names the same card: the same name
/// without the driver's code for the chip ([`device_name`]), which no other device of
/// `picked_kind` has (two cards of one model cannot be told apart that way).
fn same_card(
    probes: &[(runtime::Backend, runtime::ProbeResult)],
    picked_kind: runtime::BackendKind,
    picked: &runtime::Device,
    backend: &str,
    name: &str,
) -> bool {
    if backend == picked_kind.as_str() {
        return name == picked.name;
    }
    fn devices<'a>(
        probes: &'a [(runtime::Backend, runtime::ProbeResult)],
        kind: &'a str,
    ) -> impl Iterator<Item = &'a runtime::Device> {
        probes
            .iter()
            .filter(move |(b, p)| b.kind.as_str() == kind && p.is_usable())
            .flat_map(|(_, p)| p.devices.iter())
    }
    let card = device_name(&picked.description);
    let Some(chat) = devices(probes, backend).find(|d| d.name == name) else {
        return false;
    };
    !card.is_empty()
        && device_name(&chat.description) == card
        && devices(probes, picked_kind.as_str())
            .filter(|d| device_name(&d.description) == card)
            .count()
            == 1
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

    const MIB: u64 = 1024 * 1024;

    fn probed(
        kind: runtime::BackendKind,
        devices: &[(&str, &str, u64, u64)],
    ) -> (runtime::Backend, runtime::ProbeResult) {
        (
            runtime::Backend {
                kind,
                dir: "/b".into(),
                server: "/b/llama-server".into(),
                fit_params: None,
                build: "b1".into(),
                source: runtime::BackendSource::Installed,
            },
            runtime::ProbeResult {
                backend_kind: kind,
                devices: devices
                    .iter()
                    .map(|(name, description, total_mib, free_mib)| runtime::Device {
                        name: (*name).into(),
                        description: (*description).into(),
                        total_mib: *total_mib,
                        free_mib: *free_mib,
                    })
                    .collect(),
                error: None,
                output: Vec::new(),
            },
        )
    }

    /// What `hub::detect` reports for the Vulkan device `description` with `free` bytes free.
    fn detected(description: &str, free: u64) -> (Target, HardwareDto) {
        (
            Target::Gpu {
                name: description.into(),
                free_bytes: free,
                ram_free_bytes: Some(32_000 * MIB),
            },
            HardwareDto {
                target: "gpu".into(),
                gpu_name: Some(description.into()),
                gpu_free_bytes: Some(free),
                ram_total_bytes: None,
                ram_available_bytes: None,
                usable_bytes: free,
                backend: Some("vulkan".into()),
                intel_mac: false,
            },
        )
    }

    /// The card's free memory as a local run sees it, with the chat model (5,000 MiB) on
    /// `at_probe` when the backends were probed and on `now` now, each a (backend, device).
    fn seen_free(
        probes: &[(runtime::Backend, runtime::ProbeResult)],
        detected: (Target, HardwareDto),
        at_probe: Option<(&str, &str)>,
        now: Option<(&str, &str)>,
    ) -> u64 {
        let chat = |on: Option<(&str, &str)>| {
            on.map(|(backend, device)| (backend.to_owned(), device.to_owned(), 5_000 * MIB))
        };
        let (at_probe, now) = (chat(at_probe), chat(now));
        let (target, _, _) =
            studio_target(detected.0, detected.1, probes, |probed, total, on_card| {
                crate::runtime_manager::free_on_device(
                    probed,
                    total,
                    at_probe.as_ref(),
                    now.as_ref(),
                    on_card,
                )
            });
        match target {
            Target::Gpu { free_bytes, .. } => free_bytes,
            other => panic!("not a card: {other:?}"),
        }
    }

    /// Ruling L15-tests (AppHardware) and L15-chat-device: the card `hub::detect` found has
    /// its free memory corrected for the chat model only when the chat model is on that card
    /// (by the same backend and device name, or by ROCm's or CUDA's name for the same card),
    /// and never past the card's total.
    #[test]
    fn a_local_run_counts_the_chat_model_only_on_the_card_it_judges() {
        const A: &str = "AMD Radeon RX 7600 (RADV NAVI33)";
        let probes = [
            probed(
                runtime::BackendKind::Vulkan,
                &[
                    ("Vulkan0", A, 8_192, 8_000),
                    ("Vulkan1", "AMD Radeon RX 6600 (RADV NAVI23)", 4_096, 4_000),
                ],
            ),
            probed(
                runtime::BackendKind::Rocm,
                &[
                    ("ROCm0", "AMD Radeon RX 7600", 8_192, 8_000),
                    ("ROCm1", "AMD Radeon RX 6600", 4_096, 4_000),
                ],
            ),
        ];
        let card_a = || detected(A, 8_000 * MIB);
        // The chat model on card B, by either backend's name: card A is as probed.
        assert_eq!(
            seen_free(&probes, card_a(), None, Some(("vulkan", "Vulkan1"))),
            8_000 * MIB
        );
        assert_eq!(
            seen_free(&probes, card_a(), None, Some(("rocm", "ROCm1"))),
            8_000 * MIB
        );
        // On card A: by Vulkan's name, and by ROCm's name for the same card.
        assert_eq!(
            seen_free(&probes, card_a(), None, Some(("vulkan", "Vulkan0"))),
            3_000 * MIB
        );
        assert_eq!(
            seen_free(&probes, card_a(), None, Some(("rocm", "ROCm0"))),
            3_000 * MIB
        );
        // On card A at the probe and unloaded since: counted back, at most up to the card's
        // total (8,192 MiB).
        assert_eq!(
            seen_free(
                &probes,
                detected(A, 3_000 * MIB),
                Some(("vulkan", "Vulkan0")),
                None
            ),
            8_000 * MIB
        );
        assert_eq!(
            seen_free(
                &probes,
                detected(A, 7_000 * MIB),
                Some(("rocm", "ROCm0")),
                None
            ),
            8_192 * MIB
        );
        // A device the probes no longer have: nothing is corrected.
        assert_eq!(
            seen_free(
                &probes,
                detected("Another card", 8_000 * MIB),
                None,
                Some(("vulkan", "Vulkan0"))
            ),
            8_000 * MIB
        );

        // Two cards of one model: ROCm's name cannot say which one; Vulkan's can.
        let twins = [
            probed(
                runtime::BackendKind::Vulkan,
                &[("Vulkan0", A, 8_192, 8_000), ("Vulkan1", A, 8_192, 8_000)],
            ),
            probed(
                runtime::BackendKind::Rocm,
                &[
                    ("ROCm0", "AMD Radeon RX 7600", 8_192, 8_000),
                    ("ROCm1", "AMD Radeon RX 7600", 8_192, 8_000),
                ],
            ),
        ];
        assert_eq!(
            seen_free(&twins, card_a(), None, Some(("rocm", "ROCm1"))),
            8_000 * MIB
        );
        assert_eq!(
            seen_free(&twins, card_a(), None, Some(("vulkan", "Vulkan1"))),
            8_000 * MIB
        );
        assert_eq!(
            seen_free(&twins, card_a(), None, Some(("vulkan", "Vulkan0"))),
            3_000 * MIB
        );
    }

    /// The Studio engine's backend and the device's description go with the target.
    #[test]
    fn a_local_run_names_the_engines_backend_and_the_device() {
        const A: &str = "AMD Radeon RX 7600 (RADV NAVI33)";
        let probes = [probed(
            runtime::BackendKind::Vulkan,
            &[("Vulkan0", A, 8_192, 8_000)],
        )];
        let (target, hardware) = detected(A, 8_000 * MIB);
        let (_, backend, description) =
            studio_target(target, hardware, &probes, |probed, _, _| probed);
        assert_eq!(
            (backend.as_str(), description.as_deref()),
            ("vulkan", Some(A))
        );
        let (_, hardware) = detected(A, 0);
        let cpu = Target::Cpu {
            ram_free_bytes: 16_000 * MIB,
        };
        let (target, backend, description) =
            studio_target(cpu.clone(), hardware, &probes, |_, _, _| 0);
        assert_eq!((target, backend.as_str(), description), (cpu, "cpu", None));
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
