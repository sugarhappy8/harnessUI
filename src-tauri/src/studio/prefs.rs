//! Studio's preferences (Settings → Studio), kept as one setting ([`PREFS_KEY`]).
//!
//! - **Defaults.** A daily limit of [`DEFAULT_DAILY_LIMIT_USD`] US dollars; every cloud
//!   video, and any cloud run estimated at [`CONFIRM_AT_USD`] US dollars or more, asks before
//!   it is sent; a local job that does not fit beside the chat model unloads it
//!   ([`GpuPolicy::Unload`]).
//! - **Reading** ([`load`]): fields missing from what was saved (prefs saved by an earlier
//!   version) read as their defaults; a value that is missing, cannot be read or breaks a rule
//!   reads as the defaults.
//! - **Rules** ([`StudioPrefs::validate`]): the limit is off or within
//!   [`DAILY_LIMIT_RANGE`], the confirmation amount within [`CONFIRM_AT_RANGE`], at most
//!   [`MAX_SETUPS`] setups, each with a name. [`save`] checks them first and, when the gallery
//!   folder changed, that the new folder is a full path, exists, takes files and has no `%d`,
//!   `%3d` or `%03d` in its path (the Studio engine would read it as a numbering template).
//!   A new gallery folder applies to new results only: existing results keep their absolute
//!   paths and stay where they are.
//! - **Changes** ([`StudioPrefs::apply`]): only the fields a [`StudioPrefsPatch`] sends
//!   change; `null` turns the limit off or puts the gallery back in its default place. No
//!   patch sets when the note about local models was accepted (`studio_accept_local_notice`
//!   records it), or how the last model file was added (`studio_import_model` records it).

use std::collections::BTreeMap;
use std::ops::RangeInclusive;
use std::path::PathBuf;

use agent::media::{MediaKind, Routing};
use runtime::sd::args::is_numbering_template;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::errors;
use super::import::ImportMode;
use super::spend::{CONFIRM_AT_USD, ConfirmRules, DEFAULT_DAILY_LIMIT_USD};
use crate::error::{AppError, AppResult};
use crate::hub::check_writable_folder;

/// The settings key the preferences are kept under, as one JSON value.
pub const PREFS_KEY: &str = "studioPrefs";
/// The most setups the user can save.
pub const MAX_SETUPS: usize = 50;
/// What the daily limit can be set to, in US dollars, when it is on.
pub const DAILY_LIMIT_RANGE: RangeInclusive<f64> = 1.0..=1000.0;
/// What the amount from which a cloud run asks before it is sent can be set to, in US dollars.
pub const CONFIRM_AT_RANGE: RangeInclusive<f64> = 0.25..=100.0;

/// Where a run goes: an endpoint, or this computer. Serialised `{"endpoint": 3}` or
/// `"thisComputer"`. Named `RunTarget`, not `Target`, because `models::fit::Target` (the
/// hardware) is used beside it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunTarget {
    /// An endpoint the user added (OpenRouter or OpenAI), by its id.
    Endpoint(i64),
    /// This computer, through the Studio engine.
    ThisComputer,
}

/// What happens when a local job does not fit beside the loaded chat model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GpuPolicy {
    /// Unload the chat model to make room.
    #[default]
    Unload,
    /// Ask each time.
    Ask,
    /// Run local pictures on the processor.
    Processor,
}

/// The model last used for pictures or for video.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelChoice {
    /// Where it runs.
    pub target: RunTarget,
    /// The model's id there.
    pub model: String,
}

/// A saved setup: a model and its settings under a name, to use again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Setup {
    /// The name the user gave it.
    pub name: String,
    /// Pictures or video.
    pub kind: MediaKind,
    /// Where it runs.
    pub target: RunTarget,
    /// The model's id there.
    pub model: String,
    /// The model's settings, by name.
    pub params: Map<String, Value>,
    /// Extra settings (JSON) sent as they are.
    pub extra: Map<String, Value>,
    /// Which companies may run an OpenRouter request.
    pub routing: Option<Routing>,
}

/// Settings → Studio.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StudioPrefs {
    /// The model last used, for pictures and for video.
    pub last_model: BTreeMap<MediaKind, ModelChoice>,
    /// The advanced settings are open.
    pub advanced_open: bool,
    /// 0.2.0: Studio's side panel (Settings) was left open; it stays docked on a wide window.
    #[serde(default)]
    pub settings_open: bool,
    /// Every cloud video asks before it is sent.
    pub confirm_every_video: bool,
    /// A cloud run estimated at this many US dollars or more asks before it is sent.
    pub confirm_at_usd: f64,
    /// The daily spending limit for cloud runs, in US dollars; `None` when it is off.
    pub daily_limit_usd: Option<f64>,
    /// The saved setups.
    pub setups: Vec<Setup>,
    /// The gallery folder; `None` for the default one.
    pub gallery_dir: Option<String>,
    /// What happens when a local job does not fit beside the chat model.
    pub gpu_policy: GpuPolicy,
    /// Load the chat model again after the last queued local job.
    pub reload_chat_after: bool,
    /// The one-time first-run suggestion was dismissed.
    pub suggestion_dismissed: bool,
    /// When the note about local models was accepted, in Unix ms. Set only by
    /// `studio_accept_local_notice`, never by a patch.
    pub local_notice_accepted_at: Option<i64>,
    /// 0.2.0: how the last model file from this computer was added (copied into the models
    /// folder, or used where it is), preselected the next time. Set only by
    /// `studio_import_model`, never by a patch.
    #[serde(default)]
    pub last_import_mode: Option<ImportMode>,
}

impl Default for StudioPrefs {
    fn default() -> Self {
        Self {
            last_model: BTreeMap::new(),
            advanced_open: false,
            settings_open: false,
            confirm_every_video: true,
            confirm_at_usd: CONFIRM_AT_USD,
            daily_limit_usd: Some(DEFAULT_DAILY_LIMIT_USD),
            setups: Vec::new(),
            gallery_dir: None,
            gpu_policy: GpuPolicy::Unload,
            reload_chat_after: false,
            suggestion_dismissed: false,
            local_notice_accepted_at: None,
            last_import_mode: None,
        }
    }
}

/// A change to [`StudioPrefs`]: only the fields sent change. For the limit and the gallery
/// folder, a missing field keeps the value and `null` clears it (the limit off, the default
/// gallery folder).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StudioPrefsPatch {
    /// See [`StudioPrefs::last_model`].
    pub last_model: Option<BTreeMap<MediaKind, ModelChoice>>,
    /// See [`StudioPrefs::advanced_open`].
    pub advanced_open: Option<bool>,
    /// See [`StudioPrefs::settings_open`].
    pub settings_open: Option<bool>,
    /// See [`StudioPrefs::confirm_every_video`].
    pub confirm_every_video: Option<bool>,
    /// See [`StudioPrefs::confirm_at_usd`].
    pub confirm_at_usd: Option<f64>,
    /// See [`StudioPrefs::daily_limit_usd`]; `Some(None)` turns the limit off.
    #[serde(default, with = "store::double_option")]
    pub daily_limit_usd: Option<Option<f64>>,
    /// See [`StudioPrefs::setups`].
    pub setups: Option<Vec<Setup>>,
    /// See [`StudioPrefs::gallery_dir`]; `Some(None)` goes back to the default folder.
    #[serde(default, with = "store::double_option")]
    pub gallery_dir: Option<Option<String>>,
    /// See [`StudioPrefs::gpu_policy`].
    pub gpu_policy: Option<GpuPolicy>,
    /// See [`StudioPrefs::reload_chat_after`].
    pub reload_chat_after: Option<bool>,
    /// See [`StudioPrefs::suggestion_dismissed`].
    pub suggestion_dismissed: Option<bool>,
}

impl StudioPrefs {
    /// When a cloud run asks before it is sent.
    pub fn confirm_rules(&self) -> ConfirmRules {
        ConfirmRules {
            every_video: self.confirm_every_video,
            at_usd: self.confirm_at_usd,
        }
    }

    /// The rules the preferences keep, as the sentence to show when one is broken. The
    /// gallery folder is checked by [`save`], which can look at the disk.
    pub fn validate(&self) -> Result<(), String> {
        if self
            .daily_limit_usd
            .is_some_and(|usd| !DAILY_LIMIT_RANGE.contains(&usd))
        {
            return Err(errors::LIMIT_OUT_OF_RANGE.into());
        }
        if !CONFIRM_AT_RANGE.contains(&self.confirm_at_usd) {
            return Err(errors::CONFIRM_AT_OUT_OF_RANGE.into());
        }
        if self.setups.len() > MAX_SETUPS {
            return Err(errors::TOO_MANY_SETUPS.into());
        }
        if self.setups.iter().any(|s| s.name.trim().is_empty()) {
            return Err(errors::SETUP_WITHOUT_NAME.into());
        }
        Ok(())
    }

    /// These preferences with `patch` applied (only the fields it sends change), if the
    /// result keeps the rules.
    pub fn apply(&self, patch: StudioPrefsPatch) -> Result<StudioPrefs, String> {
        let StudioPrefsPatch {
            last_model,
            advanced_open,
            settings_open,
            confirm_every_video,
            confirm_at_usd,
            daily_limit_usd,
            setups,
            gallery_dir,
            gpu_policy,
            reload_chat_after,
            suggestion_dismissed,
        } = patch;
        let next = StudioPrefs {
            last_model: last_model.unwrap_or_else(|| self.last_model.clone()),
            advanced_open: advanced_open.unwrap_or(self.advanced_open),
            settings_open: settings_open.unwrap_or(self.settings_open),
            confirm_every_video: confirm_every_video.unwrap_or(self.confirm_every_video),
            confirm_at_usd: confirm_at_usd.unwrap_or(self.confirm_at_usd),
            daily_limit_usd: daily_limit_usd.unwrap_or(self.daily_limit_usd),
            setups: setups.unwrap_or_else(|| self.setups.clone()),
            gallery_dir: gallery_dir.unwrap_or_else(|| self.gallery_dir.clone()),
            gpu_policy: gpu_policy.unwrap_or(self.gpu_policy),
            reload_chat_after: reload_chat_after.unwrap_or(self.reload_chat_after),
            suggestion_dismissed: suggestion_dismissed.unwrap_or(self.suggestion_dismissed),
            local_notice_accepted_at: self.local_notice_accepted_at,
            last_import_mode: self.last_import_mode,
        };
        next.validate()?;
        Ok(next)
    }
}

/// The saved preferences; the defaults when none were saved, or what was saved cannot be
/// read or breaks a rule.
pub async fn load(store: &store::Store) -> StudioPrefs {
    match store.get_setting::<StudioPrefs>(PREFS_KEY).await {
        Ok(Some(prefs)) => match prefs.validate() {
            Ok(()) => prefs,
            Err(rule) => {
                tracing::warn!(
                    "the saved Studio preferences break a rule ({rule}); using the defaults"
                );
                StudioPrefs::default()
            }
        },
        Ok(None) => StudioPrefs::default(),
        Err(e) => {
            tracing::warn!(
                "the saved Studio preferences could not be read ({e}); using the defaults"
            );
            StudioPrefs::default()
        }
    }
}

/// Saves `prefs` once they keep the rules ([`StudioPrefs::validate`], refused as
/// `bad_request`) and, when the gallery folder changed, the new folder passes its check.
/// A folder saved earlier is not checked again, so other settings still save while it is
/// away (a drive unplugged).
pub async fn save(store: &store::Store, prefs: &StudioPrefs) -> AppResult<()> {
    prefs.validate().map_err(AppError::bad_request)?;
    if let Some(dir) = &prefs.gallery_dir
        && load(store).await.gallery_dir.as_ref() != Some(dir)
    {
        check_gallery_dir(PathBuf::from(dir)).await?;
    }
    store.set_setting(PREFS_KEY, prefs).await?;
    Ok(())
}

/// A gallery folder the user picked: it is a full path (results keep absolute paths), it
/// exists and takes files (the models folder's check), and its path holds no numbering
/// template the Studio engine would read in `-o`.
async fn check_gallery_dir(dir: PathBuf) -> AppResult<()> {
    if !dir.is_absolute() {
        return Err(errors::gallery_not_full_path());
    }
    tokio::task::spawn_blocking(move || {
        check_writable_folder(&dir)?;
        if is_numbering_template(&dir.to_string_lossy()) {
            return Err(errors::numbered_folder());
        }
        Ok(())
    })
    .await
    .map_err(AppError::internal)?
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::studio::test_support::Scratch;
    use serde_json::json;

    fn patch(v: Value) -> StudioPrefsPatch {
        serde_json::from_value(v).unwrap()
    }

    fn with_limit(daily_limit_usd: Option<f64>) -> StudioPrefs {
        StudioPrefs {
            daily_limit_usd,
            ..Default::default()
        }
    }

    fn with_confirm_at(confirm_at_usd: f64) -> StudioPrefs {
        StudioPrefs {
            confirm_at_usd,
            ..Default::default()
        }
    }

    fn setup(name: &str) -> Setup {
        Setup {
            name: name.into(),
            kind: MediaKind::Image,
            target: RunTarget::Endpoint(3),
            model: "black-forest-labs/flux-3-image".into(),
            params: Map::new(),
            extra: Map::new(),
            routing: None,
        }
    }

    fn with_setups(n: usize) -> StudioPrefs {
        StudioPrefs {
            setups: (0..n).map(|i| setup(&format!("Setup {i}"))).collect(),
            ..Default::default()
        }
    }

    fn with_setup_named(name: &str) -> StudioPrefs {
        StudioPrefs {
            setups: vec![setup(name)],
            ..Default::default()
        }
    }

    fn with_gallery(dir: &Path) -> StudioPrefs {
        StudioPrefs {
            gallery_dir: Some(dir.to_string_lossy().into_owned()),
            ..Default::default()
        }
    }

    #[test]
    fn the_defaults_are_the_owners() {
        let p = StudioPrefs::default();
        assert_eq!(p.daily_limit_usd, Some(10.0));
        assert!(p.confirm_every_video);
        assert_eq!(p.confirm_at_usd, 1.0);
        assert_eq!(p.gpu_policy, GpuPolicy::Unload);
        assert!(
            !p.reload_chat_after && !p.suggestion_dismissed && p.local_notice_accepted_at.is_none()
        );
        assert!(p.last_model.is_empty() && p.setups.is_empty() && p.gallery_dir.is_none());
        assert!(!p.advanced_open);
        assert_eq!(p.validate(), Ok(()));
    }

    #[test]
    fn targets_serialise_for_the_webview() {
        assert_eq!(
            serde_json::to_value(RunTarget::Endpoint(3)).unwrap(),
            json!({"endpoint": 3})
        );
        assert_eq!(
            serde_json::to_value(RunTarget::ThisComputer).unwrap(),
            json!("thisComputer")
        );
        assert_eq!(
            serde_json::from_value::<RunTarget>(json!({"endpoint": 3})).unwrap(),
            RunTarget::Endpoint(3)
        );
        assert_eq!(
            serde_json::from_value::<RunTarget>(json!("thisComputer")).unwrap(),
            RunTarget::ThisComputer
        );
    }

    #[test]
    fn prefs_serialise_for_the_webview() {
        let p = StudioPrefs {
            last_model: BTreeMap::from([(
                MediaKind::Image,
                ModelChoice {
                    target: RunTarget::Endpoint(3),
                    model: "openai/gpt-image-2".into(),
                },
            )]),
            daily_limit_usd: None,
            ..Default::default()
        };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(
            v["lastModel"],
            json!({"image": {"target": {"endpoint": 3}, "model": "openai/gpt-image-2"}})
        );
        assert_eq!(v["dailyLimitUsd"], Value::Null);
        assert_eq!(v["gpuPolicy"], json!("unload"));
        assert_eq!(v["localNoticeAcceptedAt"], Value::Null);
        assert_eq!(serde_json::from_value::<StudioPrefs>(v).unwrap(), p);
    }

    #[test]
    fn a_patch_cannot_accept_the_local_notice() {
        assert_eq!(
            StudioPrefs::default()
                .apply(patch(json!({"localNoticeAcceptedAt": 1})))
                .unwrap()
                .local_notice_accepted_at,
            None
        );
        assert_eq!(
            StudioPrefs::default()
                .apply(patch(json!({"gpuPolicy": "ask"})))
                .unwrap()
                .gpu_policy,
            GpuPolicy::Ask
        );
        let accepted = StudioPrefs {
            local_notice_accepted_at: Some(7),
            ..Default::default()
        };
        assert_eq!(
            accepted
                .apply(patch(json!({"localNoticeAcceptedAt": null})))
                .unwrap()
                .local_notice_accepted_at,
            Some(7)
        );
    }

    /// The last import's mode reads as the webview writes it, is none in prefs saved before
    /// 0.2.0, and no patch sets or clears it.
    #[test]
    fn the_last_import_mode_is_kept_and_no_patch_changes_it() {
        assert_eq!(StudioPrefs::default().last_import_mode, None);
        let copied = StudioPrefs {
            last_import_mode: Some(ImportMode::Copy),
            ..Default::default()
        };
        let v = serde_json::to_value(&copied).unwrap();
        assert_eq!(v["lastImportMode"], json!("copy"));
        assert_eq!(serde_json::from_value::<StudioPrefs>(v).unwrap(), copied);
        let in_place: StudioPrefs =
            serde_json::from_value(json!({"lastImportMode": "inPlace"})).unwrap();
        assert_eq!(in_place.last_import_mode, Some(ImportMode::InPlace));
        let before: StudioPrefs = serde_json::from_value(json!({"advancedOpen": true})).unwrap();
        assert_eq!(before.last_import_mode, None);
        for sent in [
            json!({"lastImportMode": "inPlace"}),
            json!({"lastImportMode": null}),
        ] {
            assert_eq!(
                copied.apply(patch(sent)).unwrap().last_import_mode,
                Some(ImportMode::Copy)
            );
        }
    }

    #[test]
    fn the_limit_is_between_1_and_1000_dollars_or_off() {
        for bad in [0.5, 1000.01, f64::NAN] {
            assert_eq!(
                with_limit(Some(bad)).validate(),
                Err("Choose an amount from $1 to $1,000.".into())
            );
        }
        for good in [Some(1.0), Some(1000.0), None] {
            assert!(with_limit(good).validate().is_ok());
        }
    }

    #[test]
    fn the_confirmation_amount_is_between_25_cents_and_100_dollars() {
        for bad in [0.2, 100.5, f64::NAN] {
            assert_eq!(
                with_confirm_at(bad).validate(),
                Err("Choose an amount from $0.25 to $100.".into())
            );
        }
        for good in [0.25, 2.5, 100.0] {
            assert!(with_confirm_at(good).validate().is_ok());
        }
    }

    #[test]
    fn setups_stop_at_50_and_need_a_name() {
        assert_eq!(
            with_setups(51).validate(),
            Err("Keep at most 50 setups.".into())
        );
        assert!(with_setups(MAX_SETUPS).validate().is_ok());
        assert_eq!(
            with_setup_named("  ").validate(),
            Err("Give the setup a name.".into())
        );
    }

    #[test]
    fn a_patch_changes_only_the_fields_it_sends() {
        let p = StudioPrefs {
            daily_limit_usd: Some(25.0),
            ..Default::default()
        };
        let q = p.apply(patch(json!({"confirmEveryVideo": false}))).unwrap();
        assert_eq!(
            (q.confirm_every_video, q.daily_limit_usd),
            (false, Some(25.0))
        );
        assert_eq!(
            q,
            StudioPrefs {
                confirm_every_video: false,
                ..p.clone()
            }
        );
        assert_eq!(
            p.apply(patch(json!({"dailyLimitUsd": null})))
                .unwrap()
                .daily_limit_usd,
            None
        );
        assert_eq!(p.apply(patch(json!({}))).unwrap(), p);
    }

    #[test]
    fn a_patch_sets_every_field_it_names() {
        let q = StudioPrefs::default()
            .apply(patch(json!({
                "lastModel": {"video": {"target": "thisComputer", "model": "wan-2.2-ti2v-5b"}},
                "advancedOpen": true,
                "confirmEveryVideo": false,
                "confirmAtUsd": 2.5,
                "dailyLimitUsd": 40,
                "setups": [{
                    "name": "Posters", "kind": "image", "target": {"endpoint": 3},
                    "model": "black-forest-labs/flux-3-image", "params": {"n": 2}, "extra": {},
                    "routing": {"order": ["bfl"], "only": true, "allowFallbacks": false, "zdr": true}
                }],
                "galleryDir": "/pictures/studio",
                "gpuPolicy": "processor",
                "reloadChatAfter": true,
                "suggestionDismissed": true
            })))
            .unwrap();
        assert_eq!(
            q.last_model[&MediaKind::Video],
            ModelChoice {
                target: RunTarget::ThisComputer,
                model: "wan-2.2-ti2v-5b".into()
            }
        );
        assert!(q.advanced_open && !q.confirm_every_video);
        assert_eq!((q.confirm_at_usd, q.daily_limit_usd), (2.5, Some(40.0)));
        assert_eq!(q.setups.len(), 1);
        assert_eq!(q.setups[0].params["n"], json!(2));
        assert_eq!(
            q.setups[0].routing,
            Some(Routing {
                order: vec!["bfl".into()],
                only: true,
                allow_fallbacks: false,
                zdr: true
            })
        );
        assert_eq!(q.gallery_dir.as_deref(), Some("/pictures/studio"));
        assert_eq!(q.gpu_policy, GpuPolicy::Processor);
        assert!(q.reload_chat_after && q.suggestion_dismissed);
        let cleared = q.apply(patch(json!({"galleryDir": null}))).unwrap();
        assert_eq!(cleared.gallery_dir, None);
    }

    #[test]
    fn settings_open_defaults_to_false_and_round_trips() {
        assert!(!StudioPrefs::default().settings_open);
        // Saved before 0.2.0, without the field: the side panel starts closed.
        let old: StudioPrefs = serde_json::from_value(json!({"advancedOpen": true})).unwrap();
        assert!(!old.settings_open && old.advanced_open);
        // A patch that names it sets it, and one that does not keeps it.
        let open = old.apply(patch(json!({"settingsOpen": true}))).unwrap();
        assert!(open.settings_open);
        assert!(open.apply(patch(json!({}))).unwrap().settings_open);
        let v = serde_json::to_value(&open).unwrap();
        assert_eq!(v["settingsOpen"], json!(true));
        assert_eq!(serde_json::from_value::<StudioPrefs>(v).unwrap(), open);
    }

    #[test]
    fn a_patch_that_breaks_a_rule_is_refused() {
        assert_eq!(
            StudioPrefs::default().apply(patch(json!({"dailyLimitUsd": 0.5}))),
            Err("Choose an amount from $1 to $1,000.".into())
        );
    }

    #[test]
    fn the_confirmation_rules_come_from_the_prefs() {
        assert_eq!(
            StudioPrefs::default().confirm_rules(),
            ConfirmRules::default()
        );
        let p = StudioPrefs {
            confirm_every_video: false,
            confirm_at_usd: 3.0,
            ..Default::default()
        };
        assert_eq!(
            p.confirm_rules(),
            ConfirmRules {
                every_video: false,
                at_usd: 3.0
            }
        );
    }

    #[test]
    fn a_numbering_template_is_a_percent_up_to_three_digits_and_d() {
        for template in ["shots %d", "a/%3d/b", "%03d.png", "x%123d", "50%%d"] {
            assert!(is_numbering_template(template), "{template}");
        }
        for plain in ["shots", "100% done", "%1234d", "%x", "%", "d%", "%0x3d"] {
            assert!(!is_numbering_template(plain), "{plain}");
        }
    }

    #[tokio::test]
    async fn prefs_saved_without_new_fields_read_with_defaults() {
        let store = store::Store::open_in_memory().unwrap();
        store
            .set_setting(PREFS_KEY, &json!({"advancedOpen": true}))
            .await
            .unwrap();
        let p = load(&store).await;
        assert!(p.advanced_open);
        assert_eq!(p.daily_limit_usd, Some(10.0));
        assert_eq!(p.gpu_policy, GpuPolicy::Unload);
    }

    #[tokio::test]
    async fn missing_unreadable_or_broken_prefs_read_as_the_defaults() {
        let store = store::Store::open_in_memory().unwrap();
        assert_eq!(load(&store).await, StudioPrefs::default());
        for saved in [
            json!("not prefs"),
            json!({"advancedOpen": true, "gpuPolicy": "cuda"}),
            json!({"advancedOpen": true, "dailyLimitUsd": 0.5}),
        ] {
            store.set_setting(PREFS_KEY, &saved).await.unwrap();
            assert_eq!(load(&store).await, StudioPrefs::default(), "{saved}");
        }
    }

    #[tokio::test]
    async fn saved_prefs_read_back_including_a_limit_turned_off() {
        let store = store::Store::open_in_memory().unwrap();
        let p = StudioPrefs {
            last_model: BTreeMap::from([(
                MediaKind::Image,
                ModelChoice {
                    target: RunTarget::ThisComputer,
                    model: "flux-2-klein-4b".into(),
                },
            )]),
            daily_limit_usd: None,
            setups: vec![setup("Posters")],
            local_notice_accepted_at: Some(1_794_873_600_000),
            ..Default::default()
        };
        save(&store, &p).await.unwrap();
        assert_eq!(load(&store).await, p);
    }

    #[tokio::test]
    async fn save_refuses_prefs_that_break_a_rule() {
        let store = store::Store::open_in_memory().unwrap();
        let err = save(&store, &with_setups(51)).await.unwrap_err();
        assert_eq!(
            (err.code.as_str(), err.message.as_str()),
            ("bad_request", "Keep at most 50 setups.")
        );
        assert_eq!(load(&store).await, StudioPrefs::default());
    }

    #[tokio::test]
    async fn a_gallery_folder_must_exist_and_take_files() {
        let store = store::Store::open_in_memory().unwrap();
        let scratch = Scratch::new();

        let missing = scratch.path().join("missing");
        let err = save(&store, &with_gallery(&missing)).await.unwrap_err();
        assert_eq!(err.message, "Pick an existing folder.");

        let file = scratch.write("a file", b"not a folder");
        let err = save(&store, &with_gallery(&file)).await.unwrap_err();
        assert_eq!(err.message, "Pick an existing folder.");

        // sd-cli reads `-o` as a numbering template when it holds %d, %3d or %03d. It sees the
        // whole path, so a folder above the one picked counts too.
        let numbered = scratch.path().join("shots %03d");
        let below = numbered.join("pictures");
        std::fs::create_dir_all(&below).unwrap();
        for dir in [&numbered, &below] {
            let err = save(&store, &with_gallery(dir)).await.unwrap_err();
            assert_eq!(
                (
                    err.code.as_str(),
                    err.message.as_str(),
                    err.next_step.as_str()
                ),
                (
                    "bad_request",
                    "Pick a folder whose name has no %d in it.",
                    "Choose a folder with no %d in its name or in the names of the folders above it.",
                ),
                "{}",
                dir.display()
            );
        }
        assert_eq!(load(&store).await.gallery_dir, None);

        let good = scratch.path().join("shots");
        std::fs::create_dir(&good).unwrap();
        save(&store, &with_gallery(&good)).await.unwrap();
        assert_eq!(load(&store).await, with_gallery(&good));
        assert!(!good.join(".write-test").exists());
    }

    #[tokio::test]
    async fn a_gallery_folder_must_be_a_full_path() {
        let store = store::Store::open_in_memory().unwrap();
        // "." exists and takes files, but it means wherever the app was started.
        for relative in [".", "shots"] {
            let p = StudioPrefs {
                gallery_dir: Some(relative.into()),
                ..Default::default()
            };
            let err = save(&store, &p).await.unwrap_err();
            assert_eq!(
                (
                    err.code.as_str(),
                    err.message.as_str(),
                    err.next_step.as_str()
                ),
                (
                    "bad_request",
                    "Pick an existing folder.",
                    "Choose the folder again."
                ),
                "{relative}"
            );
        }
        assert_eq!(load(&store).await.gallery_dir, None);
    }

    #[tokio::test]
    async fn a_saved_gallery_folder_is_checked_only_when_it_changes() {
        let store = store::Store::open_in_memory().unwrap();
        let scratch = Scratch::new();
        let gallery = scratch.path().join("gallery");
        std::fs::create_dir(&gallery).unwrap();
        save(&store, &with_gallery(&gallery)).await.unwrap();

        // The folder goes away (a drive unplugged): other settings still save.
        std::fs::remove_dir(&gallery).unwrap();
        let p = StudioPrefs {
            advanced_open: true,
            ..with_gallery(&gallery)
        };
        save(&store, &p).await.unwrap();
        assert_eq!(load(&store).await, p);
    }
}
