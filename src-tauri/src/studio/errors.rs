//! Studio's backend wording: every sentence a Studio error shows, in plain English.
//!
//! Each error is a four-part [`AppError`]: a code the webview can test, a message and a next
//! step in plain words, and details for a bug report. No message shows a raw setting key
//! (an Extra field the user typed is quoted), a path or a hash.

use agent::media::{
    Company, FailureKind, Invalid, LocalIo, MediaFailure, MediaKind, MediaModel, RefRole, Stage,
};
use models::gen_fit::{GenSize, NotFor};
use models::gen_search::NotOffered;
use runtime::packs::PackError;
use runtime::sd::args::ArgsError;
use runtime::sd::failure::SdFailure;
use search::FetchError;

use super::gallery::GalleryError;
use super::spend::{self, ConfirmReason};
use crate::error::AppError;

/// Settings → Studio: the daily limit is outside [`super::prefs::DAILY_LIMIT_RANGE`].
pub const LIMIT_OUT_OF_RANGE: &str = "Choose an amount from $1 to $1,000.";
/// Settings → Studio: the "ask before a run costing at least" amount is outside
/// [`super::prefs::CONFIRM_AT_RANGE`].
pub const CONFIRM_AT_OUT_OF_RANGE: &str = "Choose an amount from $0.25 to $100.";
/// Settings → Studio: more than [`super::prefs::MAX_SETUPS`] saved setups.
pub const TOO_MANY_SETUPS: &str = "Keep at most 50 setups.";
/// Settings → Studio: a saved setup without a name.
pub const SETUP_WITHOUT_NAME: &str = "Give the setup a name.";

/// The gallery folder given is not a full path: it would be read from the folder the app was
/// started in, so results would land there, while results keep absolute paths.
pub fn gallery_not_full_path() -> AppError {
    AppError::new(
        "bad_request",
        "Pick an existing folder.",
        "Choose the folder again.",
        String::new(),
    )
}

/// The gallery folder picked has `%d`, `%3d` or `%03d` in its path, which the Studio
/// engine would read as a numbering template for the files it writes there. The whole path
/// counts, so a folder above the one picked can be the cause.
pub fn numbered_folder() -> AppError {
    AppError::new(
        "bad_request",
        "Pick a folder whose name has no %d in it.",
        "Choose a folder with no %d in its name or in the names of the folders above it.",
        String::new(),
    )
}

// ---- names and amounts ------------------------------------------------------------------------

/// A company's name as shown.
fn name(company: Company) -> &'static str {
    match company {
        Company::OpenRouter => "OpenRouter",
        Company::OpenAi => "OpenAI",
        Company::Local => "This computer",
    }
}

/// Where the user can see what a company charged. Only for a cloud company: every
/// constructor words [`Company::Local`] without a website (tested by
/// `a_run_on_this_computer_never_points_to_a_website`).
fn activity_page(company: Company) -> &'static str {
    match company {
        Company::OpenAi => "platform.openai.com",
        Company::OpenRouter | Company::Local => "openrouter.ai",
    }
}

/// `n` with a comma between each group of three digits: `20000` → `20,000`.
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// A dollar amount with cents: `9.5` → `$9.50`, `1234.5` → `$1,234.50`.
pub(crate) fn usd(amount: f64) -> String {
    let cents = (amount.max(0.0) * 100.0).round();
    // Dollar amounts here are small and never negative; `as` saturates anything else.
    let cents = cents as u64;
    format!("${}.{:02}", grouped(cents / 100), cents % 100)
}

/// A dollar amount without cents when it is whole: `10.0` → `$10`, `12.5` → `$12.50`.
fn usd_short(amount: f64) -> String {
    let full = usd(amount);
    match full.strip_suffix(".00") {
        Some(whole) => whole.to_owned(),
        None => full,
    }
}

/// A file size cap as shown: `52428800` → `50 MB`, `1073741824` → `1 GB`.
fn size(bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * MIB;
    if bytes >= GIB && bytes.is_multiple_of(GIB) {
        format!("{} GB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{} MB", grouped(bytes.div_ceil(MIB)))
    } else {
        format!("{} bytes", grouped(bytes))
    }
}

/// A run on this computer whose Studio engine did not answer, in place of a company that did
/// not.
const ENGINE_DID_NOT_ANSWER: &str = "The Studio engine on this computer did not answer.";
/// A run on this computer that failed in a way only a company reports (a key, credit, its
/// server), in place of that company's words.
const ENGINE_COULD_NOT: &str = "The Studio engine on this computer could not make it.";
/// The next step after a run on this computer failed for no reason the user can change.
const TRY_AGAIN_OR_RESTART: &str = "Try again; if it keeps happening, restart the app.";

/// What a run was charged, as the start of a next step.
fn charge_line(charged: Option<bool>) -> &'static str {
    match charged {
        Some(false) => "Not charged.",
        Some(true) => "Charged.",
        None => "It may have been charged.",
    }
}

// ---- refusals before a run is sent ------------------------------------------------------------

/// The run needs the user's confirmation first (a cloud video, an estimate at or above the
/// amount in Settings → Studio, or an unknown price) and the request did not carry it.
pub fn needs_confirmation(reasons: &[ConfirmReason]) -> AppError {
    let why: Vec<&str> = reasons
        .iter()
        .map(|reason| match reason {
            ConfirmReason::Video => "it is a cloud video",
            ConfirmReason::AtLeast => "its estimate is at or above the amount that asks",
            ConfirmReason::UnknownPrice => "its price is unknown",
        })
        .collect();
    AppError::new(
        "studio_needs_confirmation",
        "Confirm this run before it is sent.",
        "Check the price, then confirm to send it.",
        format!("It asks because {}.", why.join(" and ")),
    )
}

/// The run would take today's cloud spending past the daily limit.
pub fn daily_limit(spent: f64, limit: f64) -> AppError {
    AppError::new(
        "studio_daily_limit",
        format!(
            "Daily limit reached: {} of {} spent today",
            usd(spent),
            usd_short(limit)
        ),
        "Raise the limit in Settings → Studio, or wait until tomorrow.",
        String::new(),
    )
}

/// A run whose price is unknown counts as [`spend::UNKNOWN_PRICE_RESERVE_USD`], and less than
/// that is left of today's limit `limit` with `spent` spent. The same code as
/// [`daily_limit`]: it is the daily limit that refuses it.
pub fn unknown_price_needs_reserve(spent: f64, limit: f64) -> AppError {
    AppError::new(
        "studio_daily_limit",
        format!(
            "This model's price is unknown, so Studio needs {} of today's limit left ({} left).",
            usd_short(spend::UNKNOWN_PRICE_RESERVE_USD),
            usd(limit - spent)
        ),
        "Raise the limit in Settings → Studio, or pick a model whose price is shown.",
        String::new(),
    )
}

/// The endpoint is gone, makes no pictures or video, or its saved lists do not have the model.
pub fn no_endpoint() -> AppError {
    AppError::new(
        "studio_no_endpoint",
        "This endpoint or model is no longer available.",
        "Pick another model, or get the model list again.",
        String::new(),
    )
}

/// The endpoint named `name` has no API key saved.
pub fn no_key(name: &str) -> AppError {
    AppError::new(
        "studio_no_key",
        format!("{name} has no API key."),
        "Add the key in Settings → Endpoints.",
        String::new(),
    )
}

/// Fetching an endpoint's model lists or a picture's price got no answer: the connection
/// never opened, timed out or was cut. Nothing is charged for either, and the lists saved
/// before are kept.
pub fn not_fetched(company: Company) -> AppError {
    if company == Company::Local {
        return AppError::new(
            "studio_not_fetched",
            ENGINE_DID_NOT_ANSWER,
            TRY_AGAIN_OR_RESTART,
            String::new(),
        );
    }
    AppError::new(
        "studio_not_fetched",
        format!(
            "{} could not be reached, or did not answer in time.",
            name(company)
        ),
        "Check your internet connection, then try again.",
        String::new(),
    )
}

/// A company answered a fetch of an endpoint's model lists or a picture's price with a
/// failure instead of the list or price (one it could not find, or could not give). Nothing
/// is charged for either, and the lists saved before are kept.
pub fn not_given(company: Company) -> AppError {
    if company == Company::Local {
        return AppError::new(
            "studio_not_fetched",
            "The Studio engine on this computer could not give its model list.",
            TRY_AGAIN_OR_RESTART,
            String::new(),
        );
    }
    AppError::new(
        "studio_not_fetched",
        format!("{} could not give the model list or price.", name(company)),
        "Try again later. If it keeps happening, copy the details into a bug report.",
        String::new(),
    )
}

/// A company's answered failure on a fetch of an endpoint's model lists or a picture's price:
/// a list or price it could not find or give is [`not_given`], never a run that could not be
/// made; the key, credit, rate and server refusals are worded as [`from_failure`] words them.
/// The company's own message, code and request id go in the details.
pub fn from_read_failure(company: Company, f: &MediaFailure) -> AppError {
    match f.kind {
        FailureKind::NotFound | FailureKind::Other => AppError {
            details: failure_details(f),
            ..not_given(company)
        },
        _ => from_failure(company, f),
    }
}

/// The model lists or a price that were fetched could not be saved on this computer; the
/// lists saved before are kept.
pub fn lists_not_saved(details: impl Into<String>) -> AppError {
    AppError::new(
        "studio_not_saved",
        "The model list or price could not be saved on this computer.",
        TRY_AGAIN_OR_RESTART,
        details,
    )
}

/// The request's kind is not the model's: a video model asked for pictures, or the other way.
pub fn wrong_kind(model_kind: MediaKind) -> AppError {
    let message = match model_kind {
        MediaKind::Video => "This model makes videos, not pictures.",
        MediaKind::Image => "This model makes pictures, not videos.",
    };
    AppError::new(
        "bad_request",
        message,
        "Pick the model again.",
        String::new(),
    )
}

/// Studio keeps [`store::MAX_CREATION_RUNS`] runs and refuses one more rather than delete old
/// results.
pub fn gallery_full() -> AppError {
    AppError::new(
        "studio_gallery_full",
        format!(
            "Studio keeps at most {} runs, and that many are kept.",
            grouped(store::MAX_CREATION_RUNS as u64)
        ),
        "Delete some results in the gallery to make room.",
        String::new(),
    )
}

/// A run on this computer, before the local models are set up.
pub fn no_local_engine() -> AppError {
    AppError::new(
        "studio_no_local_engine",
        "Local models are not set up on this computer.",
        "Open Get local models to set them up.",
        String::new(),
    )
}

/// Models → Import: the file is a picture or video model (its architecture or its tensor
/// table says so), which Studio adds, never the chat list.
pub fn picture_model() -> AppError {
    AppError::new(
        "picture_model",
        "This is a picture or video model.",
        "Add it in Studio → Get local models.",
        String::new(),
    )
}

/// The settings `spec::validate` refused (or an extra setting no routed company takes), each
/// in a sentence that names the field by its label.
pub fn invalid_settings(problems: &[Invalid], model: &MediaModel) -> AppError {
    AppError::new(
        "studio_bad_settings",
        invalid_sentences(problems, model).join(" "),
        "Change the settings and try again.",
        String::new(),
    )
}

/// One plain sentence per problem, for [`invalid_settings`] and the estimate's `invalid`.
pub fn invalid_sentences(problems: &[Invalid], model: &MediaModel) -> Vec<String> {
    problems
        .iter()
        .map(|problem| match problem {
            Invalid::NotOffered(key) => {
                format!("{} is not offered by this model.", label(model, key))
            }
            Invalid::BadValue(key) => {
                format!(
                    "{} has a value this model does not take.",
                    label(model, key)
                )
            }
            Invalid::Requires { key, needs } => format!(
                "{} needs {} set first.",
                label(model, key),
                label(model, needs)
            ),
            Invalid::TooManyPictures { role, max: 0 } => {
                format!("This model takes no {}.", pictures(*role, 2))
            }
            Invalid::TooManyPictures { role, max } => {
                format!("This model takes at most {max} {}.", pictures(*role, *max))
            }
            Invalid::TooFewPictures { role, min } => {
                format!("This model needs at least {min} {}.", pictures(*role, *min))
            }
            Invalid::Reserved(key) => {
                format!("\"{key}\" cannot be set under Extra fields: it has its own place.")
            }
            Invalid::EmptyPrompt => "Write a prompt first.".to_owned(),
            Invalid::PromptTooLong => "Keep the prompt under 32,000 characters.".to_owned(),
            Invalid::NotUsable(reason) => reason.clone(),
        })
        .collect()
}

/// The names Studio shows for settings a model may not list itself, as its other models
/// label them.
const LABELS: [(&str, &str); 14] = [
    ("aspect_ratio", "Shape"),
    ("n", "How many"),
    ("quality", "Quality"),
    ("resolution", "Resolution"),
    ("duration", "Length"),
    ("generate_audio", "Sound"),
    ("seed", "Seed"),
    ("size", "Exact size"),
    ("output_format", "File type"),
    ("output_compression", "Compression"),
    ("background", "Background"),
    ("frame_count", "Frames"),
    ("frame_duration_ms", "Time per frame"),
    ("moderation", "Safety level"),
];

/// A setting's name on screen: the model's own label for it, else the label Studio gives
/// that setting elsewhere, else the key as typed, in quotes (an Extra field).
fn label(model: &MediaModel, key: &str) -> String {
    model
        .params
        .iter()
        .find(|p| p.key == key)
        .map(|p| p.label.clone())
        .or_else(|| {
            LABELS
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, l)| (*l).to_owned())
        })
        .unwrap_or_else(|| format!("\"{key}\""))
}

/// Added pictures of one role, `count` of them.
fn pictures(role: RefRole, count: u32) -> &'static str {
    match (role, count == 1) {
        (RefRole::Reference, true) => "reference picture",
        (RefRole::Reference, false) => "reference pictures",
        (RefRole::StartFrame, true) => "start frame",
        (RefRole::StartFrame, false) => "start frames",
        (RefRole::EndFrame, true) => "end frame",
        (RefRole::EndFrame, false) => "end frames",
    }
}

// ---- how a run ended --------------------------------------------------------------------------

/// The company's safety filter refused the run, at `stage` when the company says.
pub fn blocked(
    company: Company,
    stage: Option<Stage>,
    charged: Option<bool>,
    request_id: Option<&str>,
) -> AppError {
    let details = request_id.map_or_else(String::new, |id| format!("Request id: {id}"));
    if company == Company::Local {
        // A run on this computer costs nothing, whatever `charged` says.
        let message = match stage {
            Some(Stage::Prompt) => "Studio refused the prompt.",
            Some(Stage::Output) => "Studio stopped the picture after it was made.",
            None => "Studio refused this.",
        };
        return AppError::new(
            "studio_blocked",
            message,
            "Nothing was charged. Change the prompt or the added pictures if you want to try again.",
            details,
        );
    }
    let who = name(company);
    let message = match stage {
        Some(Stage::Prompt) => format!("{who}'s safety filter refused the prompt."),
        Some(Stage::Output) => {
            format!("{who}'s safety filter stopped the picture after it was made.")
        }
        None => format!("{who}'s safety filter refused this."),
    };
    AppError::new(
        "studio_blocked",
        message,
        format!(
            "{} Change the prompt or the added pictures if you want to try again.",
            charge_line(charged)
        ),
        details,
    )
}

/// OpenAI needs the organization verified before it makes pictures with this model.
pub fn org_verification() -> AppError {
    AppError::new(
        "studio_org_verification",
        "OpenAI needs your organization verified before it makes pictures with this model.",
        "Verify your organization in your settings on platform.openai.com, then try again.",
        String::new(),
    )
}

/// The company no longer knows a job Studio was following.
pub fn job_lost(company: Company) -> AppError {
    if company == Company::Local {
        return AppError::new(
            "studio_job_lost",
            "The Studio engine on this computer no longer has this job.",
            "Make it again if you still want it.",
            String::new(),
        );
    }
    AppError::new(
        "studio_job_lost",
        format!("{} no longer has this job.", name(company)),
        format!(
            "Check your activity page on {}, then make it again if you still want it.",
            activity_page(company)
        ),
        String::new(),
    )
}

/// The app closed while the run waited for its place in the queue: it was never sent.
pub fn closed_before_its_turn(company: Company) -> AppError {
    if company == Company::Local {
        return AppError::new(
            "studio_not_sent",
            "Not made: the app closed before its turn. Nothing was charged.",
            "Make it again with Again.",
            String::new(),
        );
    }
    AppError::new(
        "studio_not_sent",
        "Not sent: the app closed before its turn. Nothing was charged.",
        format!("Make it again with Again to send it to {}.", name(company)),
        String::new(),
    )
}

/// *Get it anyway* or *Check again* on a run with no company job to check: a run on this
/// computer, a picture, or a run that never got a job id.
pub fn nothing_to_fetch() -> AppError {
    AppError::new(
        "bad_request",
        "There is no result to get for this run.",
        "Make it again with Again.",
        String::new(),
    )
}

/// The app closed while the run was being sent (cloud) or made (this computer), charged as
/// `charged` says: a run on this computer never; an OpenRouter picture that never arrived
/// not (`Some(false)`: OpenRouter bills only a picture it delivered); else unknown.
pub fn interrupted(company: Company, charged: Option<bool>) -> AppError {
    match (company, charged) {
        (Company::Local, _) => AppError::new(
            "studio_interrupted",
            "The app closed while this was being made.",
            "Nothing was charged. Make it again if you still want it.",
            String::new(),
        ),
        // OpenRouter bills only a picture it delivered.
        (Company::OpenRouter, Some(false)) => AppError::new(
            "studio_interrupted",
            "The app closed while sending.",
            "OpenRouter does not charge for a picture you did not receive. Make it again if \
             you still want it.",
            String::new(),
        ),
        (_, Some(false)) => AppError::new(
            "studio_interrupted",
            "The app closed while sending.",
            "Nothing was charged. Make it again if you still want it.",
            String::new(),
        ),
        _ => AppError::new(
            "studio_interrupted",
            "The app closed while sending; it may or may not have started.",
            format!("Check your activity page on {}.", activity_page(company)),
            String::new(),
        ),
    }
}

/// A run's task ended on an error inside the app (a bug) before it recorded how the run went.
/// As after a restart ([`interrupted`]), Studio cannot know whether a cloud run started.
pub fn broke_off(company: Company) -> AppError {
    match company {
        Company::Local => AppError::new(
            "studio_interrupted",
            "Studio stopped this after an error in the app.",
            "Nothing was charged. Try again; if it keeps happening, restart the app.",
            String::new(),
        ),
        _ => AppError::new(
            "studio_interrupted",
            "Studio stopped following this after an error in the app; it may or may not have started.",
            format!(
                "It may still be charged: check your activity page on {} before you make it again.",
                activity_page(company)
            ),
            String::new(),
        ),
    }
}

/// The company no longer has the finished result.
pub fn result_expired(company: Company) -> AppError {
    if company == Company::Local {
        return AppError::new(
            "studio_result_expired",
            "The result is no longer on this computer.",
            "Make it again if you still want it.",
            String::new(),
        );
    }
    AppError::new(
        "studio_result_expired",
        format!("{} no longer has the result.", name(company)),
        "Make it again if you still want it.",
        String::new(),
    )
}

/// The disk filled up while a result was written.
pub fn disk_full() -> AppError {
    AppError::new(
        "studio_disk_full",
        "The disk is full, so the result was not saved.",
        "Free some space on the disk, then try again.",
        String::new(),
    )
}

/// A result was larger than its cap of `cap` bytes.
pub fn too_large(cap: u64) -> AppError {
    AppError::new(
        "studio_too_large",
        format!(
            "The result is larger than {}, so it was not kept.",
            size(cap)
        ),
        "Choose a smaller size or a shorter length, then try again.",
        String::new(),
    )
}

/// The request never left this computer, or the connection never opened.
pub fn not_sent(company: Company) -> AppError {
    if company == Company::Local {
        return AppError::new(
            "studio_not_sent",
            "Not started. Nothing was charged.",
            TRY_AGAIN_OR_RESTART,
            String::new(),
        );
    }
    AppError::new(
        "studio_not_sent",
        "Not sent. Nothing was charged.",
        format!(
            "Check your internet connection and that {} is reachable, then try again.",
            name(company)
        ),
        String::new(),
    )
}

/// The request was sent but no answer came: a timeout or a cut connection.
pub fn not_answered(company: Company, charged: Option<bool>) -> AppError {
    if company == Company::Local {
        return AppError::new(
            "studio_not_answered",
            ENGINE_DID_NOT_ANSWER,
            TRY_AGAIN_OR_RESTART,
            String::new(),
        );
    }
    let next = match (company, charged) {
        (Company::OpenRouter, Some(false)) => {
            "OpenRouter does not charge for a picture you did not receive. Try again.".to_owned()
        }
        (_, Some(false)) => "Nothing was charged. Try again.".to_owned(),
        _ => format!(
            "It may still be charged: check your activity page on {} before you try again.",
            activity_page(company)
        ),
    };
    AppError::new(
        "studio_not_answered",
        format!(
            "{} did not answer in time, or the connection was cut.",
            name(company)
        ),
        next,
        String::new(),
    )
}

/// The user stopped the run: Cancel on a picture, Stop watching on a video, Stop on a run on
/// this computer.
pub fn stopped(company: Company, kind: MediaKind, charged: Option<bool>) -> AppError {
    let who = name(company);
    let (message, next) = match (company, kind, charged) {
        (Company::Local, _, _) => (
            "Stopped. Your computer is no longer working on it. Nothing was charged.".to_owned(),
            "Make it again if you still want it.",
        ),
        (Company::OpenRouter, MediaKind::Image, Some(false)) => (
            "Stopped. OpenRouter does not charge for a picture you did not receive.".to_owned(),
            "Make it again if you still want it.",
        ),
        (_, _, Some(false)) => (
            "Stopped. Nothing was charged.".to_owned(),
            "Make it again if you still want it.",
        ),
        (_, MediaKind::Image, _) => (
            format!("Stopped waiting. {who} may still charge for it."),
            "Make it again if you still want it.",
        ),
        // Stopped while its result was downloading: the job had finished.
        (_, MediaKind::Video, Some(true)) => (
            format!("Stopped. {who} finished this video, so it was charged."),
            "Press Get it anyway to download it.",
        ),
        (_, MediaKind::Video, _) => (
            format!("{who} may still finish and charge for this."),
            "Press Get it anyway to download it if it finished.",
        ),
    };
    AppError::new("studio_stopped", message, next, String::new())
}

/// The user stopped a video while its request was out, before the company gave it a job id:
/// there is no job to get later (so no *Get it anyway*), and Studio cannot know whether it
/// started.
pub fn stopped_while_sending(company: Company) -> AppError {
    if company == Company::Local {
        return stopped(company, MediaKind::Video, Some(false));
    }
    AppError::new(
        "studio_stopped",
        "Stopped while sending; it may or may not have started.",
        format!(
            "It may still be charged: check your activity page on {} before you make it again.",
            activity_page(company)
        ),
        String::new(),
    )
}

/// The finished result could not be downloaded.
pub fn download_failed(company: Company) -> AppError {
    if company == Company::Local {
        return AppError::new(
            "studio_download_failed",
            "The Studio engine's result could not be read.",
            TRY_AGAIN_OR_RESTART,
            String::new(),
        );
    }
    AppError::new(
        "studio_download_failed",
        format!("The result could not be downloaded from {}.", name(company)),
        "Press Get it anyway to try again.",
        String::new(),
    )
}

// ---- showing results --------------------------------------------------------------------------

/// A result over [`super::commands::SHOW_CAP`] is not played in the viewer: it would sit whole in
/// memory four times over (in the app, the copy to the webview, its buffer and its blob).
/// Only a video can be that large.
pub fn too_big_to_show() -> AppError {
    AppError::new(
        "studio_too_big_to_show",
        "This video is too big to play here.",
        "Use Show in folder to open it with your video player.",
        String::new(),
    )
}

/// The name asked for is not an added picture's (a SHA-256), so nothing is looked up.
pub fn bad_ref_name() -> AppError {
    AppError::new(
        "bad_request",
        "That is not the name of an added picture.",
        "Add the picture again.",
        String::new(),
    )
}

/// An added picture the run would send is no longer in the gallery folder (it went with the
/// last run that used it, or was removed by hand), so nothing is sent.
pub fn added_picture_missing() -> AppError {
    AppError::new(
        "studio_ref_missing",
        "One of the added pictures is no longer on this computer.",
        "Add the picture again.",
        String::new(),
    )
}

// ---- what the gallery's actions refuse --------------------------------------------------------

/// Where a Studio picture is going.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PictureUse {
    /// Into a chat's message box (*Send to chat*).
    Chat,
    /// Into the prompt, as an added picture (*Use in prompt*).
    Prompt,
}

/// A result that is not a PNG, JPEG, GIF or WebP picture (a video, or an SVG picture) cannot
/// go where `to` says.
pub fn not_a_picture_for(to: PictureUse, video: bool) -> AppError {
    let (message, next) = match (to, video) {
        (PictureUse::Chat, true) => (
            "A video cannot go into a chat: chat models do not take video.",
            "Send a picture instead.",
        ),
        (PictureUse::Chat, false) => (
            "Only PNG, JPEG, GIF and WebP pictures can go into a chat.",
            "Send a picture of one of those types instead.",
        ),
        (PictureUse::Prompt, true) => (
            "A video cannot be added to a prompt as a picture.",
            "Add a picture instead.",
        ),
        (PictureUse::Prompt, false) => (
            "Only PNG, JPEG, GIF and WebP pictures can be added to a prompt.",
            "Add a picture of one of those types instead.",
        ),
    };
    AppError::new("bad_request", message, next, String::new())
}

/// A picture over the chat's limit ([`crate::attachments::MAX_IMAGE_BYTES`], which added
/// pictures share) cannot go where `to` says.
pub fn picture_too_big_for(to: PictureUse) -> AppError {
    let cap = size(crate::attachments::MAX_IMAGE_BYTES as u64);
    let (message, next) = match to {
        PictureUse::Chat => (
            format!("This picture is over {cap}, so it cannot go into a chat."),
            "Make a smaller copy in another app, then attach that.",
        ),
        PictureUse::Prompt => (
            format!("This picture is over {cap}, so it cannot be added to a prompt."),
            "Make a smaller copy in another app, then add that.",
        ),
    };
    AppError::new("bad_request", message, next, String::new())
}

/// A video's poster frame from the webview is not a JPEG picture of at most
/// [`super::actions::POSTER_CAP`] bytes.
pub fn bad_poster() -> AppError {
    AppError::new(
        "bad_request",
        format!(
            "The poster frame was not saved: it must be a JPEG picture of at most {}.",
            size(super::actions::POSTER_CAP as u64)
        ),
        "Play the video again to make a new one.",
        String::new(),
    )
}

/// The file chosen for an export is the result's own file (reached through a link), which a
/// copy would empty.
pub fn export_onto_itself() -> AppError {
    AppError::new(
        "bad_request",
        "That file is the result itself.",
        "Choose another name or folder.",
        String::new(),
    )
}

/// The exported copy, or its settings beside it, could not be written where the user chose.
pub fn not_exported(details: impl Into<String>) -> AppError {
    AppError::new(
        "export_failed",
        "The file could not be saved.",
        "Choose another folder.",
        details,
    )
}

/// An export with its settings would write `file_name` (`<name>.json`) beside the copy, and
/// a file of that name is already there: the save dialog asked only about the copy, so it
/// is not replaced, and nothing is copied.
pub fn settings_file_exists(file_name: &str) -> AppError {
    AppError::new(
        "bad_request",
        format!("A file named {file_name} is already there."),
        "Choose another name.",
        String::new(),
    )
}

/// The file manager could not be asked to open `folder` (*Show in folder*, *Open folder*);
/// the user is given the folder to open themselves.
pub fn not_opened(folder: &std::path::Path, details: impl Into<String>) -> AppError {
    AppError::new(
        "studio_not_opened",
        "The folder could not be opened.",
        format!("Open it yourself: {}", folder.display()),
        details,
    )
}

/// A poster frame was sent for a result that is not a video: a picture's thumbnail is made
/// from the picture itself.
pub fn poster_only_for_video() -> AppError {
    AppError::new(
        "bad_request",
        "Only a video can have a poster frame saved.",
        "Play a video to save its poster frame.",
        String::new(),
    )
}

// ---- the Studio engine ------------------------------------------------------------------------

/// The Windows engine pack's download, asked for without its licence accepted.
pub fn engine_licence_not_accepted() -> AppError {
    AppError::new(
        "bad_request",
        "Accept the licence to download the Studio engine.",
        "Read the licence shown with the download, then accept it.",
        String::new(),
    )
}

/// The Windows engine pack could not be downloaded or installed: a backend download's words
/// (`packs_cmd`), with the Studio engine named where those name the backend. The codes are the
/// same as a backend download's.
pub fn engine_not_installed(e: &PackError) -> AppError {
    let (code, message, next) = match e {
        PackError::ChecksumMismatch { .. } | PackError::SizeMismatch { .. } => (
            "checksum_mismatch",
            "The downloaded Studio engine does not match its pinned checksum, so it was not \
             installed.",
            "Try again. If it keeps failing, report it: the published file may have changed.",
        ),
        PackError::Network { .. } => (
            "network",
            "The Studio engine download was interrupted.",
            "Check your connection and try again; it continues where it stopped.",
        ),
        PackError::DiskFull => (
            "disk_full",
            "The disk is full.",
            "Free some space and try again.",
        ),
        PackError::BlockedHost(_) => (
            "blocked_host",
            "The download was redirected to a host the app does not trust.",
            "Report this; GitHub may have changed its download servers.",
        ),
        _ => (
            "pack_failed",
            "The Studio engine could not be installed.",
            "Try again, or copy the details into a bug report.",
        ),
    };
    AppError::new(code, message, next, e.to_string())
}

/// *Remove* while the Studio engine is still downloading into the folder it would empty.
pub fn engine_still_downloading() -> AppError {
    AppError::new(
        "bad_request",
        "The Studio engine is still downloading.",
        "Cancel the download first, then remove it.",
        String::new(),
    )
}

/// The Studio engine's folder could not be emptied.
pub fn engine_not_removed(details: impl Into<String>) -> AppError {
    AppError::new(
        "pack_failed",
        "The Studio engine could not be removed.",
        TRY_AGAIN_OR_RESTART,
        details,
    )
}

/// What the Studio engine's licence allows, shown with a local model's licences when the
/// engine downloads with it (its parts are MIT and BSD-3-Clause, which limit nothing made with
/// it).
pub const ENGINE_LICENCE_WORDS: &str = "Free for any use, including commercial.";

// ---- local models ------------------------------------------------------------------------------

/// The code of [`not_that_model`], which a download group shows as it is (its files are gone,
/// so Resume would download them again).
pub const NOT_THAT_MODEL: &str = "studio_not_that_model";

/// Decimal gigabytes with one decimal, rounded up, as download sizes are shown: what a
/// download needs is never understated.
fn gb_up(bytes: u64) -> String {
    let tenths = bytes.div_ceil(100_000_000);
    format!("{}.{} GB", tenths / 10, tenths % 10)
}

/// A downloaded diffusion file whose own tensor table is not its family's (a FLUX.2 klein 9B
/// named like the 4B, or a file whose name lies): it is not added, and its files go.
pub fn not_that_model(details: impl Into<String>) -> AppError {
    AppError::new(
        NOT_THAT_MODEL,
        "This file is not the model its name says, so it was not added.",
        "Choose another file, or a model from the recommended list.",
        details,
    )
}

/// A local download before the note about local models was accepted.
pub fn notice_needed() -> AppError {
    AppError::new(
        "studio_notice_needed",
        "Read the note about local models first.",
        "Open the note in Get local models.",
        String::new(),
    )
}

/// A local download with a licence of one of its parts not accepted.
pub fn license_not_accepted() -> AppError {
    AppError::new(
        "studio_license_not_accepted",
        "Accept the licences to download this model.",
        "Tick each licence, then Download.",
        String::new(),
    )
}

/// A local download that needs more than the free space on the models folder's disk;
/// `needed` is what it would fetch.
pub fn disk_too_small(needed: u64) -> AppError {
    AppError::new(
        "studio_disk_too_small",
        "There is not enough free space for this model.",
        format!(
            "It needs {}; free up space or choose another models folder.",
            gb_up(needed)
        ),
        String::new(),
    )
}

/// A model harnessUI cannot run: a searched repository of a family it does not recognise, or
/// a catalog entry this build hides. `why` (the sentence search shows, or the catalog's
/// reason) goes in the details.
pub fn not_supported_yet(why: impl Into<String>) -> AppError {
    AppError::new(
        "studio_not_supported_yet",
        "harnessUI cannot run this model yet.",
        "Choose a model from the recommended list.",
        why,
    )
}

/// Why harnessUI never offers a repository, as a plain clause: what `studio_not_offered`
/// says after "harnessUI does not offer this model:", and what Get local models shows after
/// "Not offered in harnessUI:".
pub fn not_offered_reason(why: NotOffered) -> &'static str {
    match why {
        NotOffered::RegionLimited => "its licence excludes some regions",
        NotOffered::Uncensored => "it was made without safety limits",
        NotOffered::AdultContent => "it is marked as not for all audiences",
    }
}

/// A repository harnessUI never offers, whatever it holds, with the reason.
pub fn not_offered(why: NotOffered) -> AppError {
    let reason = not_offered_reason(why);
    AppError::new(
        "studio_not_offered",
        format!("harnessUI does not offer this model: {reason}."),
        "Choose another model.",
        String::new(),
    )
}

/// A picture or video model's download whose parts are not all downloaded and checked yet.
pub fn model_incomplete() -> AppError {
    AppError::new(
        "studio_model_incomplete",
        "A part of this model is not downloaded yet.",
        "Resume the download in Models → Downloads to finish it.",
        String::new(),
    )
}

/// A local prompt the prompt check refuses ([`super::prompt_check::refuses`]): it pairs a
/// sexual word with a word for children. It is never retried and there is no way round it.
pub fn wont_make() -> AppError {
    AppError::new(
        "studio_wont_make",
        "Studio won't make this.",
        "Change the prompt.",
        String::new(),
    )
}

// ---- a run on this computer: refusals ------------------------------------------------------

/// A local prompt over `runtime::sd::args::PROMPT_MAX_CHARS`, as the estimate lists it among
/// the settings the run would refuse.
pub const PROMPT_TOO_LONG_HERE: &str = "Keep the prompt under 4,000 characters for local models.";

/// A local prompt over 4,000 characters, refused before anything is recorded.
pub fn prompt_too_long_here() -> AppError {
    AppError::new(
        "studio_prompt_too_long_here",
        PROMPT_TOO_LONG_HERE,
        "Shorten it, or choose a cloud model.",
        String::new(),
    )
}

/// A local run that does not fit this computer even with the chat model unloaded (and is not
/// a picture the processor can make), with why when it is known ([`not_here_reason`]).
pub fn does_not_fit(reason: Option<String>) -> AppError {
    AppError::new(
        "studio_does_not_fit",
        "This model does not fit this computer.",
        "Choose a smaller size or a cloud model.",
        reason.unwrap_or_default(),
    )
}

/// *Restart and update* while a local job works: the restart would stop it without asking.
pub fn busy_for_update() -> AppError {
    AppError::new(
        "studio_busy_for_update",
        "A picture or video is being made on this computer.",
        "Stop it in Studio, or install the update when it finishes.",
        String::new(),
    )
}

/// *Remove* on the Studio engine while a local job works with it.
pub fn engine_in_use() -> AppError {
    AppError::new(
        "studio_engine_in_use",
        "The Studio engine is making a picture or video now.",
        "Stop it in Studio, or remove the engine when it finishes.",
        String::new(),
    )
}

/// A local run of a model that is not installed on this computer (any more).
pub fn local_model_missing() -> AppError {
    AppError::new(
        "studio_local_model_missing",
        LOCAL_NOT_INSTALLED,
        "Choose another model, or get it again in Get local models.",
        String::new(),
    )
}

// ---- Get local models ------------------------------------------------------------------------

/// *Delete* on a local model while a run on this computer waits for it or uses it: deleting a
/// file the Studio engine has open fails half-way on Windows.
pub fn model_in_use() -> AppError {
    AppError::new(
        "studio_model_in_use",
        "A run on this computer is using this model.",
        "Stop the run, then delete it.",
        String::new(),
    )
}

/// Why a recognised family's model is not supported yet when this build has no record of the
/// family (a catalog newer than the build, or one that hid the record).
pub const FAMILY_RECORD_MISSING: &str = "its family record is missing";

/// Why a recognised repository offers no file: each of its model files was refused when its
/// entry or its download was worked out (a part not pinned, a licence not known).
pub const NO_FILE_OFFERED: &str = "None of this repository's model files can be downloaded here.";

/// Why an installed model that this build cannot read is not for this computer (its family
/// record hidden by a newer signed catalog, a part's licence no longer known, a family this
/// build does not know); it is still listed, so it can be deleted.
pub const NOT_RUN_BY_THIS_VERSION: &str = "This version of harnessUI cannot run this model.";

/// What the part that makes the picture (or the clip) is, in the download dialog.
pub const PART_MAKES_THE_PICTURE: &str = "makes the picture";
/// What a text encoder is, in the download dialog.
pub const PART_READS_THE_PROMPT: &str = "reads your prompt";
/// What a CLIP-vision encoder is, in the download dialog: it reads an added picture.
pub const PART_READS_THE_PICTURE: &str = "reads the picture you add";
/// What the decoder is, in the download dialog.
pub const PART_PICTURE_DECODER: &str = "picture decoder";

/// A fit asked at a size the model is never made at (a side of 0, or longer than its
/// longest).
pub fn size_not_offered() -> AppError {
    AppError::new(
        "bad_request",
        "This model is not made at that size.",
        "Choose one of the sizes offered.",
        String::new(),
    )
}

/// The largest preset size that fits when the size asked for does not: "Fits at 768×768", or
/// for a clip "Fits at 832×480, 49 frames".
pub fn fits_at(size: GenSize, clip: bool) -> String {
    let GenSize {
        width,
        height,
        frames,
    } = size;
    if clip {
        format!("Fits at {width}×{height}, {frames} frames")
    } else {
        format!("Fits at {width}×{height}")
    }
}

/// Why a model is not for this computer, judged with the chat model's memory counted back:
/// "needs 6 GB of graphics memory" (the need rounded up to whole GB; "of free memory" on a
/// computer that runs models on the processor, `processor_only`), "needs a Mac with 32 GB or
/// more", "video needs a graphics card or an Apple-silicon Mac", "local video needs a graphics
/// card on this computer".
pub fn not_here_reason(not_for: NotFor, processor_only: bool) -> String {
    let gb = |bytes: u64| bytes.div_ceil(1_000_000_000);
    match not_for {
        NotFor::GraphicsMemory { needs } if processor_only => {
            format!("needs {} GB of free memory", gb(needs))
        }
        NotFor::GraphicsMemory { needs } => format!("needs {} GB of graphics memory", gb(needs)),
        NotFor::MacMemory { needs } => format!("needs a Mac with {} GB or more", gb(needs)),
        NotFor::VideoNeedsGpu => "video needs a graphics card or an Apple-silicon Mac".to_owned(),
        NotFor::VideoNotOnThisMac => {
            "local video needs a graphics card on this computer".to_owned()
        }
    }
}

// ---- a run on this computer: what its failure's details say -----------------------------------
//
// A local run's failure carries one of these as its message (`MediaFailure::message`), which
// [`from_failure`] puts in the details; the message and next step on screen come from the
// failure's kind.

/// No Studio engine is found on this computer (Windows before its pack is installed).
pub const LOCAL_ENGINE_NOT_FOUND: &str = "The Studio engine is not on this computer.";
/// The model asked for is not among the installed ones.
pub const LOCAL_NOT_INSTALLED: &str = "This model is not installed on this computer.";
/// A request for a picture from a video model, or the other way round.
pub const LOCAL_WRONG_KIND: &str = "This model does not make what was asked for.";
/// A part file of the model is gone.
pub const LOCAL_PART_MISSING: &str = "A file of this model is missing.";
/// A part file is not the size it was downloaded at.
pub const LOCAL_PART_CHANGED: &str = "A file of this model is not the size it was downloaded at.";
/// The diffusion file's own tensor table is not its family's; the check's details follow.
pub const LOCAL_NOT_ITS_MODEL: &str = "The model file is not the model it was downloaded as";
/// A prompt over `runtime::sd::args::PROMPT_MAX_CHARS`.
pub const LOCAL_PROMPT_TOO_LONG: &str = "The prompt is longer than 4,000 characters.";
/// A negative prompt over `runtime::sd::args::PROMPT_MAX_CHARS`.
pub const LOCAL_NEGATIVE_TOO_LONG: &str = "The negative prompt is longer than 4,000 characters.";
/// A run started again with the folder of one still running.
pub const LOCAL_ALREADY_RUNNING: &str = "This run is already running.";
/// A result asked for that the run did not make, or not yet.
pub const LOCAL_NO_SUCH_RESULT: &str = "This run has no such result.";

/// A setting whose value the engine job cannot take (the check before the run refuses it
/// first); the setting's key is named.
pub fn local_bad_value(key: &str) -> String {
    format!("The value of {key} is not one this model takes.")
}

/// The engine's arguments refused the job: their own sentence (the prompt's length, a size,
/// a `%d` in the gallery folder's path).
pub fn local_args_refused(e: &ArgsError) -> String {
    let text = e.to_string();
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => format!("{}{}.", first.to_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

/// What the Studio engine did when a run of `kind` failed. An engine that exits 0 without a
/// file finished without making it; it did not crash.
pub fn engine_failure_details(f: &SdFailure, kind: MediaKind) -> String {
    let result = match kind {
        MediaKind::Image => "a picture",
        MediaKind::Video => "a video",
    };
    match f {
        SdFailure::OutOfMemory => "The Studio engine ran out of memory.".to_owned(),
        SdFailure::DeviceLost => "The graphics driver stopped responding.".to_owned(),
        SdFailure::MissingLibrary(name) if name.is_empty() => {
            "The Studio engine could not load a library it needs.".to_owned()
        }
        SdFailure::MissingLibrary(name) => {
            format!("The Studio engine could not load a library it needs: {name}.")
        }
        SdFailure::BadModel => "The Studio engine could not read a model file.".to_owned(),
        SdFailure::EngineMissing => LOCAL_ENGINE_NOT_FOUND.to_owned(),
        SdFailure::EngineBlocked(why) => {
            format!("The system did not let the Studio engine start: {why}.")
        }
        SdFailure::Crashed { code: Some(0) } => {
            format!("The Studio engine finished without making {result}.")
        }
        SdFailure::Crashed { code: Some(code) } => {
            format!("The Studio engine stopped with exit code {code}.")
        }
        SdFailure::Crashed { code: None } => "The system stopped the Studio engine.".to_owned(),
        SdFailure::Cancelled => "The run was stopped.".to_owned(),
        SdFailure::Io(why) => format!("Studio could not use the run's folder: {why}."),
    }
}

/// A company's failure as the details show it: its own message, code, HTTP status and
/// request id, each when it gave one.
fn failure_details(f: &MediaFailure) -> String {
    let mut details: Vec<String> = Vec::new();
    if !f.message.is_empty() {
        details.push(f.message.clone());
    }
    if let Some(code) = &f.company_code {
        details.push(format!("Code: {code}"));
    }
    if let Some(status) = f.status {
        details.push(format!("HTTP status: {status}"));
    }
    if let Some(id) = &f.request_id {
        details.push(format!("Request id: {id}"));
    }
    details.join("; ")
}

/// What a company's (or the Studio engine's) failure says, in plain words, with the
/// company's own message, code and request id in the details.
pub fn from_failure(company: Company, f: &MediaFailure) -> AppError {
    let who = name(company);
    let details = failure_details(f);
    let with_details =
        |code: &str, message: String, next: String| AppError::new(code, message, next, &details);
    let simple = |code: &str, message: &str, next: &str| {
        with_details(code, message.to_owned(), next.to_owned())
    };
    // What only a company reports (a key, credit, its server): a run on this computer gets
    // the Studio engine's words instead, never a website or an account.
    let company_says = |code: &str, message: String, next: String| match company {
        Company::Local => simple(code, ENGINE_COULD_NOT, TRY_AGAIN_OR_RESTART),
        Company::OpenRouter | Company::OpenAi => with_details(code, message, next),
    };
    match f.kind {
        FailureKind::Blocked => blocked(company, f.stage, f.charged, f.request_id.as_deref()),
        FailureKind::OrgVerification if company == Company::Local => simple(
            "studio_org_verification",
            ENGINE_COULD_NOT,
            TRY_AGAIN_OR_RESTART,
        ),
        FailureKind::OrgVerification => org_verification(),
        FailureKind::Expired => result_expired(company),
        FailureKind::Unauthorized => company_says(
            "studio_unauthorized",
            format!("{who} refused the API key."),
            "Check the key in Settings → Endpoints.".to_owned(),
        ),
        FailureKind::NoCredit => company_says(
            "studio_no_credit",
            format!("Your {who} account has no credit left."),
            format!("Add credit on {}, then try again.", activity_page(company)),
        ),
        FailureKind::RateLimited => company_says(
            "studio_rate_limited",
            format!("{who} has had too many requests."),
            "Wait a minute, then try again.".to_owned(),
        ),
        FailureKind::BadRequest => company_says(
            "studio_refused",
            format!("{who} refused the request as it was."),
            "Change the settings and try again.".to_owned(),
        ),
        FailureKind::NotFound => company_says(
            "studio_not_found",
            format!("{who} could not find this model."),
            "Get the model list again, then pick a model.".to_owned(),
        ),
        FailureKind::Server => company_says(
            "studio_server_error",
            format!("{who}'s server failed."),
            format!("{} Try again in a few minutes.", charge_line(f.charged)),
        ),
        FailureKind::Other => company_says(
            "studio_failed",
            format!("{who} could not make it."),
            "Try again. If it keeps happening, copy the details into a bug report.".to_owned(),
        ),
        FailureKind::OutOfMemory => simple(
            "studio_out_of_memory",
            "Ran out of graphics memory.",
            "Try 480p or a shorter clip.",
        ),
        FailureKind::DeviceLost => simple(
            "studio_device_lost",
            "The graphics driver stopped responding.",
            "Try again. If it keeps happening, restart the computer.",
        ),
        FailureKind::ModelFileBad => simple(
            "studio_model_file_bad",
            "A model file is damaged.",
            "Download it again in Get local models.",
        ),
        FailureKind::EngineMissing => simple(
            "studio_engine_missing",
            "The Studio engine is missing or was blocked.",
            "Download it again, or allow it in your antivirus.",
        ),
        FailureKind::EngineFailed => simple(
            "studio_engine_failed",
            "The Studio engine stopped.",
            "Try again. If it keeps happening, copy the details into a bug report.",
        ),
    }
}

/// A failure writing a result on this computer: a full disk, a result over its cap of `cap`
/// bytes, or another write failure (its text goes to the log, not the screen).
pub fn from_local(e: &LocalIo, cap: u64) -> AppError {
    match e {
        LocalIo::DiskFull => disk_full(),
        LocalIo::TooLarge(_) => too_large(cap),
        LocalIo::Io(text) => {
            tracing::warn!("a Studio result could not be written: {text}");
            files()
        }
    }
}

/// A gallery failure in plain words. Its own text holds paths and hashes, so it goes to the
/// log only.
pub fn from_gallery(e: &GalleryError) -> AppError {
    match e {
        GalleryError::DiskFull => disk_full(),
        GalleryError::TooLarge(cap) => too_large(*cap),
        GalleryError::NotMedia | GalleryError::Mismatch { .. } => {
            tracing::warn!("a Studio result was not kept: {e}");
            not_media()
        }
        GalleryError::BadName | GalleryError::Io(_) => {
            tracing::warn!("a Studio file could not be reached: {e}");
            files()
        }
    }
}

/// A failed download of a result from a company's file host (through the guarded fetcher,
/// without the key), in Studio's words: the fetcher's own text speaks of web pages and names
/// hosts and addresses, so it goes to the log only.
pub fn from_fetch(company: Company, e: &FetchError) -> AppError {
    tracing::warn!("a Studio result was not downloaded from the company's file host: {e}");
    let details = match e {
        FetchError::DiskFull => return disk_full(),
        FetchError::TooLarge(cap) => return too_large(*cap),
        FetchError::NotMedia(_) => return not_media(),
        FetchError::Disk(_) => return files(),
        FetchError::Scheme(_)
        | FetchError::Credentials
        | FetchError::NoHost
        | FetchError::Blocked { .. }
        | FetchError::LocalName(_)
        | FetchError::BadRedirect(_) => {
            "The result link points to an address Studio never downloads from.".to_owned()
        }
        FetchError::TooManyRedirects => "The file host redirected too many times.".to_owned(),
        FetchError::Dns(_) => "The file host's address could not be found.".to_owned(),
        FetchError::Timeout(secs) => {
            format!("The file host did not send the whole file within {secs} seconds.")
        }
        FetchError::Status(status) => {
            format!("The file host answered with HTTP status {status}.")
        }
        FetchError::ContentType(_)
        | FetchError::Encoding(_)
        | FetchError::Challenge
        | FetchError::NoText => "The file host did not send the file as it is.".to_owned(),
        FetchError::Network(_) => "The connection to the file host failed.".to_owned(),
    };
    AppError {
        details,
        ..download_failed(company)
    }
}

/// A result that is none of the types Studio keeps.
fn not_media() -> AppError {
    AppError::new(
        "studio_not_media",
        "The result is not a picture or video Studio can keep.",
        "Try again, or pick another model.",
        String::new(),
    )
}

/// A file in the gallery folder could not be read or written.
fn files() -> AppError {
    AppError::new(
        "studio_files",
        "Studio could not reach a file in the gallery folder.",
        "Check that the gallery folder is there and the app may use it, then try again.",
        String::new(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent::media::{Choice, Layer, ParamKind, ParamSpec, Price};
    use search::netguard::AddressClass;

    fn model(params: Vec<ParamSpec>) -> MediaModel {
        MediaModel {
            id: "maker/model".into(),
            display_name: "Model".into(),
            family: "maker/model".into(),
            kind: MediaKind::Image,
            accepts_images: true,
            params,
            price: Price::Unknown,
            notes: Vec::new(),
            usable: None,
            created: None,
            companies: Vec::new(),
        }
    }

    fn quality() -> ParamSpec {
        ParamSpec {
            key: "quality".into(),
            canonical: None,
            label: "Picture quality".into(),
            help: None,
            layer: Layer::Simple,
            kind: ParamKind::Enum {
                values: vec![Choice {
                    value: "high".into(),
                    label: "High".into(),
                }],
            },
            company_default: None,
            initial: None,
            affects_price: true,
            requires: None,
        }
    }

    /// A failure of `kind` with nothing else said.
    fn failure(kind: FailureKind) -> MediaFailure {
        MediaFailure {
            kind,
            stage: None,
            charged: None,
            company_code: None,
            request_id: None,
            status: None,
            message: String::new(),
            retry_after: None,
        }
    }

    const EVERY_FAILURE_KIND: [FailureKind; 15] = [
        FailureKind::Blocked,
        FailureKind::Unauthorized,
        FailureKind::NoCredit,
        FailureKind::OrgVerification,
        FailureKind::RateLimited,
        FailureKind::BadRequest,
        FailureKind::NotFound,
        FailureKind::Expired,
        FailureKind::Server,
        FailureKind::Other,
        FailureKind::OutOfMemory,
        FailureKind::EngineMissing,
        FailureKind::EngineFailed,
        FailureKind::ModelFileBad,
        FailureKind::DeviceLost,
    ];

    /// Every constructor that takes a company, with `company`, in each case it words
    /// differently.
    fn with_company(company: Company) -> Vec<AppError> {
        let mut all = vec![
            not_fetched(company),
            not_given(company),
            blocked(company, None, None, None),
            blocked(company, Some(Stage::Prompt), Some(false), Some("req_1")),
            blocked(company, Some(Stage::Output), Some(true), None),
            job_lost(company),
            closed_before_its_turn(company),
            interrupted(company, None),
            interrupted(company, Some(false)),
            broke_off(company),
            result_expired(company),
            not_sent(company),
            stopped_while_sending(company),
            download_failed(company),
        ];
        for charged in [None, Some(false), Some(true)] {
            all.push(not_answered(company, charged));
            all.push(stopped(company, MediaKind::Image, charged));
            all.push(stopped(company, MediaKind::Video, charged));
        }
        all.extend(every_fetch_error().iter().map(|e| from_fetch(company, e)));
        all.extend(
            EVERY_FAILURE_KIND
                .iter()
                .map(|kind| from_failure(company, &failure(*kind))),
        );
        all.extend(
            EVERY_FAILURE_KIND
                .iter()
                .map(|kind| from_read_failure(company, &failure(*kind))),
        );
        all
    }

    /// Every constructor in this file, which `studio_errors_have_four_parts` checks. A new
    /// constructor is added here too (Tasks L11, L13 and L15 append theirs).
    fn every_constructor() -> Vec<AppError> {
        let mut all = vec![
            needs_confirmation(&[ConfirmReason::Video]),
            daily_limit(9.5, 10.0),
            unknown_price_needs_reserve(0.0, 3.0),
            no_endpoint(),
            no_key("My OpenRouter"),
            wrong_kind(MediaKind::Video),
            gallery_full(),
            no_local_engine(),
            picture_model(),
            invalid_settings(&[Invalid::EmptyPrompt], &model(Vec::new())),
            org_verification(),
            disk_full(),
            too_large(1024),
            from_local(&LocalIo::Io("denied".into()), 1),
            from_gallery(&GalleryError::NotMedia),
            from_gallery(&GalleryError::Io("/a/path".into())),
            gallery_not_full_path(),
            numbered_folder(),
            nothing_to_fetch(),
            too_big_to_show(),
            bad_ref_name(),
            added_picture_missing(),
            bad_poster(),
            export_onto_itself(),
            not_exported("denied"),
            settings_file_exists("kite.json"),
            not_opened(std::path::Path::new("/home/u/harnessUI/studio"), "denied"),
            poster_only_for_video(),
            lists_not_saved("database is locked"),
            engine_licence_not_accepted(),
            engine_still_downloading(),
            engine_not_removed("denied"),
            not_that_model("klein 9B"),
            notice_needed(),
            license_not_accepted(),
            disk_too_small(5_000_000_000),
            not_supported_yet("harnessUI does not know this kind of model yet."),
            not_offered(NotOffered::RegionLimited),
            not_offered(NotOffered::Uncensored),
            not_offered(NotOffered::AdultContent),
            model_incomplete(),
            wont_make(),
            prompt_too_long_here(),
            does_not_fit(None),
            does_not_fit(Some("needs 6 GB of graphics memory".into())),
            busy_for_update(),
            engine_in_use(),
            local_model_missing(),
            size_not_offered(),
            model_in_use(),
        ];
        all.extend(every_pack_error().iter().map(engine_not_installed));
        for to in [PictureUse::Chat, PictureUse::Prompt] {
            all.push(picture_too_big_for(to));
            all.push(not_a_picture_for(to, true));
            all.push(not_a_picture_for(to, false));
        }
        for company in [Company::OpenRouter, Company::OpenAi, Company::Local] {
            all.extend(with_company(company));
        }
        all
    }

    /// Every way downloading and installing the Studio engine pack can fail.
    fn every_pack_error() -> Vec<PackError> {
        vec![
            PackError::WrongPlatform,
            PackError::PathTooLong(300),
            PackError::Cancelled,
            PackError::BlockedHost("files.example.com".into()),
            PackError::ChecksumMismatch {
                file: "sd.zip".into(),
            },
            PackError::SizeMismatch {
                file: "sd.zip".into(),
                got: 1,
                expected: 2,
            },
            PackError::Network {
                file: "sd.zip".into(),
                message: "connection reset".into(),
            },
            PackError::Archive {
                file: "sd.zip".into(),
                message: "damaged".into(),
            },
            PackError::DiskFull,
            PackError::Io("vcomp140.dll is not in llama.cpp's folder".into()),
        ]
    }

    /// The engine pack's failures read as a backend download's do, with the Studio engine
    /// named where those name the backend.
    #[test]
    fn an_engine_download_failure_names_the_studio_engine() {
        for e in every_pack_error() {
            let shown = engine_not_installed(&e);
            assert_eq!(shown.details, e.to_string(), "{e:?}");
            for text in [&shown.message, &shown.next_step] {
                assert!(
                    !text.contains("backend") && !text.contains("ROCm"),
                    "{e:?} says {text:?}"
                );
            }
        }
        let shown = |e: PackError| {
            let e = engine_not_installed(&e);
            (e.code, e.message, e.next_step)
        };
        assert_eq!(
            shown(PackError::Network {
                file: "sd.zip".into(),
                message: "reset".into()
            }),
            (
                "network".to_owned(),
                "The Studio engine download was interrupted.".to_owned(),
                "Check your connection and try again; it continues where it stopped.".to_owned()
            )
        );
        assert_eq!(
            shown(PackError::ChecksumMismatch {
                file: "sd.zip".into()
            })
            .1,
            "The downloaded Studio engine does not match its pinned checksum, so it was not \
             installed."
        );
        assert_eq!(shown(PackError::DiskFull).0, "disk_full");
        assert_eq!(
            shown(PackError::BlockedHost("x.example.com".into())).0,
            "blocked_host"
        );
        assert_eq!(
            shown(PackError::Io("denied".into())).1,
            "The Studio engine could not be installed."
        );
        let e = engine_licence_not_accepted();
        assert_eq!(
            (e.code.as_str(), e.message.as_str()),
            (
                "bad_request",
                "Accept the licence to download the Studio engine."
            )
        );
        let e = engine_not_removed("denied");
        assert_eq!(
            (e.message.as_str(), e.details.as_str()),
            ("The Studio engine could not be removed.", "denied")
        );
    }

    /// Every way the guarded fetcher can fail, with a host and an address in it where the
    /// variant carries one.
    fn every_fetch_error() -> Vec<FetchError> {
        vec![
            FetchError::Scheme("ftp".into()),
            FetchError::Credentials,
            FetchError::NoHost,
            FetchError::Blocked {
                host: "files.example.com".into(),
                ip: "10.0.0.7".parse().unwrap(),
                class: AddressClass::Private,
            },
            FetchError::LocalName("files.example.com".into()),
            FetchError::Dns("files.example.com: no such host".into()),
            FetchError::Timeout(600),
            FetchError::TooManyRedirects,
            FetchError::BadRedirect("http://10.0.0.7/x".into()),
            FetchError::Status(403),
            FetchError::ContentType("text/html".into()),
            FetchError::Encoding("br".into()),
            FetchError::Network("connection reset by files.example.com".into()),
            FetchError::Challenge,
            FetchError::NoText,
            FetchError::NotMedia("text/html".into()),
            FetchError::TooLarge(1024 * 1024 * 1024),
            FetchError::DiskFull,
            FetchError::Disk("/home/u/harnessUI/studio/.partial/x: denied".into()),
        ]
    }

    #[test]
    fn a_result_link_failure_is_said_in_studios_words() {
        for e in every_fetch_error() {
            let shown = from_fetch(Company::OpenRouter, &e);
            for text in [&shown.message, &shown.next_step, &shown.details] {
                assert!(
                    !text.contains("example.com")
                        && !text.contains("10.0.0.7")
                        && !text.contains('/')
                        && !text.contains("page"),
                    "{e:?} shows {text:?}"
                );
            }
        }
        let code = |e: FetchError| from_fetch(Company::OpenRouter, &e).code;
        assert_eq!(code(FetchError::DiskFull), "studio_disk_full");
        assert_eq!(code(FetchError::TooLarge(1)), "studio_too_large");
        assert_eq!(
            code(FetchError::NotMedia("text/html".into())),
            "studio_not_media"
        );
        assert_eq!(code(FetchError::Disk("denied".into())), "studio_files");
        assert_eq!(code(FetchError::Status(403)), "studio_download_failed");
        assert_eq!(code(FetchError::Credentials), "studio_download_failed");
        let refused = from_fetch(
            Company::OpenRouter,
            &FetchError::LocalName("localhost".into()),
        );
        assert_eq!(
            refused.message,
            "The result could not be downloaded from OpenRouter."
        );
        assert_eq!(
            refused.details,
            "The result link points to an address Studio never downloads from."
        );
        assert_eq!(
            from_fetch(Company::OpenRouter, &FetchError::Status(403)).details,
            "The file host answered with HTTP status 403."
        );
    }

    #[test]
    fn a_run_that_never_had_its_turn_says_nothing_was_charged() {
        let e = closed_before_its_turn(Company::OpenRouter);
        assert_eq!(
            e.message,
            "Not sent: the app closed before its turn. Nothing was charged."
        );
        assert_eq!(e.code, "studio_not_sent");
        let e = nothing_to_fetch();
        assert_eq!(e.code, "bad_request");
        assert_eq!(e.next_step, "Make it again with Again.");
    }

    #[test]
    fn a_failed_read_of_a_list_or_price_is_not_worded_as_a_run() {
        let answered = MediaFailure {
            message: "No endpoints found".into(),
            status: Some(404),
            ..failure(FailureKind::NotFound)
        };
        let e = from_read_failure(Company::OpenRouter, &answered);
        assert_eq!(
            (e.code.as_str(), e.message.as_str(), e.next_step.as_str()),
            (
                "studio_not_fetched",
                "OpenRouter could not give the model list or price.",
                "Try again later. If it keeps happening, copy the details into a bug report."
            )
        );
        assert_eq!(e.details, "No endpoints found; HTTP status: 404");
        let e = from_read_failure(Company::OpenAi, &failure(FailureKind::Other));
        assert_eq!(e.message, "OpenAI could not give the model list or price.");
        // A refusal of the key, credit or rate keeps the words a run gets.
        let e = from_read_failure(Company::OpenRouter, &failure(FailureKind::Unauthorized));
        assert_eq!(e.code, "studio_unauthorized");
        assert_eq!(e.message, "OpenRouter refused the API key.");
        let e = lists_not_saved("database is locked");
        assert_eq!(
            (e.code.as_str(), e.message.as_str(), e.details.as_str()),
            (
                "studio_not_saved",
                "The model list or price could not be saved on this computer.",
                "database is locked"
            )
        );
    }

    #[test]
    fn the_gallery_actions_say_what_to_do_instead() {
        let e = not_opened(
            std::path::Path::new("/home/u/harnessUI/studio"),
            "no file manager",
        );
        assert_eq!(
            (e.code.as_str(), e.message.as_str(), e.next_step.as_str()),
            (
                "studio_not_opened",
                "The folder could not be opened.",
                "Open it yourself: /home/u/harnessUI/studio"
            )
        );
        assert_eq!(e.details, "no file manager");
        let e = settings_file_exists("kite.json");
        assert_eq!(
            (e.code.as_str(), e.message.as_str(), e.next_step.as_str()),
            (
                "bad_request",
                "A file named kite.json is already there.",
                "Choose another name."
            )
        );
        assert_eq!(poster_only_for_video().code, "bad_request");
    }

    /// A local download's refusals, in the words Get local models shows.
    #[test]
    fn a_local_download_says_why_it_was_refused() {
        let words = |e: AppError| (e.code, e.message, e.next_step);
        assert_eq!(
            words(disk_too_small(5_040_000_000)),
            (
                "studio_disk_too_small".into(),
                "There is not enough free space for this model.".into(),
                "It needs 5.1 GB; free up space or choose another models folder.".into()
            )
        );
        assert_eq!(
            disk_too_small(5_000_000_000).next_step,
            "It needs 5.0 GB; free up space or choose another models folder."
        );
        assert_eq!(
            words(not_offered(NotOffered::RegionLimited)),
            (
                "studio_not_offered".into(),
                "harnessUI does not offer this model: its licence excludes some regions.".into(),
                "Choose another model.".into()
            )
        );
        let e = not_supported_yet("This repository holds several kinds of model.");
        assert_eq!(
            (e.code.as_str(), e.message.as_str(), e.details.as_str()),
            (
                "studio_not_supported_yet",
                "harnessUI cannot run this model yet.",
                "This repository holds several kinds of model."
            )
        );
        assert_eq!(
            words(not_that_model("found klein 9B")),
            (
                "studio_not_that_model".into(),
                "This file is not the model its name says, so it was not added.".into(),
                "Choose another file, or a model from the recommended list.".into()
            )
        );
        assert_eq!(
            notice_needed().message,
            "Read the note about local models first."
        );
        assert_eq!(
            license_not_accepted().message,
            "Accept the licences to download this model."
        );
    }

    /// A local run's refusals, in the words Studio shows (Task L15).
    #[test]
    fn a_local_run_says_why_it_was_refused() {
        let words = |e: AppError| (e.code, e.message, e.next_step);
        assert_eq!(
            words(prompt_too_long_here()),
            (
                "studio_prompt_too_long_here".into(),
                PROMPT_TOO_LONG_HERE.into(),
                "Shorten it, or choose a cloud model.".into()
            )
        );
        assert_eq!(
            PROMPT_TOO_LONG_HERE,
            "Keep the prompt under 4,000 characters for local models."
        );
        assert_eq!(
            words(does_not_fit(Some("needs 6 GB of graphics memory".into()))),
            (
                "studio_does_not_fit".into(),
                "This model does not fit this computer.".into(),
                "Choose a smaller size or a cloud model.".into()
            )
        );
        assert_eq!(
            does_not_fit(Some("needs 6 GB of graphics memory".into())).details,
            "needs 6 GB of graphics memory"
        );
        assert_eq!(
            words(busy_for_update()),
            (
                "studio_busy_for_update".into(),
                "A picture or video is being made on this computer.".into(),
                "Stop it in Studio, or install the update when it finishes.".into()
            )
        );
        assert_eq!(engine_in_use().code, "studio_engine_in_use");
        assert_eq!(local_model_missing().code, "studio_local_model_missing");
        assert_eq!(
            words(model_in_use()),
            (
                "studio_model_in_use".into(),
                "A run on this computer is using this model.".into(),
                "Stop the run, then delete it.".into()
            )
        );
    }

    /// Why a model is not offered here, as the estimate and Get local models say it.
    #[test]
    fn a_reason_names_what_this_computer_lacks() {
        use models::gen_fit::NotFor;
        assert_eq!(
            not_here_reason(
                NotFor::GraphicsMemory {
                    needs: 5_770_000_000
                },
                false
            ),
            "needs 6 GB of graphics memory"
        );
        assert_eq!(
            not_here_reason(
                NotFor::GraphicsMemory {
                    needs: 6_000_000_000
                },
                false
            ),
            "needs 6 GB of graphics memory"
        );
        assert_eq!(
            not_here_reason(
                NotFor::GraphicsMemory {
                    needs: 20_100_000_000
                },
                true
            ),
            "needs 21 GB of free memory"
        );
        assert_eq!(
            not_here_reason(
                NotFor::MacMemory {
                    needs: 31_000_000_000
                },
                false
            ),
            "needs a Mac with 31 GB or more"
        );
        assert_eq!(
            not_here_reason(NotFor::VideoNeedsGpu, true),
            "video needs a graphics card or an Apple-silicon Mac"
        );
        assert_eq!(
            not_here_reason(NotFor::VideoNotOnThisMac, false),
            "local video needs a graphics card on this computer"
        );
    }

    /// Get local models' words: a smaller size that fits, and why a repository is never
    /// offered in the clause `studio_not_offered` uses.
    #[test]
    fn get_local_models_says_what_fits_and_what_is_never_offered() {
        let size = |width, height, frames| GenSize {
            width,
            height,
            frames,
        };
        assert_eq!(fits_at(size(768, 768, 1), false), "Fits at 768×768");
        assert_eq!(
            fits_at(size(832, 480, 49), true),
            "Fits at 832×480, 49 frames"
        );
        for why in [
            NotOffered::RegionLimited,
            NotOffered::Uncensored,
            NotOffered::AdultContent,
        ] {
            assert_eq!(
                not_offered(why).message,
                format!(
                    "harnessUI does not offer this model: {}.",
                    not_offered_reason(why)
                )
            );
        }
    }

    #[test]
    fn studio_errors_have_four_parts() {
        for e in every_constructor() {
            assert!(
                !e.code.is_empty() && !e.message.is_empty() && !e.next_step.is_empty(),
                "{e:?}"
            );
        }
    }

    #[test]
    fn a_run_on_this_computer_never_points_to_a_website() {
        for e in with_company(Company::Local) {
            for text in [&e.message, &e.next_step, &e.details] {
                let lower = text.to_lowercase();
                for cloud in [
                    "openrouter",
                    "openai",
                    ".ai",
                    ".com",
                    "activity page",
                    "internet",
                    "account",
                    "api key",
                    "get it anyway",
                    "may have been charged",
                    "may still",
                ] {
                    assert!(!lower.contains(cloud), "{e:?} says {cloud:?}");
                }
                // The company's name for this computer is never a sentence's subject.
                assert!(!text.contains("This computer"), "{e:?}");
            }
        }
        let e = not_answered(Company::Local, None);
        assert_eq!(
            (e.code.as_str(), e.message.as_str(), e.next_step.as_str()),
            (
                "studio_not_answered",
                "The Studio engine on this computer did not answer.",
                "Try again; if it keeps happening, restart the app."
            )
        );
        let e = job_lost(Company::Local);
        assert_eq!(
            e.message,
            "The Studio engine on this computer no longer has this job."
        );
        assert_eq!(e.next_step, "Make it again if you still want it.");
        let e = from_failure(Company::Local, &failure(FailureKind::NoCredit));
        assert_eq!(e.code, "studio_no_credit");
        assert_eq!(
            e.message,
            "The Studio engine on this computer could not make it."
        );
        assert_eq!(
            from_failure(Company::Local, &failure(FailureKind::Server)).next_step,
            "Try again; if it keeps happening, restart the app."
        );
        assert_eq!(
            from_failure(Company::Local, &failure(FailureKind::OrgVerification)).code,
            "studio_org_verification"
        );
        // The local kinds keep their own words.
        assert_eq!(
            from_failure(Company::Local, &failure(FailureKind::EngineFailed)).message,
            "The Studio engine stopped."
        );
        // A cloud company still points to its page.
        assert_eq!(
            job_lost(Company::OpenRouter).next_step,
            "Check your activity page on openrouter.ai, then make it again if you still want it."
        );
    }

    #[test]
    fn a_video_stopped_while_sending_has_no_get_it_anyway() {
        let e = stopped_while_sending(Company::OpenRouter);
        assert_eq!(e.code, "studio_stopped");
        assert_eq!(
            e.message,
            "Stopped while sending; it may or may not have started."
        );
        assert_eq!(
            e.next_step,
            "It may still be charged: check your activity page on openrouter.ai before you make it again."
        );
        assert!(!e.next_step.contains("Get it anyway"));
        assert_eq!(
            stopped_while_sending(Company::Local).message,
            "Stopped. Your computer is no longer working on it. Nothing was charged."
        );
    }

    #[test]
    fn a_task_that_broke_off_says_it_may_or_may_not_have_started() {
        let e = broke_off(Company::OpenAi);
        assert_eq!(e.code, "studio_interrupted");
        assert_eq!(
            e.message,
            "Studio stopped following this after an error in the app; it may or may not have started."
        );
        assert!(e.next_step.contains("platform.openai.com"));
        let e = added_picture_missing();
        assert_eq!(
            (e.code.as_str(), e.message.as_str(), e.next_step.as_str()),
            (
                "studio_ref_missing",
                "One of the added pictures is no longer on this computer.",
                "Add the picture again."
            )
        );
    }

    #[test]
    fn the_daily_limit_says_what_was_spent_of_what() {
        assert_eq!(
            daily_limit(9.5, 10.0).message,
            "Daily limit reached: $9.50 of $10 spent today"
        );
        assert_eq!(
            daily_limit(12.0, 12.5).message,
            "Daily limit reached: $12.00 of $12.50 spent today"
        );
        assert_eq!(
            daily_limit(999.99, 1000.0).message,
            "Daily limit reached: $999.99 of $1,000 spent today"
        );
    }

    #[test]
    fn an_unknown_price_refused_for_the_reserve_says_so() {
        let e = unknown_price_needs_reserve(0.0, 3.0);
        assert_eq!(e.code, "studio_daily_limit");
        assert_eq!(
            e.message,
            "This model's price is unknown, so Studio needs $5 of today's limit left ($3.00 left)."
        );
        assert_eq!(
            unknown_price_needs_reserve(5.8, 10.0).message,
            "This model's price is unknown, so Studio needs $5 of today's limit left ($4.20 left)."
        );
        // Spent past the limit: nothing is left.
        assert!(
            unknown_price_needs_reserve(10.4, 10.0)
                .message
                .ends_with("($0.00 left).")
        );
    }

    #[test]
    fn the_safety_message_names_the_company_the_stage_and_the_charge() {
        let e = blocked(
            Company::OpenAi,
            Some(Stage::Prompt),
            Some(false),
            Some("req_8f2"),
        );
        assert_eq!(e.code, "studio_blocked");
        assert_eq!(e.message, "OpenAI's safety filter refused the prompt.");
        assert!(e.next_step.contains("Not charged"));
        assert!(e.details.contains("req_8f2"));
        assert_eq!(
            blocked(Company::OpenRouter, Some(Stage::Output), None, None).message,
            "OpenRouter's safety filter stopped the picture after it was made."
        );
        assert!(
            blocked(Company::OpenRouter, None, None, None)
                .next_step
                .starts_with("It may have been charged.")
        );
    }

    #[test]
    fn a_stop_says_what_it_may_cost() {
        let msg = |company, kind, charged| stopped(company, kind, charged).message;
        assert_eq!(
            msg(Company::OpenAi, MediaKind::Image, None),
            "Stopped waiting. OpenAI may still charge for it."
        );
        assert_eq!(
            msg(Company::OpenRouter, MediaKind::Image, Some(false)),
            "Stopped. OpenRouter does not charge for a picture you did not receive."
        );
        assert_eq!(
            msg(Company::OpenAi, MediaKind::Image, Some(false)),
            "Stopped. Nothing was charged."
        );
        assert_eq!(
            msg(Company::OpenRouter, MediaKind::Video, None),
            "OpenRouter may still finish and charge for this."
        );
        // Stopped while its result was downloading: it had finished.
        let done = stopped(Company::OpenRouter, MediaKind::Video, Some(true));
        assert_eq!(
            done.message,
            "Stopped. OpenRouter finished this video, so it was charged."
        );
        assert_eq!(done.next_step, "Press Get it anyway to download it.");
        assert_eq!(
            msg(Company::Local, MediaKind::Video, Some(false)),
            "Stopped. Your computer is no longer working on it. Nothing was charged."
        );
    }

    #[test]
    fn an_interrupted_run_says_where_to_look() {
        let e = interrupted(Company::OpenRouter, None);
        assert_eq!(
            e.message,
            "The app closed while sending; it may or may not have started."
        );
        assert_eq!(e.next_step, "Check your activity page on openrouter.ai.");
        let picture = interrupted(Company::OpenRouter, Some(false));
        assert_eq!(picture.message, "The app closed while sending.");
        assert_eq!(
            picture.next_step,
            "OpenRouter does not charge for a picture you did not receive. Make it again if you \
             still want it."
        );
        assert_eq!(
            interrupted(Company::Local, Some(false)).message,
            "The app closed while this was being made."
        );
    }

    #[test]
    fn bad_settings_name_fields_by_their_labels() {
        let m = model(vec![quality()]);
        let sentences = invalid_sentences(
            &[
                Invalid::BadValue("quality".into()),
                Invalid::NotOffered("seed".into()),
                Invalid::NotOffered("cfg_boost".into()),
                Invalid::Requires {
                    key: "output_compression".into(),
                    needs: "output_format".into(),
                },
                Invalid::TooManyPictures {
                    role: RefRole::StartFrame,
                    max: 1,
                },
                Invalid::TooManyPictures {
                    role: RefRole::EndFrame,
                    max: 0,
                },
                Invalid::TooFewPictures {
                    role: RefRole::Reference,
                    min: 2,
                },
                Invalid::Reserved("prompt".into()),
                Invalid::PromptTooLong,
                Invalid::NotUsable("Needs a video to start from.".into()),
            ],
            &m,
        );
        assert_eq!(
            sentences,
            [
                "Picture quality has a value this model does not take.",
                "Seed is not offered by this model.",
                "\"cfg_boost\" is not offered by this model.",
                "Compression needs File type set first.",
                "This model takes at most 1 start frame.",
                "This model takes no end frames.",
                "This model needs at least 2 reference pictures.",
                "\"prompt\" cannot be set under Extra fields: it has its own place.",
                "Keep the prompt under 32,000 characters.",
                "Needs a video to start from.",
            ]
        );
        let e = invalid_settings(
            &[Invalid::EmptyPrompt, Invalid::BadValue("quality".into())],
            &m,
        );
        assert_eq!(e.code, "studio_bad_settings");
        assert_eq!(
            e.message,
            "Write a prompt first. Picture quality has a value this model does not take."
        );
        assert_eq!(e.next_step, "Change the settings and try again.");
    }

    #[test]
    fn caps_and_counts_read_plainly() {
        assert_eq!(
            too_large(50 * 1024 * 1024).message,
            "The result is larger than 50 MB, so it was not kept."
        );
        assert_eq!(
            too_large(1024 * 1024 * 1024).message,
            "The result is larger than 1 GB, so it was not kept."
        );
        assert_eq!(
            gallery_full().message,
            "Studio keeps at most 20,000 runs, and that many are kept."
        );
        assert_eq!(usd(0.041), "$0.04");
        assert_eq!(usd(1234.5), "$1,234.50");
    }

    #[test]
    fn gallery_and_write_failures_never_show_paths() {
        let e = from_gallery(&GalleryError::Io(
            "/home/u/harnessUI/studio/x.png: denied".into(),
        ));
        assert!(!e.message.contains('/') && !e.details.contains('/'));
        assert_eq!(
            from_gallery(&GalleryError::DiskFull).code,
            "studio_disk_full"
        );
        assert_eq!(
            from_gallery(&GalleryError::TooLarge(10)).code,
            "studio_too_large"
        );
        assert_eq!(from_local(&LocalIo::DiskFull, 1).code, "studio_disk_full");
        assert_eq!(
            from_local(&LocalIo::TooLarge(1024), 50 * 1024 * 1024).message,
            "The result is larger than 50 MB, so it was not kept."
        );
    }

    #[test]
    fn a_company_failure_keeps_its_own_words_in_the_details() {
        let f = MediaFailure {
            kind: FailureKind::NoCredit,
            stage: None,
            charged: Some(false),
            company_code: Some("insufficient_credits".into()),
            request_id: Some("gen-1".into()),
            status: Some(402),
            message: "Insufficient credits".into(),
            retry_after: None,
        };
        let e = from_failure(Company::OpenRouter, &f);
        assert_eq!(e.code, "studio_no_credit");
        assert_eq!(e.message, "Your OpenRouter account has no credit left.");
        assert_eq!(
            e.details,
            "Insufficient credits; Code: insufficient_credits; HTTP status: 402; Request id: gen-1"
        );
        let local = MediaFailure {
            kind: FailureKind::OutOfMemory,
            ..f
        };
        let e = from_failure(Company::Local, &local);
        assert_eq!(
            (e.code.as_str(), e.message.as_str(), e.next_step.as_str()),
            (
                "studio_out_of_memory",
                "Ran out of graphics memory.",
                "Try 480p or a shorter clip."
            )
        );
    }

    /// A local run's failure says in its details what the engine did, in a sentence: an
    /// engine that exits 0 without a file finished without making it, never "crashed".
    #[test]
    fn a_local_runs_details_say_what_the_engine_did() {
        assert_eq!(
            engine_failure_details(&SdFailure::Crashed { code: Some(0) }, MediaKind::Image),
            "The Studio engine finished without making a picture."
        );
        assert_eq!(
            engine_failure_details(&SdFailure::Crashed { code: Some(0) }, MediaKind::Video),
            "The Studio engine finished without making a video."
        );
        assert_eq!(
            engine_failure_details(&SdFailure::Crashed { code: Some(3) }, MediaKind::Image),
            "The Studio engine stopped with exit code 3."
        );
        assert_eq!(
            engine_failure_details(
                &SdFailure::MissingLibrary("vulkan-1.dll".into()),
                MediaKind::Image
            ),
            "The Studio engine could not load a library it needs: vulkan-1.dll."
        );
        assert_eq!(
            local_args_refused(&ArgsError::BadSize),
            "The size is not a multiple of 16 from 64 to 2048."
        );
        assert_eq!(
            local_args_refused(&ArgsError::PercentInOutputPath),
            "The output path holds a %d numbering template."
        );
    }
}
