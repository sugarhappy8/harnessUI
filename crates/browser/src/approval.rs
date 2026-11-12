//! Approvals raised during a call (ARCHITECTURE.md 13.3): the handle the app gives the
//! browser server for navigations it holds in CDP.
//!
//! A held navigation that matches the submit intent approved for the call in progress is
//! released without asking again. Any other held navigation raises its own always-ask
//! card through [`NavigationApprover`], whether or not a call is running, and is failed
//! on Deny, Stop or timeout. Each decision is its own audit row, linked by the app to the
//! originating call.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::intent::HoldKind;

/// What the approval card shows for one held request. Every string is sanitised for the
/// card: control, bidirectional and zero-width characters are visible escapes, and the
/// host is punycode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeldNavigation {
    /// Random id of this hold, for the audit row.
    pub id: String,
    /// Why it is held.
    pub kind: HoldKind,
    /// HTTP method (`POST`, `GET`…).
    pub method: String,
    /// The destination host in punycode, with a non-default port.
    pub host: String,
    /// The destination origin (`scheme://host[:port]`).
    pub origin: String,
    /// The full destination URL, escaped for display.
    pub url: String,
    /// The origin of the page that made the request, if known.
    pub from_origin: Option<String>,
    /// The request is for the tab's main frame (not an embedded frame).
    pub top_level: bool,
    /// The built-in tool whose call was running when the request was held, if any.
    pub during_tool: Option<String>,
    /// One line for the card.
    pub summary: String,
    /// The full preview for the card.
    pub preview: String,
}

/// The longest URL shown on a card, in characters.
const MAX_URL: usize = 2000;

impl HeldNavigation {
    /// Builds the card text for a request to `url`.
    pub fn new(
        kind: HoldKind,
        method: &str,
        url: &Url,
        from_origin: Option<&str>,
        top_level: bool,
        during_tool: Option<&str>,
    ) -> Self {
        let host = policy::sanitize::display_host(url).unwrap_or_default();
        let origin = policy::sanitize::origin(url).unwrap_or_else(|| "null".into());
        let method = policy::sanitize::one_line(&method.to_ascii_uppercase(), 16);
        let shown_url = policy::sanitize::one_line(url.as_str(), MAX_URL);
        let from = from_origin.map(|o| policy::sanitize::one_line(o, 200));
        let summary = match kind {
            HoldKind::Submission => format!("Submit to {host} ({method})"),
            HoldKind::LeavesSite => format!("Open {host} (leaves the current site)"),
            HoldKind::Background => format!("Send data to {host} ({method}, background)"),
        };
        let why = match kind {
            HoldKind::Submission => {
                "The page is submitting a form or sending data with a navigation. It can send what was typed on the page."
            }
            HoldKind::LeavesSite => "The page is moving the tab to a different site.",
            HoldKind::Background => {
                "Strict mode is on for this site, and the page is sending data in the background."
            }
        };
        let mut preview = format!("{why}\n{method} {shown_url}\nDestination: {origin}");
        if let Some(from) = &from {
            preview.push_str(&format!("\nFrom page: {from}"));
        }
        if !top_level {
            preview.push_str("\nRequested by an embedded frame.");
        }
        if let Some(tool) = during_tool {
            preview.push_str(&format!(
                "\nRequested while {} was running.",
                policy::sanitize::one_line(tool, 64)
            ));
        }
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            kind,
            method,
            host,
            origin,
            url: shown_url,
            from_origin: from,
            top_level,
            during_tool: during_tool.map(str::to_string),
            summary: policy::sanitize::one_line(&summary, 200),
            preview: policy::sanitize::visible(&preview),
        }
    }
}

impl HeldNavigation {
    /// Allow all (0.0.4): whether the app may let this held request through without a
    /// card while its chat is in Allow all. Only a page moving its tab to another site
    /// during a call, and only while no untrusted content had entered the turn when that
    /// call was decided: then nothing the model read can have chosen the destination.
    ///
    /// Everything else keeps its card in every mode. A held submission was not announced
    /// by the action the model described (one that was is released by its intent, with no
    /// card), it has no label to judge, and it can send what was typed. A request held
    /// outside a call was started by the page alone.
    pub fn released_in_allow_all(&self, untrusted_in_turn: bool) -> bool {
        self.kind == HoldKind::LeavesSite
            && self.top_level
            && self.during_tool.is_some()
            && !untrusted_in_turn
    }
}

/// Implemented by the app: shows an always-ask card (Allow once / Deny) for a held
/// navigation and returns the user's answer. `cancel` fires on Stop of the call in
/// progress, on the approval timeout and when the browser shuts down; the card must then
/// be resolved as cancelled and the method return `false`.
#[async_trait]
pub trait NavigationApprover: Send + Sync {
    /// `true` lets the request through; `false` fails it.
    async fn approve(
        &self,
        chat_id: i64,
        request: HeldNavigation,
        cancel: CancellationToken,
    ) -> bool;
}

/// An approver that denies everything (no card can be shown).
#[derive(Debug, Default, Clone, Copy)]
pub struct DenyAll;

#[async_trait]
impl NavigationApprover for DenyAll {
    async fn approve(&self, _: i64, _: HeldNavigation, _: CancellationToken) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_text_is_sanitised_and_punycode() {
        let url = Url::parse("https://bücher.example/pay?to=\u{202E}evil").unwrap();
        let held = HeldNavigation::new(
            HoldKind::Submission,
            "post",
            &url,
            Some("https://shop.example"),
            true,
            Some("browser_click"),
        );
        assert_eq!(held.host, "xn--bcher-kva.example");
        assert_eq!(held.origin, "https://xn--bcher-kva.example");
        assert_eq!(held.method, "POST");
        assert_eq!(held.summary, "Submit to xn--bcher-kva.example (POST)");
        assert!(
            held.preview
                .contains("POST https://xn--bcher-kva.example/pay")
        );
        assert!(held.preview.contains("From page: https://shop.example"));
        assert!(held.preview.contains("browser_click"));
        // The URL parser percent-encodes the override; no raw control character survives.
        assert!(!held.preview.contains('\u{202E}'));
        assert!(!held.url.contains('\u{202E}'));
        assert_eq!(held.id.len(), 36);
    }

    #[test]
    fn kinds_have_their_own_wording() {
        let url = Url::parse("http://other.example:8080/").unwrap();
        let leave = HeldNavigation::new(HoldKind::LeavesSite, "GET", &url, None, true, None);
        assert_eq!(
            leave.summary,
            "Open other.example:8080 (leaves the current site)"
        );
        let bg = HeldNavigation::new(HoldKind::Background, "PUT", &url, None, false, None);
        assert!(bg.summary.contains("background"));
        assert!(bg.preview.contains("embedded frame"));
    }

    /// Allow all: only a move to another site during a call, before anything untrusted
    /// entered the turn, is let through without a card.
    #[test]
    fn allow_all_releases_only_a_move_to_another_site_before_untrusted_content() {
        let url = Url::parse("https://other.example/").unwrap();
        let held = |kind: HoldKind, top_level: bool, tool: Option<&str>| {
            HeldNavigation::new(
                kind,
                "GET",
                &url,
                Some("https://a.example"),
                top_level,
                tool,
            )
        };
        let leaves = held(HoldKind::LeavesSite, true, Some("browser_navigate"));
        assert!(leaves.released_in_allow_all(false));
        assert!(
            !leaves.released_in_allow_all(true),
            "after untrusted content a move to another site asks"
        );
        // Outside a call the page acted alone.
        assert!(!held(HoldKind::LeavesSite, true, None).released_in_allow_all(false));
        assert!(
            !held(HoldKind::LeavesSite, false, Some("browser_click")).released_in_allow_all(false)
        );
        // Submissions and background sends always ask.
        for kind in [HoldKind::Submission, HoldKind::Background] {
            for untrusted in [false, true] {
                assert!(
                    !held(kind, true, Some("browser_click")).released_in_allow_all(untrusted),
                    "{kind:?}"
                );
            }
        }
    }

    #[tokio::test]
    async fn deny_all_denies() {
        let url = Url::parse("https://a.example/").unwrap();
        let held = HeldNavigation::new(HoldKind::Submission, "POST", &url, None, true, None);
        assert!(!DenyAll.approve(1, held, CancellationToken::new()).await);
    }
}
