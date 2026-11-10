//! App-wide state handed to commands.

use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use crate::runtime_manager::RuntimeManager;

pub struct AppState {
    pub store: store::Store,
    pub secrets: Arc<dyn store::secrets::SecretStore>,
    pub runtime: Arc<RuntimeManager>,
    /// Cancels the reply being generated (one at a time in v1).
    pub generation: Mutex<Option<CancellationToken>>,
}
