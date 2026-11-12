//! The chat's open file and its result ids (item5-spec.md R12, WP9g `session.rs`): one
//! binding (chat, canonical path, identity, decompiler, generation, open counter), the
//! reason a chat's file was closed, and the ledger of `ev_…` ids, each recorded for the
//! chat whose result showed it first. [`SessionView`] is the read-only snapshot `describe`
//! uses (no side effects there).
//!
//! TODO(0.0.6): WP9g builds the mutable session and the ledger. This is WP9a's contract
//! stub: the view types are frozen.

use std::collections::HashMap;

use crate::{Decompiler, FileIdentity};

/// The open file, as `describe` sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindingView {
    /// The chat that opened it.
    pub chat_id: i64,
    /// Its canonical path.
    pub canonical: String,
    /// Its identity when opened.
    pub identity: FileIdentity,
    /// The decompiler it was opened with.
    pub decompiler: Decompiler,
    /// rea's generation when opened.
    pub generation: u64,
    /// Grows at every open, close and reset the app sends.
    pub open_seq: u64,
}

/// Why a chat's file was closed (the message the model reads).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClosedReason {
    /// "rea closed this file because another chat opened one."
    OtherChat,
    /// "rea closed this file because the decompiler was changed or updated in Settings."
    SettingsChanged,
}

/// A result id's owner, as `describe` sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvidenceView {
    /// The chat whose result showed it first.
    pub chat_id: i64,
    /// The store epoch it was seen in.
    pub store_epoch: u64,
    /// The canonical path it came from.
    pub canonical: String,
}

/// A snapshot of the session.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionView {
    /// The open file, if any.
    pub binding: Option<BindingView>,
    /// Grows at every rea start and every reset.
    pub store_epoch: u64,
    closed: HashMap<i64, ClosedReason>,
    evidence: HashMap<String, EvidenceView>,
}

impl SessionView {
    /// A snapshot from its parts.
    pub fn new(
        binding: Option<BindingView>,
        store_epoch: u64,
        closed: HashMap<i64, ClosedReason>,
        evidence: HashMap<String, EvidenceView>,
    ) -> Self {
        Self {
            binding,
            store_epoch,
            closed,
            evidence,
        }
    }
    /// Why this chat's file was closed, if it was.
    pub fn closed_reason(&self, chat: i64) -> Option<ClosedReason> {
        self.closed.get(&chat).copied()
    }
    /// Who a result id belongs to.
    pub fn evidence(&self, id: &str) -> Option<&EvidenceView> {
        self.evidence.get(id)
    }
}
