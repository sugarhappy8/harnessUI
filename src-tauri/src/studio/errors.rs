//! Studio's backend wording: every sentence a Studio error shows, in plain English.
//!
//! Each error is a four-part [`AppError`]: a code the webview can test, a message and a next
//! step in plain words, and details for a bug report. No message shows a raw setting key
//! (an Extra field the user typed is quoted), a path or a hash.

use agent::media::{
    Company, FailureKind, Invalid, LocalIo, MediaFailure, MediaKind, MediaModel, RefRole, Stage,
};
use search::FetchError;

use super::gallery::GalleryError;
use super::spend::ConfirmReason;
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

/// Where the user can see what a company charged.
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
        request_id.map_or_else(String::new, |id| format!("Request id: {id}")),
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

/// The app closed while the run was being sent (cloud) or made (this computer).
pub fn interrupted(company: Company) -> AppError {
    match company {
        Company::Local => AppError::new(
            "studio_interrupted",
            "The app closed while this was being made.",
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

/// The company no longer has the finished result.
pub fn result_expired(company: Company) -> AppError {
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

/// The finished result could not be downloaded.
pub fn download_failed(company: Company) -> AppError {
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

/// What a company's (or the Studio engine's) failure says, in plain words, with the
/// company's own message, code and request id in the details.
pub fn from_failure(company: Company, f: &MediaFailure) -> AppError {
    let who = name(company);
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
    let with_details = |code: &str, message: String, next: String| {
        AppError::new(code, message, next, details.join("; "))
    };
    let simple = |code: &str, message: &str, next: &str| {
        with_details(code, message.to_owned(), next.to_owned())
    };
    match f.kind {
        FailureKind::Blocked => blocked(company, f.stage, f.charged, f.request_id.as_deref()),
        FailureKind::OrgVerification => org_verification(),
        FailureKind::Expired => result_expired(company),
        FailureKind::Unauthorized => with_details(
            "studio_unauthorized",
            format!("{who} refused the API key."),
            "Check the key in Settings → Endpoints.".to_owned(),
        ),
        FailureKind::NoCredit => with_details(
            "studio_no_credit",
            format!("Your {who} account has no credit left."),
            format!("Add credit on {}, then try again.", activity_page(company)),
        ),
        FailureKind::RateLimited => with_details(
            "studio_rate_limited",
            format!("{who} has had too many requests."),
            "Wait a minute, then try again.".to_owned(),
        ),
        FailureKind::BadRequest => with_details(
            "studio_refused",
            format!("{who} refused the request as it was."),
            "Change the settings and try again.".to_owned(),
        ),
        FailureKind::NotFound => with_details(
            "studio_not_found",
            format!("{who} could not find this model."),
            "Get the model list again, then pick a model.".to_owned(),
        ),
        FailureKind::Server => with_details(
            "studio_server_error",
            format!("{who}'s server failed."),
            format!("{} Try again in a few minutes.", charge_line(f.charged)),
        ),
        FailureKind::Other => with_details(
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

    /// Every constructor in this file, which `studio_errors_have_four_parts` checks. A new
    /// constructor is added here too (Tasks L11, L13 and L15 append theirs).
    fn every_constructor() -> Vec<AppError> {
        let failure = |kind| MediaFailure {
            kind,
            stage: None,
            charged: None,
            company_code: None,
            request_id: None,
            status: None,
            message: String::new(),
            retry_after: None,
        };
        let mut all = vec![
            needs_confirmation(&[ConfirmReason::Video]),
            daily_limit(9.5, 10.0),
            no_endpoint(),
            no_key("My OpenRouter"),
            wrong_kind(MediaKind::Video),
            gallery_full(),
            no_local_engine(),
            invalid_settings(&[Invalid::EmptyPrompt], &model(Vec::new())),
            blocked(Company::OpenAi, None, None, None),
            org_verification(),
            job_lost(Company::OpenRouter),
            interrupted(Company::OpenRouter),
            interrupted(Company::Local),
            result_expired(Company::OpenRouter),
            disk_full(),
            too_large(1024),
            not_sent(Company::OpenAi),
            not_answered(Company::OpenAi, None),
            stopped(Company::OpenAi, MediaKind::Image, None),
            download_failed(Company::OpenRouter),
            from_local(&LocalIo::Io("denied".into()), 1),
            from_gallery(&GalleryError::NotMedia),
            from_gallery(&GalleryError::Io("/a/path".into())),
            gallery_not_full_path(),
            numbered_folder(),
            closed_before_its_turn(Company::OpenRouter),
            nothing_to_fetch(),
            too_big_to_show(),
            not_fetched(Company::OpenRouter),
            bad_ref_name(),
        ];
        all.extend(
            every_fetch_error()
                .iter()
                .map(|e| from_fetch(Company::OpenRouter, e)),
        );
        for kind in [
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
        ] {
            all.push(from_failure(Company::OpenRouter, &failure(kind)));
        }
        all
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
    fn studio_errors_have_four_parts() {
        for e in every_constructor() {
            assert!(
                !e.code.is_empty() && !e.message.is_empty() && !e.next_step.is_empty(),
                "{e:?}"
            );
        }
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
        let e = interrupted(Company::OpenRouter);
        assert_eq!(
            e.message,
            "The app closed while sending; it may or may not have started."
        );
        assert_eq!(e.next_step, "Check your activity page on openrouter.ai.");
        assert_eq!(
            interrupted(Company::Local).message,
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
}
