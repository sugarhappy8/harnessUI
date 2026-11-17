//! Each endpoint's saved picture and video model lists (0.1.0 Studio).
//!
//! - **Saved as they came.** `providerMediaModels:<id>` ([`cache_key`]) keeps the company's
//!   answers ([`CachedCatalog`]): its picture and video lists and, for an OpenRouter picture
//!   model whose price was asked for, its `/endpoints` answer. The models and their controls
//!   are built each time the lists are read ([`parse_catalog`]), so a fix to that reader
//!   applies to lists saved earlier.
//! - **Fetched only on request.** [`refresh`] (Test, *Get the list*, *Update model lists*)
//!   and [`refresh_price`] (picking an OpenRouter picture model, *Get the price*) are the
//!   only calls that ask the company; a failure keeps what was saved. [`models_for`] and
//!   [`model`] read the saved lists only, so opening Studio sends nothing.
//! - **Which endpoints.** Only OpenRouter and OpenAI endpoints make pictures or video
//!   ([`media_company`]); Anthropic, abliteration.ai and servers people run themselves get
//!   nothing.
//! - **Deleted endpoints.** [`forget`] removes an endpoint's lists, and lists that arrive
//!   after the endpoint was deleted are not saved.

use std::collections::{BTreeMap, BTreeSet};

use agent::media::{
    Company, LocalIo, MediaError, MediaKind, MediaModel, MediaProvider, RawCatalog, media_company,
    parse_catalog,
};
use agent::{EndpointPlace, ProviderError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::providers::kind_of;

/// Saves and removals of the lists, one at a time: a price fetched while the lists are
/// updated (or the other way round) is not lost, and nothing is saved for an endpoint after
/// it was deleted.
static WRITES: Mutex<()> = Mutex::const_new(());

/// The settings key of an endpoint's saved picture and video lists.
pub fn cache_key(id: store::ProviderId) -> String {
    format!("providerMediaModels:{}", id.0)
}

/// An endpoint's picture and video lists as the company gave them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedCatalog {
    /// When the lists were fetched, in Unix ms.
    pub fetched_at: i64,
    /// The lists.
    pub raw: RawCatalog,
    /// The `/endpoints` answer of each OpenRouter picture model whose price was asked for,
    /// by model id.
    pub image_prices: BTreeMap<String, CachedPrice>,
}

/// One picture model's price answer as the company gave it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedPrice {
    /// When it was fetched, in Unix ms.
    pub fetched_at: i64,
    /// The answer.
    pub raw: Value,
}

/// An endpoint that makes pictures or video, with the models of its saved lists.
#[derive(Debug, Clone, PartialEq)]
pub struct EndpointModels {
    /// The endpoint.
    pub provider: store::Provider,
    /// The company behind it.
    pub company: Company,
    /// The host its requests go to, for example `openrouter.ai`.
    pub host: String,
    /// Whether the endpoint is on the internet.
    pub cloud: bool,
    /// Whether it may have a key (its models are greyed out without one).
    pub has_key: bool,
    /// When its lists were fetched; `None` when none are saved, so Studio offers to get them.
    pub fetched_at: Option<i64>,
    /// The models of its saved lists: pictures, then videos.
    pub models: Vec<PricedModel>,
}

/// A model of an endpoint's saved lists, and when its price was fetched.
#[derive(Debug, Clone, PartialEq)]
pub struct PricedModel {
    /// The model, priced from the saved answers.
    pub model: MediaModel,
    /// When its price was fetched. An OpenRouter picture is priced by its own `/endpoints`
    /// answer: `None` until that is asked for ("price shown when picked"). Any other model
    /// is priced by its list: the list's time.
    pub price_fetched_at: Option<i64>,
    /// The saved `/endpoints` answer of an OpenRouter picture model, when there is one.
    pub endpoints: Option<Value>,
}

/// Fetches an endpoint's picture and video lists and saves them, keeping the saved prices
/// of the pictures the new lists still have. A failure saves nothing, so the lists saved
/// before stay.
pub async fn refresh(
    store: &store::Store,
    id: store::ProviderId,
    media: &dyn MediaProvider,
    now: i64,
) -> Result<(), MediaError> {
    let raw = media.fetch_catalog().await?;
    let _writes = WRITES.lock().await;
    if store.get_provider(id).await.map_err(not_saved)?.is_none() {
        return Ok(());
    }
    let listed = picture_ids(media.company(), &raw);
    let mut image_prices = read(store, id)
        .await
        .map(|saved| saved.image_prices)
        .unwrap_or_default();
    image_prices.retain(|model, _| listed.contains(model));
    let saved = CachedCatalog {
        fetched_at: now,
        raw,
        image_prices,
    };
    store
        .set_setting(&cache_key(id), &saved)
        .await
        .map_err(not_saved)
}

/// Fetches one picture model's price answer and saves it with the endpoint's lists. Only a
/// picture model of the saved lists is asked for: any other name would go into the address
/// unchecked, and its price could not be kept. An answer without a price (OpenAI's prices
/// come with its list) is not saved.
pub async fn refresh_price(
    store: &store::Store,
    id: store::ProviderId,
    media: &dyn MediaProvider,
    model: &str,
    now: i64,
) -> Result<(), MediaError> {
    let listed = read(store, id)
        .await
        .is_some_and(|saved| picture_ids(media.company(), &saved.raw).contains(model));
    if !listed {
        return Err(MediaError::NotSent(ProviderError::InvalidRequest(
            "the model is not a picture model in the endpoint's saved list".into(),
        )));
    }
    let raw = media.fetch_image_price(model).await?;
    if raw.is_null() {
        return Ok(());
    }
    let _writes = WRITES.lock().await;
    // Lists removed meanwhile (the endpoint was deleted) are not brought back.
    let Some(mut saved) = read(store, id).await else {
        return Ok(());
    };
    saved.image_prices.insert(
        model.to_owned(),
        CachedPrice {
            fetched_at: now,
            raw,
        },
    );
    store
        .set_setting(&cache_key(id), &saved)
        .await
        .map_err(not_saved)
}

/// Removes an endpoint's saved lists. A failure is only logged.
pub async fn forget(store: &store::Store, id: store::ProviderId) {
    let _writes = WRITES.lock().await;
    if let Err(err) = store.delete_setting(&cache_key(id)).await {
        tracing::warn!(
            "the picture and video lists of endpoint {} could not be removed: {err}",
            id.0
        );
    }
}

/// The endpoints among `providers` that make pictures or video, in their order, each with
/// the models of its saved lists. `has_key` says whether an endpoint may have a key.
/// From the saved lists only: never builds a client or touches the network.
pub async fn models_for(
    store: &store::Store,
    providers: &[store::Provider],
    has_key: impl Fn(store::ProviderId) -> bool,
) -> Vec<EndpointModels> {
    let mut out = Vec::new();
    for provider in providers {
        let Some(company) = media_company(kind_of(&provider.kind)) else {
            continue;
        };
        let saved = read(store, provider.id).await;
        out.push(EndpointModels {
            provider: provider.clone(),
            company,
            host: agent::endpoint_host(&provider.base_url)
                .unwrap_or_else(|| provider.base_url.trim().to_owned()),
            cloud: agent::endpoint_place(&provider.base_url) == EndpointPlace::Internet,
            has_key: has_key(provider.id),
            fetched_at: saved.as_ref().map(|saved| saved.fetched_at),
            models: saved
                .map(|saved| priced_models(company, &saved))
                .unwrap_or_default(),
        });
    }
    out
}

/// One model of an endpoint's saved lists, with the endpoint: `None` when the endpoint is
/// gone, makes no pictures or video, or its saved lists do not have the model. From the
/// saved lists only, like [`models_for`]. The keychain is not read here, so `has_key` is
/// `true`: a missing key is found by the request itself.
pub async fn model(
    store: &store::Store,
    id: store::ProviderId,
    model: &str,
) -> Option<(EndpointModels, PricedModel)> {
    let provider = store.get_provider(id).await.ok().flatten()?;
    let endpoint = models_for(store, std::slice::from_ref(&provider), |_| true)
        .await
        .into_iter()
        .next()?;
    let found = endpoint
        .models
        .iter()
        .find(|m| m.model.id == model)?
        .clone();
    Some((endpoint, found))
}

/// An endpoint's saved lists: `None` when none are saved, or when they were saved in a form
/// this version cannot read (Studio then offers to get them again).
async fn read(store: &store::Store, id: store::ProviderId) -> Option<CachedCatalog> {
    match store.get_setting(&cache_key(id)).await {
        Ok(saved) => saved,
        Err(err) => {
            tracing::warn!(
                "the saved picture and video lists of endpoint {} could not be read: {err}",
                id.0
            );
            None
        }
    }
}

/// The models of saved lists, each priced from the saved answers and with when its price
/// was fetched.
fn priced_models(company: Company, saved: &CachedCatalog) -> Vec<PricedModel> {
    let answers: BTreeMap<String, Value> = saved
        .image_prices
        .iter()
        .map(|(model, price)| (model.clone(), price.raw.clone()))
        .collect();
    parse_catalog(company, &saved.raw, &answers)
        .into_iter()
        .map(|model| {
            let (price_fetched_at, endpoints) =
                if company == Company::OpenRouter && model.kind == MediaKind::Image {
                    let price = saved.image_prices.get(&model.id);
                    (price.map(|p| p.fetched_at), price.map(|p| p.raw.clone()))
                } else {
                    (Some(saved.fetched_at), None)
                };
            PricedModel {
                model,
                price_fetched_at,
                endpoints,
            }
        })
        .collect()
}

/// The ids of the picture models in a company's lists.
fn picture_ids(company: Company, raw: &RawCatalog) -> BTreeSet<String> {
    parse_catalog(company, raw, &BTreeMap::new())
        .into_iter()
        .filter(|model| model.kind == MediaKind::Image)
        .map(|model| model.id)
        .collect()
}

/// A failure keeping the lists on this computer.
fn not_saved(err: store::StoreError) -> MediaError {
    MediaError::Local(LocalIo::Io(err.to_string()))
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use agent::ProviderError;
    use agent::media::{
        Fetched, JobDone, JobState, MediaKind, MediaRequest, Price, RequestPreview, Started,
    };
    use async_trait::async_trait;

    use super::*;

    const IMAGES: &str = include_str!(
        "../../../crates/agent/tests/fixtures/media/openrouter-images-2026-11-17.json"
    );
    const VIDEOS: &str = include_str!(
        "../../../crates/agent/tests/fixtures/media/openrouter-videos-2026-11-17.json"
    );
    const ENDPOINTS: &str = include_str!(
        "../../../crates/agent/tests/fixtures/media/openrouter-image-endpoints-2026-11-17.json"
    );
    const OPENAI_MODELS: &str =
        include_str!("../../../crates/agent/tests/fixtures/media/openai-models.json");
    const FLUX: &str = "black-forest-labs/flux-3-image";
    const NANO_BANANA: &str = "google/gemini-nano-banana-2.1";
    const VEO: &str = "google/veo-3.1";
    const OPENAI_FLARE: &str = "gpt-image-2.6-flare";

    /// OpenRouter's two recorded lists, as `fetch_catalog` gives them.
    fn recorded_lists() -> RawCatalog {
        RawCatalog {
            images: serde_json::from_str(IMAGES).unwrap(),
            videos: Some(serde_json::from_str(VIDEOS).unwrap()),
        }
    }

    /// OpenRouter's recorded `/endpoints` answer for one picture model.
    fn recorded_answer(model: &str) -> Value {
        let saved: Value = serde_json::from_str(ENDPOINTS).unwrap();
        saved["answers"][model].clone()
    }

    async fn endpoint(store: &store::Store, kind: &str, base_url: &str) -> store::Provider {
        let id = store
            .upsert_provider(store::ProviderInput {
                id: None,
                name: format!("My {kind}"),
                kind: kind.into(),
                base_url: base_url.into(),
                secret_ref: None,
                enabled: true,
            })
            .await
            .unwrap();
        store.get_provider(id).await.unwrap().unwrap()
    }

    async fn openrouter(store: &store::Store) -> store::Provider {
        endpoint(store, "openrouter", "https://openrouter.ai/api/v1").await
    }

    async fn save(store: &store::Store, id: store::ProviderId, saved: &CachedCatalog) {
        store.set_setting(&cache_key(id), saved).await.unwrap();
    }

    async fn saved(store: &store::Store, id: store::ProviderId) -> Option<CachedCatalog> {
        store.get_setting(&cache_key(id)).await.unwrap()
    }

    fn priced<'a>(endpoint: &'a EndpointModels, id: &str) -> &'a PricedModel {
        endpoint
            .models
            .iter()
            .find(|m| m.model.id == id)
            .unwrap_or_else(|| panic!("no model {id}"))
    }

    /// A media company that answers from memory and counts what it was asked.
    struct Fake {
        company: Company,
        catalog: Result<RawCatalog, MediaError>,
        price: Value,
        requests: AtomicUsize,
    }

    impl Fake {
        fn openrouter() -> Self {
            Self {
                company: Company::OpenRouter,
                catalog: Ok(recorded_lists()),
                price: recorded_answer(FLUX),
                requests: AtomicUsize::new(0),
            }
        }

        fn failing() -> Self {
            Self {
                catalog: Err(MediaError::Unknown(ProviderError::Network(
                    "timed out".into(),
                ))),
                ..Self::openrouter()
            }
        }

        fn requests(&self) -> usize {
            self.requests.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl MediaProvider for Fake {
        fn company(&self) -> Company {
            self.company
        }

        fn host(&self) -> String {
            "openrouter.ai".into()
        }

        async fn fetch_catalog(&self) -> Result<RawCatalog, MediaError> {
            self.requests.fetch_add(1, Ordering::SeqCst);
            self.catalog.clone()
        }

        async fn fetch_image_price(&self, _model: &str) -> Result<Value, MediaError> {
            self.requests.fetch_add(1, Ordering::SeqCst);
            Ok(self.price.clone())
        }

        fn preview(&self, _req: &MediaRequest) -> RequestPreview {
            unreachable!("the lists never preview a request")
        }

        async fn start(&self, _req: &MediaRequest, _scratch: &Path) -> Result<Started, MediaError> {
            unreachable!("the lists never start a run")
        }

        async fn check(&self, _remote_id: &str) -> Result<JobState, MediaError> {
            unreachable!("the lists never check a job")
        }

        async fn download(
            &self,
            _remote_id: &str,
            _job: &JobDone,
            _index: usize,
            _dest: &Path,
        ) -> Result<Fetched, MediaError> {
            unreachable!("the lists never download a result")
        }
    }

    #[tokio::test]
    async fn models_come_from_the_saved_lists_only() {
        let store = store::Store::open_in_memory().unwrap();
        let provider = openrouter(&store).await;
        let lists = CachedCatalog {
            fetched_at: 500,
            raw: recorded_lists(),
            image_prices: BTreeMap::new(),
        };
        save(&store, provider.id, &lists).await;

        let listed = models_for(&store, std::slice::from_ref(&provider), |_| true).await;
        assert_eq!(listed.len(), 1);
        let endpoint = &listed[0];
        // 68 pictures and 33 videos.
        assert_eq!(endpoint.models.len(), 101);
        assert_eq!(endpoint.fetched_at, Some(500));
        assert_eq!(endpoint.provider, provider);
        assert_eq!(endpoint.company, Company::OpenRouter);
        assert_eq!(endpoint.host, "openrouter.ai");
        assert!(endpoint.cloud);
        assert!(endpoint.has_key);
        // Without a saved `/endpoints` answer a picture's price is not known yet.
        let flux = priced(endpoint, FLUX);
        assert_eq!(flux.model.price, Price::Unknown);
        assert_eq!(flux.endpoints, None);
        // Whether the endpoint has a key is the caller's answer.
        let without_key = models_for(&store, &[provider], |_| false).await;
        assert!(!without_key[0].has_key);
    }

    #[tokio::test]
    async fn an_endpoint_without_a_list_offers_to_get_it() {
        let store = store::Store::open_in_memory().unwrap();
        let provider = openrouter(&store).await;
        let listed = models_for(&store, &[provider], |_| true).await;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].fetched_at, None);
        assert!(listed[0].models.is_empty());
        // A list saved in a form this version cannot read counts as no list.
        let store = store::Store::open_in_memory().unwrap();
        let provider = openrouter(&store).await;
        store
            .set_setting(&cache_key(provider.id), &serde_json::json!({"old": true}))
            .await
            .unwrap();
        let listed = models_for(&store, &[provider], |_| true).await;
        assert_eq!(listed[0].fetched_at, None);
        assert!(listed[0].models.is_empty());
    }

    #[tokio::test]
    async fn only_openrouter_and_openai_endpoints_are_listed() {
        let store = store::Store::open_in_memory().unwrap();
        let providers = vec![
            endpoint(&store, "anthropic", "https://api.anthropic.com").await,
            endpoint(&store, "generic", "http://127.0.0.1:8080/v1").await,
            endpoint(&store, "ollama", "http://127.0.0.1:11434").await,
            openrouter(&store).await,
            endpoint(&store, "openai", "https://api.openai.com/v1").await,
            endpoint(&store, "abliteration", "https://api.abliteration.ai/v1").await,
        ];
        let listed = models_for(&store, &providers, |_| true).await;
        let companies: Vec<(Company, &str)> = listed
            .iter()
            .map(|e| (e.company, e.host.as_str()))
            .collect();
        assert_eq!(
            companies,
            [
                (Company::OpenRouter, "openrouter.ai"),
                (Company::OpenAi, "api.openai.com")
            ]
        );
    }

    #[tokio::test]
    async fn a_saved_picture_price_is_used() {
        let store = store::Store::open_in_memory().unwrap();
        let provider = openrouter(&store).await;
        let answer = recorded_answer(FLUX);
        let lists = CachedCatalog {
            fetched_at: 500,
            raw: recorded_lists(),
            image_prices: BTreeMap::from([(
                FLUX.to_owned(),
                CachedPrice {
                    fetched_at: 700,
                    raw: answer.clone(),
                },
            )]),
        };
        save(&store, provider.id, &lists).await;

        let (endpoint, flux) = model(&store, provider.id, FLUX).await.unwrap();
        assert!(
            matches!(flux.model.price, Price::PerImage(_)),
            "{:?}",
            flux.model.price
        );
        assert!(!flux.model.companies.is_empty());
        assert_eq!(flux.price_fetched_at, Some(700));
        assert_eq!(flux.endpoints, Some(answer));
        assert_eq!(endpoint.provider, provider);
        assert_eq!(endpoint.fetched_at, Some(500));
        // No such model, or no such endpoint.
        assert!(model(&store, provider.id, "nobody/nothing").await.is_none());
        assert!(model(&store, store::ProviderId(999), FLUX).await.is_none());
    }

    #[tokio::test]
    async fn a_failed_refresh_keeps_the_old_list() {
        let store = store::Store::open_in_memory().unwrap();
        let provider = openrouter(&store).await;
        let lists = CachedCatalog {
            fetched_at: 500,
            raw: recorded_lists(),
            image_prices: BTreeMap::new(),
        };
        save(&store, provider.id, &lists).await;

        let fake = Fake::failing();
        let failed = refresh(&store, provider.id, &fake, 900).await;
        assert!(matches!(failed, Err(MediaError::Unknown(_))), "{failed:?}");
        assert_eq!(saved(&store, provider.id).await, Some(lists));
    }

    #[tokio::test]
    async fn forget_removes_the_list() {
        let store = store::Store::open_in_memory().unwrap();
        let provider = openrouter(&store).await;
        let fake = Fake::openrouter();
        refresh(&store, provider.id, &fake, 900).await.unwrap();
        let listed = models_for(&store, std::slice::from_ref(&provider), |_| true).await;
        assert_eq!(listed[0].fetched_at, Some(900));
        assert_eq!(listed[0].models.len(), 101);

        forget(&store, provider.id).await;
        assert_eq!(saved(&store, provider.id).await, None);
        let listed = models_for(&store, &[provider], |_| true).await;
        assert_eq!(listed[0].fetched_at, None);
        assert!(listed[0].models.is_empty());
    }

    #[tokio::test]
    async fn a_picture_price_says_when_it_was_fetched() {
        let store = store::Store::open_in_memory().unwrap();
        let provider = openrouter(&store).await;
        let fake = Fake::openrouter();
        refresh(&store, provider.id, &fake, 500).await.unwrap();
        refresh_price(&store, provider.id, &fake, FLUX, 1000)
            .await
            .unwrap();

        let listed = models_for(&store, &[provider], |_| true).await;
        let endpoint = &listed[0];
        let flux = priced(endpoint, FLUX);
        assert_eq!(flux.price_fetched_at, Some(1000));
        assert!(matches!(flux.model.price, Price::PerImage(_)));
        assert_eq!(flux.endpoints, Some(recorded_answer(FLUX)));
        let other = priced(endpoint, NANO_BANANA);
        assert_eq!(other.model.kind, MediaKind::Image);
        assert_eq!(other.price_fetched_at, None);
        // A video's price is in the list itself.
        let video = priced(endpoint, VEO);
        assert_eq!(video.model.kind, MediaKind::Video);
        assert_eq!(video.price_fetched_at, Some(500));
        assert_eq!(video.endpoints, None);
    }

    #[tokio::test]
    async fn an_updated_list_keeps_the_prices_of_the_pictures_it_still_has() {
        let store = store::Store::open_in_memory().unwrap();
        let provider = openrouter(&store).await;
        let kept = CachedPrice {
            fetched_at: 700,
            raw: recorded_answer(FLUX),
        };
        let lists = CachedCatalog {
            fetched_at: 500,
            raw: recorded_lists(),
            image_prices: BTreeMap::from([
                (FLUX.to_owned(), kept.clone()),
                (
                    "gone/withdrawn-image".to_owned(),
                    CachedPrice {
                        fetched_at: 600,
                        raw: recorded_answer(FLUX),
                    },
                ),
            ]),
        };
        save(&store, provider.id, &lists).await;

        refresh(&store, provider.id, &Fake::openrouter(), 900)
            .await
            .unwrap();
        let updated = saved(&store, provider.id).await.unwrap();
        assert_eq!(updated.fetched_at, 900);
        assert_eq!(
            updated.image_prices,
            BTreeMap::from([(FLUX.to_owned(), kept)])
        );
    }

    #[tokio::test]
    async fn a_price_is_fetched_only_for_a_picture_in_the_saved_list() {
        let store = store::Store::open_in_memory().unwrap();
        let provider = openrouter(&store).await;
        let fake = Fake::openrouter();
        // No list saved yet: nothing is asked.
        let refused = refresh_price(&store, provider.id, &fake, FLUX, 1000).await;
        assert!(
            matches!(refused, Err(MediaError::NotSent(_))),
            "{refused:?}"
        );
        assert_eq!(fake.requests(), 0);

        refresh(&store, provider.id, &fake, 500).await.unwrap();
        assert_eq!(fake.requests(), 1);
        for not_a_listed_picture in [VEO, "nobody/nothing", "../../models"] {
            let refused =
                refresh_price(&store, provider.id, &fake, not_a_listed_picture, 1000).await;
            assert!(
                matches!(refused, Err(MediaError::NotSent(_))),
                "{refused:?}"
            );
        }
        assert_eq!(fake.requests(), 1);
        assert!(
            saved(&store, provider.id)
                .await
                .unwrap()
                .image_prices
                .is_empty()
        );
    }

    #[tokio::test]
    async fn an_answer_without_a_price_is_not_saved() {
        // OpenAI's prices are in the bundled settings file: its price answer is `null`.
        let store = store::Store::open_in_memory().unwrap();
        let provider = endpoint(&store, "openai", "https://api.openai.com/v1").await;
        let fake = Fake {
            company: Company::OpenAi,
            catalog: Ok(RawCatalog {
                images: serde_json::from_str(OPENAI_MODELS).unwrap(),
                videos: None,
            }),
            price: Value::Null,
            requests: AtomicUsize::new(0),
        };
        refresh(&store, provider.id, &fake, 500).await.unwrap();
        let listed = models_for(&store, std::slice::from_ref(&provider), |_| true).await;
        let ids: Vec<&str> = listed[0]
            .models
            .iter()
            .map(|m| m.model.id.as_str())
            .collect();
        assert_eq!(ids, ["gpt-image-2.6-flare", "gpt-image-2.6-sunburst"]);
        // OpenAI's pictures are priced with the list.
        assert_eq!(listed[0].models[0].price_fetched_at, Some(500));

        refresh_price(&store, provider.id, &fake, OPENAI_FLARE, 1000)
            .await
            .unwrap();
        assert!(
            saved(&store, provider.id)
                .await
                .unwrap()
                .image_prices
                .is_empty()
        );
        let (_, model) = model(&store, provider.id, OPENAI_FLARE).await.unwrap();
        assert_eq!(model.price_fetched_at, Some(500));
        assert_eq!(model.endpoints, None);
    }

    #[tokio::test]
    async fn nothing_is_saved_for_a_deleted_endpoint() {
        let store = store::Store::open_in_memory().unwrap();
        let provider = openrouter(&store).await;
        store.delete_provider(provider.id).await.unwrap();
        refresh(&store, provider.id, &Fake::openrouter(), 500)
            .await
            .unwrap();
        assert_eq!(saved(&store, provider.id).await, None);
    }

    #[test]
    fn the_settings_key_names_the_endpoint() {
        assert_eq!(cache_key(store::ProviderId(7)), "providerMediaModels:7");
    }
}
