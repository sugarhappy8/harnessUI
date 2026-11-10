//! The one error type commands return. It serialises to the four parts the UI
//! shows: a code, a plain-language message, what to do next, and technical details
//! behind "Copy details" (ARCHITECTURE.md 15). Raw errors go to the log, not the UI.

use serde::Serialize;

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppError {
    pub code: String,
    pub message: String,
    pub next_step: String,
    pub details: String,
}

impl AppError {
    pub fn new(
        code: &str,
        message: impl Into<String>,
        next_step: impl Into<String>,
        details: impl Into<String>,
    ) -> Self {
        let err = Self {
            code: code.to_string(),
            message: message.into(),
            next_step: next_step.into(),
            details: details.into(),
        };
        tracing::warn!(code = %err.code, details = %err.details, "{}", err.message);
        err
    }

    pub fn internal(details: impl std::fmt::Display) -> Self {
        Self::new(
            "internal",
            "Something went wrong inside the app.",
            "Try again. If it keeps happening, copy the details into a bug report.",
            details.to_string(),
        )
    }

    pub fn not_found(what: &str) -> Self {
        Self::new(
            "not_found",
            format!("That {what} no longer exists."),
            "Refresh the list and try again.",
            String::new(),
        )
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(
            "bad_request",
            message,
            "Check the value and try again.",
            String::new(),
        )
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for AppError {}

impl From<store::StoreError> for AppError {
    fn from(e: store::StoreError) -> Self {
        Self::new(
            "storage",
            "The app could not read or write its local database.",
            "Make sure the disk is not full, then restart the app.",
            e.to_string(),
        )
    }
}

impl From<agent::ProviderError> for AppError {
    fn from(e: agent::ProviderError) -> Self {
        let code = match &e {
            agent::ProviderError::ContextOverflow { .. } => "context_overflow",
            agent::ProviderError::Unauthorized => "unauthorized",
            agent::ProviderError::Network(_) => "network",
            agent::ProviderError::Cancelled => "cancelled",
            agent::ProviderError::ServerParseError(_) => "model_output_unparseable",
            _ => "provider",
        };
        let next = match &e {
            agent::ProviderError::ContextOverflow { .. } => {
                "Start a new chat, or raise the context size in Settings → Local models and reload the model."
            }
            agent::ProviderError::Unauthorized => {
                "Check the API key for this endpoint in Settings → Endpoints."
            }
            agent::ProviderError::Network(_) => {
                "Check that the server is running and the address is right."
            }
            _ => "Try again. If it keeps happening, copy the details into a bug report.",
        };
        Self::new(code, e.user_message(), next, format!("{e:?}"))
    }
}

pub type AppResult<T> = Result<T, AppError>;
