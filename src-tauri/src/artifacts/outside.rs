//! Whether content from outside the conversation has entered a chat, and which pages
//! the user has allowed to run their scripts all the same.
//!
//! A page the model writes is what the model was told to write. Once the model has read
//! a file, a web page or the result of a tool, what it writes may be what a stranger told
//! it to write (ARCHITECTURE.md 13.5). A preview page can make no request, but a script
//! in it can still open a connection whose host name carries data. So a page written
//! after such content entered the chat is shown without its scripts until the user says
//! otherwise (`protocol::running`).
//!
//! "Entered the chat" is decided here, by the app, from what tools ran: nothing the model
//! writes can clear it.

use std::collections::HashSet;
use std::sync::Mutex;

use agent::history::{HistoryMessage, Part};

/// Tools whose results are the app's own data about the chat. The result of every other
/// tool is content from outside the conversation. A wrapped call (`call_tool`) and a
/// stored result read again (`read_output`) are deliberately not here: what they return
/// is some other tool's result.
const OWN_DATA_TOOLS: &[&str] = &[
    "create_artifact",
    "update_artifact",
    "read_artifact",
    "list_artifacts",
    "find_tools",
    "update_task",
];

/// Whether a result of the tool `name` is the app's own data about the chat.
pub fn is_own_data_tool(name: &str) -> bool {
    OWN_DATA_TOOLS.contains(&name)
}

/// The chats that content from outside the conversation has entered since the app
/// started. A chat's stored history is looked at when a turn starts ([`in_history`]), so
/// this does not have to outlive the app.
#[derive(Default)]
pub struct Outside(Mutex<HashSet<i64>>);

impl Outside {
    pub fn note(&self, chat: i64) {
        if let Ok(mut chats) = self.0.lock() {
            chats.insert(chat);
        }
    }

    /// Whether outside content has entered the chat. A state that cannot be read counts
    /// as yes.
    pub fn seen(&self, chat: i64) -> bool {
        self.0.lock().map(|chats| chats.contains(&chat)).unwrap_or(true)
    }
}

/// Whether a chat's history holds the result of a tool that brought in outside content.
/// A call that did not run has no result text and counts for nothing.
pub fn in_history(history: &[HistoryMessage]) -> bool {
    history.iter().flat_map(|m| &m.parts).any(|part| match part {
        Part::ToolResult(r) => {
            !is_own_data_tool(&r.name) && (!r.text.is_empty() || !r.images.is_empty())
        }
        _ => false,
    })
}

/// The versions whose scripts the user allowed although they were written after outside
/// content. Kept in memory, like grants (D12): it ends when the app quits.
#[derive(Default)]
pub struct Allowed(Mutex<HashSet<(i64, u32)>>);

impl Allowed {
    pub fn allow(&self, id: i64, version: u32) {
        if let Ok(mut versions) = self.0.lock() {
            versions.insert((id, version));
        }
    }

    /// Whether the user allowed this version's scripts. A state that cannot be read
    /// counts as no.
    pub fn is_allowed(&self, id: i64, version: u32) -> bool {
        self.0
            .lock()
            .map(|versions| versions.contains(&(id, version)))
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent::Role;
    use agent::history::{CallStatus, ResultRecord};

    fn result(name: &str, text: &str) -> HistoryMessage {
        HistoryMessage {
            role: Role::Tool,
            parts: vec![Part::ToolResult(ResultRecord {
                call_id: "c1".into(),
                name: name.into(),
                status: CallStatus::Ok,
                text: text.into(),
                images: Vec::new(),
                source: None,
                notes: Vec::new(),
            })],
            reasoning: None,
            id: None,
        }
    }

    fn said(role: Role, text: &str) -> HistoryMessage {
        HistoryMessage {
            role,
            parts: vec![Part::Text(text.into())],
            reasoning: None,
            id: None,
        }
    }

    #[test]
    fn a_chat_is_marked_once_and_for_good() {
        let outside = Outside::default();
        assert!(!outside.seen(1));
        outside.note(1);
        assert!(outside.seen(1));
        assert!(!outside.seen(2), "another chat is not marked");
        outside.note(1);
        assert!(outside.seen(1));
    }

    #[test]
    fn only_results_of_tools_that_read_the_outside_count() {
        // What the user and the model say is the conversation itself.
        assert!(!in_history(&[
            said(Role::User, "make me a timer page"),
            said(Role::Assistant, "Here it is."),
        ]));
        // The app's own data about the chat is not outside content.
        for own in OWN_DATA_TOOLS {
            assert!(!in_history(&[result(own, "Artifact #1 saved.")]), "{own}");
        }
        // Every other tool's result is, also one the app does not know.
        for tool in [
            "read_file",
            "run_command",
            "web_search",
            "read_source",
            "browser_navigate",
            "call_tool",
            "read_output",
            "run_subagent",
            "load_skill",
            "some_server__some_tool",
        ] {
            assert!(in_history(&[result(tool, "text")]), "{tool}");
        }
        // A call that did not run brought nothing in.
        assert!(!in_history(&[result("read_file", "")]));
        // One such result anywhere in the history is enough.
        assert!(in_history(&[
            said(Role::User, "read notes.txt"),
            result("read_file", "notes"),
            said(Role::Assistant, "Done."),
            said(Role::User, "now make a page"),
        ]));
    }

    #[test]
    fn the_user_allows_one_version_at_a_time() {
        let allowed = Allowed::default();
        assert!(!allowed.is_allowed(3, 2));
        allowed.allow(3, 2);
        assert!(allowed.is_allowed(3, 2));
        assert!(!allowed.is_allowed(3, 3), "the next version is a different page");
        assert!(!allowed.is_allowed(4, 2));
    }
}
