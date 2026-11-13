//! The chat's open file and its result ids (item5-spec.md R12, WP9g `session.rs`): one
//! binding (chat, canonical path, identity, decompiler, generation, open counter), the
//! reason a chat's file was closed, and the ledger of `ev_…` ids, each recorded for the
//! chat whose result showed it first. [`SessionView`] is the read-only snapshot `describe`
//! uses (no side effects there); [`Session`] is the state `call` changes while it holds
//! rea's lock (`crate::server`).
//!
//! - The **open counter** grows at every `open_binary`, `close_binary` and reset the app
//!   sends, so a session call described before any of them no longer matches.
//! - The **store epoch** grows at every start of rea and every `reset_investigation`: rea
//!   keeps its results in memory, per process, so ids of an older epoch are gone (or no
//!   longer this chat's to read).
//! - The **ledger** keeps at most [`MAX_IDS`] records of an id for a chat. rea's ids are
//!   digests of a result's content, so they can be predicted, and a file's text can show
//!   one before rea makes it. So an id belongs to the chats whose own successful results
//!   showed it **about the same file** (another chat that checks the same file gets the
//!   same id, and may read it: rea's record of it is about a file that chat can read). An
//!   id seen in any result about **another** file is contested: rea's record may be about
//!   either file, so no chat may read it. Ids in error results are never recorded (they
//!   still contest). When the ledger is full, new ids are left out, never older ones
//!   dropped (dropping one would let a later result give it to another chat), and the next
//!   `open_binary` clears rea, which starts a new store epoch.

use std::collections::{HashMap, VecDeque};

use crate::{Decompiler, FileIdentity};

/// Most records of a result id for a chat the ledger keeps (in one store epoch).
pub const MAX_IDS: usize = 10_000;
/// Most chats whose closed reason is kept (oldest dropped).
const MAX_CLOSED: usize = 1_000;

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

/// The identity a folder binding shows in [`BindingView`] (a `.app` bundle has none: it is
/// walked again instead, [`Binding::folder`]).
pub const FOLDER_IDENTITY: FileIdentity = FileIdentity {
    device: 0,
    file: 0,
    size: 0,
    modified: 0,
};

/// The open file, with what the app needs to open it again (R21).
#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    /// What `describe` sees.
    pub view: BindingView,
    /// It is a folder (a Mac `.app` bundle): walked again at each call instead of an
    /// identity check.
    pub folder: bool,
    /// The arguments it was opened with (canonical path, format, architecture, entry
    /// point), sent again when rea lost it.
    pub open_args: serde_json::Map<String, serde_json::Value>,
}

/// One result id in the ledger.
#[derive(Clone, Debug)]
struct Entry {
    /// The chats whose own successful results about `canonical` showed it, the first one
    /// first.
    owners: Vec<i64>,
    /// The store epoch it was seen in.
    store_epoch: u64,
    /// The canonical path of the first result that showed it.
    canonical: String,
    /// It was also seen in a result about another file: no chat may read it.
    contested: bool,
}

/// The session `call` changes while it holds rea's lock.
#[derive(Debug, Default)]
pub struct Session {
    binding: Option<Binding>,
    open_seq: u64,
    store_epoch: u64,
    closed: HashMap<i64, ClosedReason>,
    closed_order: VecDeque<i64>,
    evidence: HashMap<String, Entry>,
    /// Records of an id for a chat (the sum of every entry's owners), at most [`MAX_IDS`].
    slots: usize,
    /// An id was left out because the ledger was full: the next open clears rea.
    full: bool,
    /// The chat and rea generation of the last `open_binary` sent (by the model or the
    /// app); rea keeps that file's results after a close.
    last_open: Option<(i64, u64)>,
}

impl Session {
    /// An empty session.
    pub fn new() -> Self {
        Self::default()
    }

    /// The snapshot `describe` uses for `chat`: the binding (whoever holds it), this chat's
    /// closed reason, and this chat's ids of the current epoch (another chat's ids are
    /// never in it, so they read as unknown).
    pub fn snapshot_for(&self, chat: i64) -> SessionView {
        let closed = self
            .closed
            .get(&chat)
            .map(|r| HashMap::from([(chat, *r)]))
            .unwrap_or_default();
        let evidence = self
            .evidence
            .keys()
            .filter_map(|id| Some((id.clone(), self.evidence_for(chat, id)?)))
            .collect();
        SessionView::new(
            self.binding.as_ref().map(|b| b.view.clone()),
            self.store_epoch,
            closed,
            evidence,
        )
    }

    /// The open file.
    pub fn binding(&self) -> Option<&Binding> {
        self.binding.as_ref()
    }

    /// The open file, if `chat` holds it.
    pub fn binding_of(&self, chat: i64) -> Option<&Binding> {
        self.binding.as_ref().filter(|b| b.view.chat_id == chat)
    }

    /// The open counter.
    pub fn open_seq(&self) -> u64 {
        self.open_seq
    }

    /// The store epoch.
    pub fn store_epoch(&self) -> u64 {
        self.store_epoch
    }

    /// The last `open_binary` sent: chat and rea generation.
    pub fn last_open(&self) -> Option<(i64, u64)> {
        self.last_open
    }

    /// Why `chat`'s file was closed, if it was.
    pub fn closed_reason(&self, chat: i64) -> Option<ClosedReason> {
        self.closed.get(&chat).copied()
    }

    /// The id as `chat` may read it: recorded for `chat` in the current epoch and not
    /// contested.
    pub fn evidence_for(&self, chat: i64, id: &str) -> Option<EvidenceView> {
        self.evidence
            .get(id)
            .filter(|e| {
                e.store_epoch == self.store_epoch && !e.contested && e.owners.contains(&chat)
            })
            .map(|e| EvidenceView {
                chat_id: chat,
                store_epoch: e.store_epoch,
                canonical: e.canonical.clone(),
            })
    }

    /// Whether `open_binary` from `chat` in rea generation `generation` must first clear
    /// rea (R12 step 6): the last open was by another chat or in an older generation, the
    /// ledger holds another chat's ids in this epoch, or it was full.
    pub fn must_clear_before_open(&self, chat: i64, generation: u64) -> bool {
        let last = self
            .last_open
            .is_some_and(|(c, g)| c != chat || g != generation);
        last || self.full || self.others_hold_ids(chat)
    }

    /// Whether the current epoch holds result ids of a chat other than `chat`.
    pub fn others_hold_ids(&self, chat: i64) -> bool {
        self.evidence
            .values()
            .any(|e| e.store_epoch == self.store_epoch && e.owners.iter().any(|&c| c != chat))
    }

    /// `open_binary` succeeded for `chat`: it holds the file now (the counter grows); a
    /// previous holder that is another chat is told why its file closed.
    pub fn bind(
        &mut self,
        chat: i64,
        canonical: String,
        identity: Option<FileIdentity>,
        decompiler: Decompiler,
        generation: u64,
        open_args: serde_json::Map<String, serde_json::Value>,
    ) {
        if let Some(old) = &self.binding
            && old.view.chat_id != chat
        {
            self.set_closed(old.view.chat_id, ClosedReason::OtherChat);
        }
        self.open_seq += 1;
        self.closed.remove(&chat);
        self.last_open = Some((chat, generation));
        let folder = identity.is_none();
        self.binding = Some(Binding {
            view: BindingView {
                chat_id: chat,
                canonical,
                identity: identity.unwrap_or(FOLDER_IDENTITY),
                decompiler,
                generation,
                open_seq: self.open_seq,
            },
            folder,
            open_args,
        });
    }

    /// `open_binary` was sent but failed (rea may have closed what it had): nobody holds a
    /// file now. `generation` is rea's; the open still counts as the last one.
    pub fn open_failed(&mut self, chat: i64, generation: u64) {
        self.end_binding(Some(ClosedReason::OtherChat), chat);
        self.last_open = Some((chat, generation));
    }

    /// The model closed its file (`close_binary`): nobody holds one now; rea keeps the
    /// results.
    pub fn closed_by_model(&mut self) {
        self.open_seq += 1;
        self.binding = None;
    }

    /// The app sent `close_binary` and `reset_investigation`: no file is open and rea holds
    /// no results; every recorded id is gone. The holder, if another chat than `for_chat`,
    /// is told `reason`.
    pub fn cleared(&mut self, reason: ClosedReason, for_chat: i64) {
        self.end_binding(Some(reason), for_chat);
        self.store_epoch += 1;
        self.forget_ids();
        self.last_open = None;
    }

    /// rea started (a new process): its results are gone; the binding stays, so the chat
    /// that held the file can have it opened again (R21).
    pub fn rea_started(&mut self) {
        self.store_epoch += 1;
        self.forget_ids();
    }

    /// rea is not running (an idle stop, a crash): its results are gone. The binding
    /// stays for the reopen (R21), as after a start.
    pub fn rea_stopped(&mut self) {
        self.store_epoch += 1;
        self.forget_ids();
    }

    /// The binding ended because of a change in Settings (a decompiler switch, an update, a
    /// removal) or the app quitting: its chat reads `SettingsChanged`.
    pub fn settings_changed(&mut self) {
        if let Some(b) = self.binding.take() {
            self.open_seq += 1;
            self.set_closed(b.view.chat_id, ClosedReason::SettingsChanged);
        }
    }

    /// The binding ended because its file left the chat's folders or changed on disk: the
    /// chat itself is told by the call that found out, so no closed reason is kept.
    pub fn dropped(&mut self) {
        if self.binding.take().is_some() {
            self.open_seq += 1;
        }
    }

    /// Records the ids of a successful result of `chat` about `canonical`. An id already
    /// recorded in this epoch about the same file gains `chat` as an owner; one recorded
    /// about another file is contested. A full ledger leaves new records out.
    pub fn record(&mut self, chat: i64, canonical: &str, ids: &[String]) {
        for id in ids {
            if let Some(e) = self.evidence.get_mut(id)
                && e.store_epoch == self.store_epoch
            {
                if e.canonical != canonical {
                    e.contested = true;
                } else if !e.owners.contains(&chat) {
                    if self.slots < MAX_IDS {
                        e.owners.push(chat);
                        self.slots += 1;
                    } else {
                        self.full = true;
                    }
                }
                continue;
            }
            // A stale entry of an older epoch (never kept today: each new epoch empties the
            // ledger) is forgotten before recording again.
            if let Some(old) = self.evidence.remove(id) {
                self.slots -= old.owners.len();
            }
            if self.slots >= MAX_IDS {
                self.full = true;
                continue;
            }
            self.evidence.insert(
                id.clone(),
                Entry {
                    owners: vec![chat],
                    store_epoch: self.store_epoch,
                    canonical: canonical.to_string(),
                    contested: false,
                },
            );
            self.slots += 1;
        }
    }

    /// Ids seen in an error result about `canonical` (never recorded): one recorded about
    /// another file is contested, as for a successful result.
    pub fn saw_in_error(&mut self, canonical: &str, ids: &[String]) {
        for id in ids {
            if let Some(e) = self.evidence.get_mut(id)
                && e.store_epoch == self.store_epoch
                && e.canonical != canonical
            {
                e.contested = true;
            }
        }
    }

    /// How many ids are recorded (every epoch).
    pub fn recorded(&self) -> usize {
        self.evidence.len()
    }

    /// Whether an id was left out because the ledger was full (until rea is cleared).
    pub fn is_full(&self) -> bool {
        self.full
    }

    fn forget_ids(&mut self) {
        self.evidence.clear();
        self.slots = 0;
        self.full = false;
    }

    fn end_binding(&mut self, reason: Option<ClosedReason>, for_chat: i64) {
        self.open_seq += 1;
        if let Some(b) = self.binding.take()
            && b.view.chat_id != for_chat
            && let Some(reason) = reason
        {
            self.set_closed(b.view.chat_id, reason);
        }
    }

    fn set_closed(&mut self, chat: i64, reason: ClosedReason) {
        if self.closed.insert(chat, reason).is_none() {
            self.closed_order.push_back(chat);
        }
        while self.closed_order.len() > MAX_CLOSED {
            if let Some(old) = self.closed_order.pop_front() {
                self.closed.remove(&old);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u32) -> String {
        format!("ev_{n:064x}")
    }

    fn ident(n: u64) -> FileIdentity {
        FileIdentity {
            device: 1,
            file: n,
            size: 10,
            modified: 5,
        }
    }

    fn open(s: &mut Session, chat: i64, path: &str, generation: u64) {
        s.bind(
            chat,
            path.into(),
            Some(ident(1)),
            Decompiler::Kuna,
            generation,
            serde_json::Map::new(),
        );
    }

    #[test]
    fn binding_and_the_open_counter() {
        let mut s = Session::new();
        assert!(s.snapshot_for(1).binding.is_none());
        open(&mut s, 1, "/a/x", 1);
        let v = s.snapshot_for(1).binding.unwrap();
        assert_eq!((v.chat_id, v.open_seq, v.generation), (1, 1, 1));
        assert_eq!(s.binding_of(1).unwrap().view.canonical, "/a/x");
        assert!(s.binding_of(2).is_none());
        // Another chat's snapshot shows the binding (describe checks the chat).
        assert_eq!(s.snapshot_for(2).binding.unwrap().chat_id, 1);
        s.closed_by_model();
        assert_eq!(s.open_seq(), 2);
        assert!(s.binding().is_none());
        assert_eq!(s.closed_reason(1), None);
        assert_eq!(s.last_open(), Some((1, 1)));
    }

    #[test]
    fn another_chat_opening_closes_the_first_with_its_reason() {
        let mut s = Session::new();
        open(&mut s, 1, "/a/x", 1);
        assert!(!s.must_clear_before_open(1, 1));
        assert!(s.must_clear_before_open(2, 1));
        s.cleared(ClosedReason::OtherChat, 2);
        assert_eq!(s.closed_reason(1), Some(ClosedReason::OtherChat));
        open(&mut s, 2, "/b/y", 1);
        assert_eq!(
            s.snapshot_for(1).closed_reason(1),
            Some(ClosedReason::OtherChat)
        );
        // Chat 2's snapshot does not carry chat 1's reason.
        assert_eq!(s.snapshot_for(2).closed_reason(1), None);
        // Opening again clears chat 1's reason.
        s.cleared(ClosedReason::OtherChat, 1);
        open(&mut s, 1, "/a/x", 1);
        assert_eq!(s.closed_reason(1), None);
        assert_eq!(s.closed_reason(2), Some(ClosedReason::OtherChat));
    }

    #[test]
    fn an_older_generation_or_another_chats_ids_need_clearing() {
        let mut s = Session::new();
        assert!(!s.must_clear_before_open(1, 1));
        open(&mut s, 1, "/a/x", 1);
        assert!(s.must_clear_before_open(1, 2), "older generation");
        s.record(2, "/b/check", &[id(7)]);
        assert!(s.must_clear_before_open(1, 1), "chat 2's ids in this epoch");
        s.rea_started();
        // A new epoch: chat 2's ids are gone; the last open is still of generation 1.
        assert!(!s.must_clear_before_open(1, 1));
        assert!(s.must_clear_before_open(1, 2));
    }

    #[test]
    fn ids_belong_to_the_first_chat_that_saw_them() {
        let mut s = Session::new();
        s.record(1, "/a/x", &[id(1), id(2)]);
        s.record(2, "/b/y", &[id(3)]);
        assert_eq!(s.evidence_for(1, &id(2)).unwrap().canonical, "/a/x");
        assert!(s.evidence_for(2, &id(2)).is_none());
        assert_eq!(s.evidence_for(2, &id(3)).unwrap().chat_id, 2);
        let v1 = s.snapshot_for(1);
        assert!(v1.evidence(&id(1)).is_some() && v1.evidence(&id(2)).is_some());
        assert!(v1.evidence(&id(3)).is_none());
        let v2 = s.snapshot_for(2);
        assert!(v2.evidence(&id(2)).is_none());
        assert_eq!(v2.evidence(&id(3)).unwrap().canonical, "/b/y");
        assert!(s.others_hold_ids(1) && s.others_hold_ids(2) && s.others_hold_ids(3));
        let mut one = Session::new();
        one.record(1, "/a/x", &[id(1)]);
        assert!(!one.others_hold_ids(1) && one.others_hold_ids(2));
    }

    /// rea's ids are digests of the content: another chat that checks the same file gets
    /// the same ids, and may read them (rea's record is about a file it can read).
    #[test]
    fn the_same_file_in_another_chat_shares_its_ids() {
        let mut s = Session::new();
        s.record(1, "/a/x", &[id(1)]);
        s.record(2, "/a/x", &[id(1)]);
        let v2 = s.snapshot_for(2);
        let e = v2.evidence(&id(1)).expect("chat 2 may read it");
        assert_eq!((e.chat_id, e.canonical.as_str()), (2, "/a/x"));
        assert_eq!(s.evidence_for(1, &id(1)).unwrap().chat_id, 1);
        // Recorded again by the same chat: nothing changes.
        s.record(2, "/a/x", &[id(1)]);
        assert_eq!(s.slots, 2);
    }

    /// An id seen in a result about another file (a hostile file can show an id rea gave,
    /// or will give, about another one) is read by nobody, whichever chat saw it.
    #[test]
    fn an_id_seen_about_another_file_is_contested() {
        let mut s = Session::new();
        s.record(1, "/a/x", &[id(1), id(2), id(3)]);
        s.record(2, "/b/hostile", &[id(1)]);
        assert!(s.evidence_for(1, &id(1)).is_none());
        assert!(s.evidence_for(2, &id(1)).is_none());
        assert!(s.snapshot_for(1).evidence(&id(1)).is_none());
        // The same chat with another file contests too (rea keeps the first record only).
        s.record(1, "/a/copy", &[id(2)]);
        assert!(s.evidence_for(1, &id(2)).is_none());
        // An error result contests without recording.
        s.saw_in_error("/c/z", &[id(3), id(9)]);
        assert!(s.evidence_for(1, &id(3)).is_none());
        assert!(s.evidence_for(3, &id(9)).is_none());
        assert_eq!(s.recorded(), 3);
        // Contested ids still mean another chat's results are in rea.
        assert!(s.others_hold_ids(2));
        // A new epoch forgets the contest.
        s.rea_started();
        s.record(2, "/b/hostile", &[id(1)]);
        assert_eq!(s.evidence_for(2, &id(1)).unwrap().canonical, "/b/hostile");
    }

    #[test]
    fn a_new_epoch_forgets_every_id() {
        let mut s = Session::new();
        s.record(1, "/a/x", &[id(1)]);
        let epoch = s.store_epoch();
        s.rea_started();
        assert_eq!(s.store_epoch(), epoch + 1);
        assert!(s.evidence_for(1, &id(1)).is_none());
        assert!(s.snapshot_for(1).evidence(&id(1)).is_none());
        // After the restart the id may be seen again, by whichever chat sees it first.
        s.record(2, "/b/y", &[id(1)]);
        assert_eq!(s.evidence_for(2, &id(1)).unwrap().chat_id, 2);
        assert!(s.evidence_for(1, &id(1)).is_none());
        s.cleared(ClosedReason::OtherChat, 1);
        assert!(s.evidence_for(2, &id(1)).is_none());
        assert_eq!(s.recorded(), 0);
        assert_eq!(s.last_open(), None);
    }

    /// A full ledger leaves new ids out and never drops one: a dropped id could be given
    /// to another chat by a later result (security review, WP9g r1). The next open
    /// clears rea.
    #[test]
    fn a_full_ledger_drops_nothing_and_clears_at_the_next_open() {
        let mut s = Session::new();
        let mine: Vec<String> = (0..5).map(id).collect();
        s.record(1, "/a/x", &mine);
        let junk: Vec<String> = (100..(100 + MAX_IDS as u32)).map(id).collect();
        s.record(2, "/b/hostile", &junk);
        assert_eq!(s.recorded(), MAX_IDS);
        assert!(s.is_full());
        // Chat 1's ids are all still chat 1's; chat 2 got no more than the room left.
        for i in &mine {
            assert_eq!(s.evidence_for(1, i).unwrap().canonical, "/a/x");
            assert!(s.evidence_for(2, i).is_none());
        }
        assert!(s.evidence_for(2, &id(100)).is_some());
        assert!(s.evidence_for(2, &id(100 + MAX_IDS as u32 - 1)).is_none());
        // 10,001st: chat 2's result shows chat 1's id after the junk: still chat 1's,
        // never chat 2's, and the result about another file contests it.
        s.record(2, "/b/hostile", &[id(0)]);
        assert!(s.evidence_for(2, &id(0)).is_none());
        assert!(s.evidence_for(1, &id(0)).is_none(), "contested");
        assert_eq!(s.evidence_for(1, &id(1)).unwrap().chat_id, 1);
        // The same file in chat 3 adds no record while full.
        s.record(3, "/a/x", &[id(1)]);
        assert!(s.evidence_for(3, &id(1)).is_none());
        // The next open clears rea, whoever opens; then ids are recorded again.
        assert!(s.must_clear_before_open(1, 1));
        s.cleared(ClosedReason::OtherChat, 1);
        assert!(!s.is_full());
        s.record(1, "/a/x", &[id(1)]);
        assert_eq!(s.evidence_for(1, &id(1)).unwrap().chat_id, 1);
        let mut t = Session::new();
        t.record(
            1,
            "/a/x",
            &(0..(MAX_IDS as u32 + 1)).map(id).collect::<Vec<_>>(),
        );
        assert!(t.is_full());
        assert!(
            t.must_clear_before_open(1, 0),
            "full, even for the same chat"
        );
        t.cleared(ClosedReason::OtherChat, 1);
        assert!(!t.is_full() && t.recorded() == 0);
    }

    #[test]
    fn settings_changes_and_drops() {
        let mut s = Session::new();
        open(&mut s, 3, "/c/z", 1);
        let seq = s.open_seq();
        s.settings_changed();
        assert_eq!(s.closed_reason(3), Some(ClosedReason::SettingsChanged));
        assert_eq!(s.open_seq(), seq + 1);
        assert!(s.binding().is_none());
        s.settings_changed();
        assert_eq!(s.open_seq(), seq + 1, "nothing to end");
        open(&mut s, 3, "/c/z", 1);
        s.dropped();
        assert!(s.binding().is_none());
        assert_eq!(s.closed_reason(3), None);
    }

    #[test]
    fn a_failed_open_ends_the_binding() {
        let mut s = Session::new();
        open(&mut s, 1, "/a/x", 1);
        let seq = s.open_seq();
        s.open_failed(2, 1);
        assert!(s.binding().is_none());
        assert_eq!(s.open_seq(), seq + 1);
        assert_eq!(s.closed_reason(1), Some(ClosedReason::OtherChat));
        assert_eq!(s.last_open(), Some((2, 1)));
        open(&mut s, 1, "/a/x", 1);
        s.open_failed(1, 1);
        assert_eq!(s.closed_reason(1), None);
    }

    #[test]
    fn a_folder_binding_shows_the_placeholder_identity() {
        let mut s = Session::new();
        s.bind(
            1,
            "/Apps/X.app".into(),
            None,
            Decompiler::Kuna,
            1,
            serde_json::Map::new(),
        );
        let b = s.binding().unwrap();
        assert!(b.folder);
        assert_eq!(b.view.identity, FOLDER_IDENTITY);
    }

    #[test]
    fn closed_reasons_are_capped() {
        let mut s = Session::new();
        for chat in 0..(MAX_CLOSED as i64 + 10) {
            open(&mut s, chat, "/a/x", 1);
            s.settings_changed();
        }
        assert!(s.closed_reason(0).is_none());
        assert_eq!(
            s.closed_reason(MAX_CLOSED as i64 + 9),
            Some(ClosedReason::SettingsChanged)
        );
    }
}
