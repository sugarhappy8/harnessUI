//! The form-submission gate (ARCHITECTURE.md D8, 9.3) and the navigation hold rules
//! (13.3 "Approvals raised during a call"). Pure logic, no I/O.
//!
//! 1. *Intent, before the action* ([`click_intent`], [`enter_intent`]): a click that
//!    resolves to a submit control, Enter in a form field, any button click or Enter after
//!    the agent typed on the page, any button whose label is a consequential verb. Each
//!    makes the action always-ask.
//! 2. *Enforcement* ([`decide`]): every non-GET navigation and every form-submission
//!    navigation is held until approved. A held navigation that matches the submit intent
//!    approved for the call in progress ([`intent_matches`]) is released without a second
//!    prompt; any other raises its own always-ask approval. A top-level navigation that
//!    leaves the granted site is held too.

use serde::{Deserialize, Serialize};

/// What the tool learned about an element from the page, in an isolated world the page's
/// scripts cannot reach. Labels and attributes are still chosen by the page.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ElementInfo {
    /// Lowercase tag name.
    pub tag: String,
    /// Lowercase `type` attribute (inputs and buttons).
    pub input_type: String,
    /// Lowercase `role` attribute.
    pub role: String,
    /// The visible label: aria-label, value, text, title or alt, trimmed.
    pub label: String,
    /// A submit button or submit/image input that belongs to a form.
    pub is_submit_control: bool,
    /// Inside a form, or associated with one.
    pub in_form: bool,
    /// The submission method (`get`, `post`, `dialog`), if a form is involved.
    pub form_method: Option<String>,
    /// The absolute submission URL, if a form is involved.
    pub form_action: Option<String>,
    /// A button, a button-like input, or an element with role button or menu item.
    pub is_button_like: bool,
    /// A text field, text area or editable element.
    pub is_editable: bool,
    /// A link with an `href`.
    pub is_link: bool,
    /// The link's absolute `href`.
    pub href: Option<String>,
    /// Disabled controls do nothing when clicked.
    pub disabled: bool,
    /// The focused element is inside a frame the tool cannot look into.
    pub opaque_frame: bool,
}

/// Why an action may submit something.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "detail")]
pub enum SubmitReason {
    /// The click resolves to a form's submit control.
    SubmitControl,
    /// Enter pressed in a form field.
    EnterInFormField,
    /// A button click after the agent typed on this page.
    ButtonAfterTyping,
    /// Enter after the agent typed on this page.
    EnterAfterTyping,
    /// A button whose label is a consequential verb (the verb).
    ConsequentialLabel(String),
    /// Focus is in an embedded frame the tool cannot inspect.
    OpaqueFocus,
}

impl SubmitReason {
    /// One sentence for the approval card.
    pub fn describe(&self) -> String {
        match self {
            Self::SubmitControl => "This clicks a form's submit button.".into(),
            Self::EnterInFormField => "Enter in a form field usually submits the form.".into(),
            Self::ButtonAfterTyping => {
                "The agent typed on this page; a button click now may send what was typed."
                    .into()
            }
            Self::EnterAfterTyping => {
                "The agent typed on this page; Enter now may send what was typed.".into()
            }
            Self::ConsequentialLabel(verb) => {
                format!("The button's label says \"{verb}\", which usually acts on your behalf.")
            }
            Self::OpaqueFocus => {
                "Focus is inside an embedded frame the tool cannot inspect, so this key may submit something or press a button there.".into()
            }
        }
    }
}

/// Labels that usually commit something on the user's behalf. Matched as whole words or
/// phrases after lower-casing and replacing punctuation with spaces.
// TODO(M8): English only; labels in other languages are caught only by the submit-control
// and typed-on-page rules and by the request hold.
const CONSEQUENTIAL: &[&str] = &[
    "buy",
    "buy now",
    "pay",
    "pay now",
    "purchase",
    "checkout",
    "check out",
    "place order",
    "order",
    "send",
    "submit",
    "delete",
    "remove",
    "confirm",
    "sign up",
    "signup",
    "register",
    "log in",
    "login",
    "sign in",
    "signin",
    "post",
    "publish",
    "transfer",
    "donate",
    "subscribe",
    "book",
    "reserve",
    "apply",
    "authorize",
    "authorise",
    "approve",
    "withdraw",
    "reply",
    "share",
];

fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// The consequential verb in `label`, if any (whole words only: "Payment details" does
/// not match "pay").
pub fn consequential_verb(label: &str) -> Option<&'static str> {
    let label_words = words(label);
    if label_words.is_empty() {
        return None;
    }
    // Longest phrases first, so "place order" wins over "order".
    let mut verbs: Vec<&'static str> = CONSEQUENTIAL.to_vec();
    verbs.sort_by_key(|v| std::cmp::Reverse(v.len()));
    verbs.into_iter().find(|verb| {
        let verb_words = words(verb);
        label_words
            .windows(verb_words.len())
            .any(|window| window == verb_words.as_slice())
    })
}

/// D8 intent for a click on `el`. `typed` says whether the agent typed on this page.
pub fn click_intent(el: &ElementInfo, typed: bool) -> Option<SubmitReason> {
    if el.disabled {
        return None;
    }
    if el.is_submit_control {
        return Some(SubmitReason::SubmitControl);
    }
    if el.is_button_like && typed {
        return Some(SubmitReason::ButtonAfterTyping);
    }
    if el.is_button_like
        && let Some(verb) = consequential_verb(&el.label)
    {
        return Some(SubmitReason::ConsequentialLabel(verb.to_string()));
    }
    None
}

/// D8 intent for a key press, given the focused element (`None` when nothing in
/// particular has focus). Enter in a form field, Enter after typing, and Enter or Space on
/// a button that [`click_intent`] would flag.
///
/// The key is judged as [`crate::keys::parse_key`] reads it, which is exactly what
/// `browser_press_key` presses: `" Enter"` or `"shift + enter"` are Enter too. A key that
/// does not parse is never pressed, so it has no intent.
pub fn key_intent(key: &str, focused: Option<&ElementInfo>, typed: bool) -> Option<SubmitReason> {
    let Ok(press) = crate::keys::parse_key(key) else {
        return None;
    };
    let is_enter = press.key == "Enter";
    let is_space = press.key == " ";
    if !is_enter && !is_space {
        return None;
    }
    if let Some(el) = focused {
        // Focus inside a frame the tool cannot look into: Enter may submit, and Space
        // presses whatever button has the focus there, whose label nobody can check.
        if el.opaque_frame {
            return Some(SubmitReason::OpaqueFocus);
        }
        if el.is_button_like || el.is_submit_control {
            return click_intent(el, typed);
        }
        if is_enter && el.is_editable && el.in_form {
            return Some(SubmitReason::EnterInFormField);
        }
    }
    if is_enter && typed {
        return Some(SubmitReason::EnterAfterTyping);
    }
    None
}

/// The submit intent the user approved with an always-ask card, carried in the action's
/// payload and matched against held navigations during the call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitIntent {
    /// Why the card was shown.
    pub reason: SubmitReason,
    /// The tab (target id) the action runs in.
    pub tab: String,
    /// The expected method, when a form is involved.
    pub method: Option<String>,
    /// Where the submission goes: the form's action origin, or the frame's own origin
    /// when the target is not known in advance.
    pub target_origin: String,
}

/// What [`decide`] needs to know about one paused request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PausedRequest {
    /// The tab (target id) the request belongs to.
    pub tab: String,
    /// The tab that opened this one, for pop-ups.
    pub opener_tab: Option<String>,
    /// HTTP method, upper case.
    pub method: String,
    /// The request's origin (`scheme://host[:port]`).
    pub origin: String,
    /// The main frame of its tab (not a sub-frame).
    pub top_level: bool,
    /// A document request (navigation), not XHR or fetch.
    pub is_document: bool,
    /// The page reported this navigation as a form submission.
    pub form_submission: bool,
    /// Strict mode is on for the frame's site (background requests are held too).
    pub strict: bool,
    /// The redirect hop of a request already released (its request id).
    pub redirect_of_released: bool,
}

impl PausedRequest {
    /// A submission in D8's sense: any non-GET navigation, or a form submission.
    pub fn is_submission(&self) -> bool {
        let safe = matches!(self.method.as_str(), "GET" | "HEAD" | "OPTIONS");
        if self.is_document {
            !safe || self.form_submission
        } else {
            self.strict && !safe
        }
    }
}

/// The call in progress, as far as held navigations are concerned.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallView {
    /// The approved submit intent, unless already used.
    pub intent: Option<SubmitIntent>,
    /// The origin an approved `browser_navigate` / `browser_back` goes to, and its tab.
    pub expected_navigation: Option<(String, String)>,
}

/// Why a request is held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HoldKind {
    /// A non-GET navigation or a form submission (D8).
    Submission,
    /// A top-level navigation that leaves the granted site.
    LeavesSite,
    /// A background non-GET request on a strict-mode site.
    Background,
}

/// What to do with a paused request whose scheme and address already passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Let it through.
    Continue,
    /// Let it through: it is the submission the user approved for this call (the intent
    /// is used up).
    ContinueUsingIntent,
    /// Let it through: it is the navigation the user approved for this call; the tab's
    /// granted site becomes its origin.
    ContinueApprovedNavigation,
    /// Hold it and ask.
    Hold(HoldKind),
}

/// Whether a held submission is the one approved for the call in progress.
pub fn intent_matches(intent: &SubmitIntent, req: &PausedRequest) -> bool {
    let same_tab = req.tab == intent.tab || req.opener_tab.as_deref() == Some(intent.tab.as_str());
    let method_ok = intent
        .method
        .as_deref()
        .is_none_or(|m| m.eq_ignore_ascii_case(&req.method));
    same_tab && method_ok && req.origin == intent.target_origin
}

/// `granted` with `http` upgraded to `https` on the same host and default port; an
/// upgrade sends nothing new anywhere.
fn is_https_upgrade(granted: &str, origin: &str) -> bool {
    granted
        .strip_prefix("http://")
        .zip(origin.strip_prefix("https://"))
        .is_some_and(|(a, b)| !a.contains(':') && a == b)
}

/// The hold rules for one paused request (scheme and address checks come first and are
/// not repeated here). `granted_site` is the site the tab's grant covers.
pub fn decide(req: &PausedRequest, granted_site: Option<&str>, call: &CallView) -> Verdict {
    if req.is_submission() {
        if let Some(intent) = &call.intent
            && intent_matches(intent, req)
        {
            return Verdict::ContinueUsingIntent;
        }
        if req.redirect_of_released && granted_site == Some(req.origin.as_str()) {
            // A 307/308 hop of a submission that was already approved, staying on the site.
            return Verdict::Continue;
        }
        return Verdict::Hold(if req.is_document {
            HoldKind::Submission
        } else {
            HoldKind::Background
        });
    }
    if !req.is_document || !req.top_level {
        return Verdict::Continue;
    }
    if let Some(granted) = granted_site
        && (granted == req.origin || is_https_upgrade(granted, &req.origin))
    {
        return Verdict::Continue;
    }
    if let Some((origin, tab)) = &call.expected_navigation
        && *origin == req.origin
        && (*tab == req.tab || req.opener_tab.as_ref() == Some(tab))
    {
        return Verdict::ContinueApprovedNavigation;
    }
    Verdict::Hold(HoldKind::LeavesSite)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn button(label: &str) -> ElementInfo {
        ElementInfo {
            tag: "button".into(),
            label: label.into(),
            is_button_like: true,
            ..Default::default()
        }
    }

    #[test]
    fn consequential_verbs_match_whole_words_only() {
        for (label, verb) in [
            ("Buy now", Some("buy now")),
            ("Pay", Some("pay")),
            ("Payment details", None),
            ("Send message", Some("send")),
            ("Submit", Some("submit")),
            ("DELETE ACCOUNT", Some("delete")),
            ("Confirm", Some("confirm")),
            ("Place order", Some("place order")),
            ("Order history", Some("order")),
            ("Sign up for free", Some("sign up")),
            ("Log-in", Some("log in")),
            ("Login", Some("login")),
            ("Post", Some("post")),
            ("Posting guidelines", None),
            ("Transfer funds", Some("transfer")),
            ("Proceed to checkout", Some("checkout")),
            ("Next", None),
            ("Search", None),
            ("Show more", None),
            ("", None),
        ] {
            assert_eq!(consequential_verb(label), verb, "{label}");
        }
    }

    #[test]
    fn submit_controls_always_ask() {
        let el = ElementInfo {
            tag: "button".into(),
            label: "Go".into(),
            is_submit_control: true,
            is_button_like: true,
            in_form: true,
            ..Default::default()
        };
        assert_eq!(click_intent(&el, false), Some(SubmitReason::SubmitControl));
    }

    #[test]
    fn button_after_typing_asks_but_not_before() {
        let el = button("Next");
        assert_eq!(click_intent(&el, false), None);
        assert_eq!(
            click_intent(&el, true),
            Some(SubmitReason::ButtonAfterTyping)
        );
        // A plain link after typing is not a button.
        let link = ElementInfo {
            tag: "a".into(),
            label: "Next".into(),
            is_link: true,
            ..Default::default()
        };
        assert_eq!(click_intent(&link, true), None);
    }

    #[test]
    fn consequential_buttons_ask_even_without_typing() {
        assert_eq!(
            click_intent(&button("Delete"), false),
            Some(SubmitReason::ConsequentialLabel("delete".into()))
        );
        // A link labelled "Delete" is a GET link; D8 lists those as not caught.
        let link = ElementInfo {
            tag: "a".into(),
            label: "Delete".into(),
            is_link: true,
            ..Default::default()
        };
        assert_eq!(click_intent(&link, false), None);
        let mut disabled = button("Delete");
        disabled.disabled = true;
        assert_eq!(click_intent(&disabled, true), None);
    }

    #[test]
    fn enter_rules() {
        let field = ElementInfo {
            tag: "input".into(),
            input_type: "text".into(),
            is_editable: true,
            in_form: true,
            ..Default::default()
        };
        assert_eq!(
            key_intent("Enter", Some(&field), false),
            Some(SubmitReason::EnterInFormField)
        );
        let loose_field = ElementInfo {
            in_form: false,
            ..field.clone()
        };
        assert_eq!(key_intent("Enter", Some(&loose_field), false), None);
        assert_eq!(
            key_intent("Enter", Some(&loose_field), true),
            Some(SubmitReason::EnterAfterTyping)
        );
        assert_eq!(
            key_intent("Enter", None, true),
            Some(SubmitReason::EnterAfterTyping)
        );
        assert_eq!(key_intent("Enter", None, false), None);
        assert_eq!(key_intent("Tab", Some(&field), true), None);
        assert_eq!(key_intent("Control+A", Some(&field), true), None);
        // Enter or Space on a flagged button is a click on it.
        assert_eq!(
            key_intent("Space", Some(&button("Pay")), false),
            Some(SubmitReason::ConsequentialLabel("pay".into()))
        );
        assert_eq!(key_intent("Space", Some(&field), true), None);
        let opaque = ElementInfo {
            opaque_frame: true,
            ..Default::default()
        };
        assert_eq!(
            key_intent("Enter", Some(&opaque), false),
            Some(SubmitReason::OpaqueFocus)
        );
    }

    #[test]
    fn the_key_judged_is_the_key_pressed() {
        // Review fix: parse_key trims what the model typed, so the intent check must
        // judge the same key, or " Enter" presses Enter without a card.
        let field = ElementInfo {
            tag: "input".into(),
            is_editable: true,
            in_form: true,
            ..Default::default()
        };
        for key in [
            " Enter",
            "Enter ",
            "\tEnter\n",
            "RETURN",
            "Shift + Enter",
            "Control+Enter",
        ] {
            assert_eq!(
                key_intent(key, Some(&field), false),
                Some(SubmitReason::EnterInFormField),
                "{key:?}"
            );
            assert_eq!(
                key_intent(key, None, true),
                Some(SubmitReason::EnterAfterTyping),
                "{key:?}"
            );
        }
        assert_eq!(
            key_intent(" Space ", Some(&button("Pay")), false),
            Some(SubmitReason::ConsequentialLabel("pay".into()))
        );
        // Keys that do not parse are never pressed.
        assert_eq!(key_intent("Enter+", Some(&field), true), None);
        assert_eq!(key_intent("Hyper+Enter", Some(&field), true), None);
    }

    #[test]
    fn space_in_a_frame_the_tool_cannot_see_into_asks() {
        // Review fix: Space presses the focused button inside the frame, whose label
        // cannot be checked, so it fails closed like Enter.
        let opaque = ElementInfo {
            opaque_frame: true,
            ..Default::default()
        };
        assert_eq!(
            key_intent("Space", Some(&opaque), false),
            Some(SubmitReason::OpaqueFocus)
        );
        assert_eq!(key_intent("Tab", Some(&opaque), false), None);
        assert_eq!(key_intent("a", Some(&opaque), false), None);
    }

    fn req(method: &str, origin: &str) -> PausedRequest {
        PausedRequest {
            tab: "T1".into(),
            opener_tab: None,
            method: method.into(),
            origin: origin.into(),
            top_level: true,
            is_document: true,
            form_submission: false,
            strict: false,
            redirect_of_released: false,
        }
    }

    fn intent(origin: &str, method: Option<&str>) -> SubmitIntent {
        SubmitIntent {
            reason: SubmitReason::SubmitControl,
            tab: "T1".into(),
            method: method.map(str::to_string),
            target_origin: origin.into(),
        }
    }

    const A: &str = "https://a.example";
    const B: &str = "https://b.example";

    #[test]
    fn same_site_get_navigation_continues() {
        assert_eq!(
            decide(&req("GET", A), Some(A), &CallView::default()),
            Verdict::Continue
        );
    }

    #[test]
    fn leaving_the_site_is_held_unless_it_is_the_approved_navigation() {
        assert_eq!(
            decide(&req("GET", B), Some(A), &CallView::default()),
            Verdict::Hold(HoldKind::LeavesSite)
        );
        assert_eq!(
            decide(&req("GET", B), None, &CallView::default()),
            Verdict::Hold(HoldKind::LeavesSite)
        );
        let call = CallView {
            expected_navigation: Some((B.into(), "T1".into())),
            ..Default::default()
        };
        assert_eq!(
            decide(&req("GET", B), Some(A), &call),
            Verdict::ContinueApprovedNavigation
        );
        // The approved navigation is for another tab.
        let other_tab = CallView {
            expected_navigation: Some((B.into(), "T2".into())),
            ..Default::default()
        };
        assert_eq!(
            decide(&req("GET", B), Some(A), &other_tab),
            Verdict::Hold(HoldKind::LeavesSite)
        );
    }

    #[test]
    fn https_upgrade_of_the_granted_site_is_not_leaving_it() {
        assert_eq!(
            decide(
                &req("GET", "https://a.example"),
                Some("http://a.example"),
                &CallView::default()
            ),
            Verdict::Continue
        );
        // A non-default port is a different site.
        assert_eq!(
            decide(
                &req("GET", "https://a.example"),
                Some("http://a.example:8080"),
                &CallView::default()
            ),
            Verdict::Hold(HoldKind::LeavesSite)
        );
        // A downgrade is leaving.
        assert_eq!(
            decide(
                &req("GET", "http://a.example"),
                Some("https://a.example"),
                &CallView::default()
            ),
            Verdict::Hold(HoldKind::LeavesSite)
        );
    }

    #[test]
    fn sub_frame_get_navigations_are_not_held() {
        let mut r = req("GET", B);
        r.top_level = false;
        assert_eq!(decide(&r, Some(A), &CallView::default()), Verdict::Continue);
    }

    #[test]
    fn every_post_navigation_is_held_even_on_the_granted_site() {
        assert_eq!(
            decide(&req("POST", A), Some(A), &CallView::default()),
            Verdict::Hold(HoldKind::Submission)
        );
        let mut sub = req("POST", A);
        sub.top_level = false;
        assert_eq!(
            decide(&sub, Some(A), &CallView::default()),
            Verdict::Hold(HoldKind::Submission)
        );
        let mut get_form = req("GET", A);
        get_form.form_submission = true;
        assert_eq!(
            decide(&get_form, Some(A), &CallView::default()),
            Verdict::Hold(HoldKind::Submission)
        );
    }

    #[test]
    fn the_approved_intent_releases_only_a_matching_submission() {
        let call = CallView {
            intent: Some(intent(A, Some("post"))),
            ..Default::default()
        };
        assert_eq!(
            decide(&req("POST", A), Some(A), &call),
            Verdict::ContinueUsingIntent
        );
        // Another target origin raises its own card.
        assert_eq!(
            decide(&req("POST", B), Some(A), &call),
            Verdict::Hold(HoldKind::Submission)
        );
        // Another method too.
        assert_eq!(
            decide(&req("PUT", A), Some(A), &call),
            Verdict::Hold(HoldKind::Submission)
        );
        // Another tab too, unless it is a pop-up opened by the action's tab.
        let mut other = req("POST", A);
        other.tab = "T2".into();
        assert_eq!(
            decide(&other, Some(A), &call),
            Verdict::Hold(HoldKind::Submission)
        );
        other.opener_tab = Some("T1".into());
        assert_eq!(decide(&other, Some(A), &call), Verdict::ContinueUsingIntent);
        // No method known: any method to the expected origin.
        let loose = CallView {
            intent: Some(intent(A, None)),
            ..Default::default()
        };
        assert_eq!(
            decide(&req("DELETE", A), Some(A), &loose),
            Verdict::ContinueUsingIntent
        );
    }

    #[test]
    fn background_requests_are_held_only_in_strict_mode() {
        let mut xhr = req("POST", A);
        xhr.is_document = false;
        xhr.top_level = false;
        assert_eq!(
            decide(&xhr, Some(A), &CallView::default()),
            Verdict::Continue
        );
        xhr.strict = true;
        assert_eq!(
            decide(&xhr, Some(A), &CallView::default()),
            Verdict::Hold(HoldKind::Background)
        );
        let mut get = xhr.clone();
        get.method = "GET".into();
        assert_eq!(
            decide(&get, Some(A), &CallView::default()),
            Verdict::Continue
        );
    }

    #[test]
    fn redirect_hops_of_a_released_submission_stay_on_site_only() {
        let mut hop = req("POST", A);
        hop.redirect_of_released = true;
        assert_eq!(
            decide(&hop, Some(A), &CallView::default()),
            Verdict::Continue
        );
        hop.origin = B.into();
        assert_eq!(
            decide(&hop, Some(A), &CallView::default()),
            Verdict::Hold(HoldKind::Submission)
        );
    }

    #[test]
    fn reasons_read_as_sentences() {
        for r in [
            SubmitReason::SubmitControl,
            SubmitReason::EnterInFormField,
            SubmitReason::ButtonAfterTyping,
            SubmitReason::EnterAfterTyping,
            SubmitReason::ConsequentialLabel("pay".into()),
            SubmitReason::OpaqueFocus,
        ] {
            let s = r.describe();
            assert!(s.ends_with('.'), "{s}");
        }
    }
}
