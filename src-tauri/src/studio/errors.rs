//! Studio's backend wording: every sentence a Studio error shows, in plain English.

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

/// The gallery folder picked has `%d`, `%3d` or `%03d` in its path, which the Studio
/// engine would read as a numbering template for the files it writes there.
pub fn numbered_folder() -> AppError {
    AppError::new(
        "bad_request",
        "Pick a folder whose name has no %d in it.",
        "Rename the folder, or pick another one.",
        String::new(),
    )
}
