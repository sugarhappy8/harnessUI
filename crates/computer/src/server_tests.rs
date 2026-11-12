//! End-to-end tests of the computer server on the fake desktop: describe → (policy) →
//! call, as the host runs them.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use mcp_host::{BuiltinServer, CallContext, ChatContext, CompiledSchema, Content, Described};
use policy::{Action, AskReason, CardKind, Decision, Risk, Scope, TurnContext, evaluate};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::apps::AppInfo;
use crate::geometry::{MonitorInfo, Rect};
use crate::guard::{ELEVATED_REFUSAL, Refusal};
use crate::platform::InputEvent;
use crate::platform::fake::{FakeDesktop, FakeWindow};
use crate::profile::{Convention, ResizeRule, SmartResize};

const ME: u32 = 1000;
const WEBVIEW: u32 = 1001;
const CHAT: i64 = 7;

fn app(name: &str) -> AppInfo {
    AppInfo::from_windows_path(&format!(r"C:\Apps\{name}.exe"))
}

fn primary() -> MonitorInfo {
    MonitorInfo {
        id: 1,
        name: "Built-in".into(),
        bounds: Rect::new(0.0, 0.0, 1000.0, 800.0),
        pixels_per_unit: 1.0,
        ui_scale: 1.0,
        primary: true,
    }
}

fn left_monitor() -> MonitorInfo {
    MonitorInfo {
        id: 2,
        name: "Left".into(),
        bounds: Rect::new(-1280.0, -200.0, 1280.0, 1024.0),
        pixels_per_unit: 1.0,
        ui_scale: 1.25,
        primary: false,
    }
}

/// Notepad fills the primary monitor and has the focus; this app's window sits on the
/// right half (owned by the webview, rooted in the app), behind nothing.
fn desktop() -> FakeDesktop {
    let d = FakeDesktop::new(vec![primary(), left_monitor()]);
    let mut mine = FakeWindow::new(
        2,
        Rect::new(600.0, 0.0, 400.0, 800.0),
        WEBVIEW,
        app("harness"),
    );
    mine.root_pid = Some(ME);
    mine.colour = [255, 0, 255];
    d.push_front(mine);
    let mut notepad = FakeWindow::new(1, Rect::new(0.0, 0.0, 1000.0, 800.0), 50, app("notepad"));
    notepad.colour = [0, 200, 0];
    d.state().windows.push(notepad);
    d.state().foreground = Some(1);
    d.state().capture_excludes = vec![ME, WEBVIEW];
    d
}

fn me() -> SelfProcess {
    SelfProcess::new(ME, || vec![WEBVIEW])
}

fn server(d: &FakeDesktop) -> ComputerServer {
    server_with(d, ComputerConfig::default())
}

fn server_with(d: &FakeDesktop, config: ComputerConfig) -> ComputerServer {
    let s = ComputerServer::new(d.capture(), d.injector(), me(), config);
    // Identity resize and pixel coordinates: model pixels are screen pixels.
    s.set_profile(
        CHAT,
        ModelProfile {
            convention: Convention::Pixels,
            resize: ResizeRule::Original,
        },
    );
    s
}

fn chat() -> ChatContext {
    ChatContext {
        chat_id: CHAT,
        result_cap_tokens: 2000,
        full_access: false,
        ..Default::default()
    }
}

fn ctx() -> CallContext {
    CallContext {
        chat: chat(),
        cancel: CancellationToken::new(),
        progress: None,
    }
}

fn args(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap()
}

async fn describe(s: &ComputerServer, tool: &str, a: Value) -> Result<Action, String> {
    match s.describe(tool, &args(a), &chat()).await {
        Described::Action(a) => Ok(a),
        Described::Invalid(why) => Err(why),
    }
}

/// Describe, refuse on a hard limit, then call with the approved action.
async fn run(s: &ComputerServer, tool: &str, a: Value) -> Result<(Action, ToolOutput), String> {
    let action = describe(s, tool, a.clone()).await?;
    if let Some(h) = &action.hard_limit {
        return Err(h.clone());
    }
    let out = s.call(tool, &args(a), &action, ctx()).await;
    Ok((action, out))
}

async fn screenshot(s: &ComputerServer) -> ToolOutput {
    let (_, out) = run(s, TOOL_SCREENSHOT, json!({})).await.unwrap();
    assert!(!out.is_error, "{}", out.joined_text());
    out
}

fn decode(out: &ToolOutput) -> image::RgbImage {
    let data = out
        .content
        .iter()
        .find_map(|c| match c {
            Content::Image { mime, data_base64 } => {
                assert_eq!(mime, "image/png");
                Some(data_base64.clone())
            }
            _ => None,
        })
        .expect("an image block");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .unwrap();
    image::load_from_memory(&bytes).unwrap().to_rgb8()
}

/// The device pixel a move event lands on.
fn moved_to(e: &InputEvent) -> (i32, i32) {
    match e {
        InputEvent::Move { to } => to.device(),
        other => panic!("not a move: {other:?}"),
    }
}

fn buttons(d: &FakeDesktop) -> Vec<InputEvent> {
    d.events()
        .into_iter()
        .filter(|e| matches!(e, InputEvent::Button { .. }))
        .collect()
}

fn ready() -> TurnContext {
    TurnContext {
        tool_enabled: true,
        server_ready: true,
        ..Default::default()
    }
}

#[test]
fn tools_have_fixed_names_and_schemas_that_compile() {
    let d = desktop();
    let s = server(&d);
    let names: Vec<String> = s.tools().into_iter().map(|t| t.name).collect();
    assert_eq!(
        names,
        vec![
            "computer_screenshot",
            "computer_click",
            "computer_move",
            "computer_drag",
            "computer_scroll",
            "computer_type",
            "computer_key"
        ]
    );
    for t in s.tools() {
        CompiledSchema::compile(&t.input_schema).unwrap_or_else(|e| panic!("{}: {e}", t.name));
        assert_eq!(t.input_schema["additionalProperties"], json!(false));
        assert_eq!(t.destructive, Some(false));
    }
    assert_eq!(s.name(), "computer");
}

#[tokio::test]
async fn screenshot_is_read_only_resized_like_llama_cpp_and_leaves_this_app_out() {
    let d = desktop();
    let s = server(&d);
    let smart = SmartResize::qwen2_vl(8, 4096);
    s.set_profile(
        CHAT,
        ModelProfile {
            convention: Convention::Pixels,
            resize: ResizeRule::Smart(smart),
        },
    );
    d.state().cursor = GlobalPoint::new(100.0, 100.0);
    let (action, out) = run(&s, TOOL_SCREENSHOT, json!({})).await.unwrap();
    assert_eq!(action.scope, Scope::Screenshots);
    assert!(action.read_only);
    assert!(!action.outbound);
    assert_eq!(action.risk, Risk::Normal);
    assert!(
        action.summary.contains("monitor 1 of 2"),
        "{}",
        action.summary
    );
    assert!(!out.is_error, "{}", out.joined_text());
    let img = decode(&out);
    let expected = smart.target_size(1000, 800);
    assert_eq!(img.dimensions(), expected);
    // This app's window (magenta, right side) is not in the image: Notepad shows through.
    let px = img.get_pixel(expected.0 * 9 / 10, expected.1 / 2).0;
    assert_eq!(px, [0, 200, 0], "this app's window must be left out");
    // The cursor marker is drawn just below its tip.
    let (cx, cy) = (
        100.0 / 1000.0 * expected.0 as f64,
        100.0 / 800.0 * expected.1 as f64,
    );
    let marker = img.get_pixel(cx as u32 + 1, cy as u32 + 4).0;
    assert_eq!(marker, [255, 255, 255]);
    let text = out.joined_text();
    assert!(
        text.contains(&format!("sent as {}x{} pixels", expected.0, expected.1)),
        "{text}"
    );
    assert!(text.contains("Coordinates are pixels of this"), "{text}");
    assert!(text.contains("The pointer"), "{text}");
    let geo = s.last_screenshot(CHAT).unwrap();
    assert_eq!(geo.image, expected);
    assert_eq!(geo.captured, (1000, 800));
    assert_eq!(geo.monitor.id, 1);
}

#[tokio::test]
async fn screenshot_monitor_choice_and_errors() {
    let d = desktop();
    let s = server(&d);
    // The window in front decides the default monitor.
    let mut far = FakeWindow::new(5, Rect::new(-1000.0, 0.0, 600.0, 400.0), 60, app("mail"));
    far.colour = [1, 2, 3];
    d.push_front(far);
    let a = describe(&s, TOOL_SCREENSHOT, json!({})).await.unwrap();
    assert!(a.summary.contains("monitor 2 of 2"), "{}", a.summary);
    let a = describe(&s, TOOL_SCREENSHOT, json!({"monitor": 1}))
        .await
        .unwrap();
    assert!(a.summary.contains("monitor 1 of 2"));
    let e = describe(&s, TOOL_SCREENSHOT, json!({"monitor": 3}))
        .await
        .unwrap_err();
    assert!(e.contains("no monitor 3"), "{e}");
    // A capture failure is a readable tool error.
    d.state().capture_error = Some(CaptureError::NoPermission(
        "Screen Recording is not allowed for this app".into(),
    ));
    let (_, out) = run(&s, TOOL_SCREENSHOT, json!({})).await.unwrap();
    assert!(out.is_error);
    assert!(out.joined_text().contains("Screen Recording"));
}

#[tokio::test]
async fn pointer_tools_need_a_screenshot_first() {
    let d = desktop();
    let s = server(&d);
    let e = describe(&s, TOOL_CLICK, json!({"coordinate": [10, 10]}))
        .await
        .unwrap_err();
    assert!(e.contains("computer_screenshot first"), "{e}");
}

#[tokio::test]
async fn click_maps_the_coordinate_and_sends_move_down_up() {
    let d = desktop();
    let s = server(&d);
    screenshot(&s).await;
    d.clear_events();
    let (action, out) = run(&s, TOOL_CLICK, json!({"coordinate": [100, 200]}))
        .await
        .unwrap();
    assert_eq!(
        action.scope,
        Scope::ComputerInput {
            kind: "pointer".into(),
            app: app("notepad").id
        }
    );
    assert_eq!(action.risk, Risk::Normal);
    assert!(!action.outbound);
    assert!(action.preview.contains("notepad"), "{}", action.preview);
    assert!(!out.is_error, "{}", out.joined_text());
    let ev = d.events();
    assert_eq!(moved_to(&ev[0]), (100, 200));
    assert_eq!(
        ev[1..],
        [
            InputEvent::Button {
                button: MouseButton::Left,
                down: true
            },
            InputEvent::Button {
                button: MouseButton::Left,
                down: false
            },
        ]
    );
    assert!(
        out.joined_text()
            .contains("Clicked the left button at [100, 200]")
    );
    assert_eq!(s.held(), Held::default());
}

#[tokio::test]
async fn double_right_click_and_out_of_range_coordinates() {
    let d = desktop();
    let s = server(&d);
    screenshot(&s).await;
    d.clear_events();
    let (_, out) = run(
        &s,
        TOOL_CLICK,
        json!({"coordinate": [10, 10], "button": "right", "clicks": 2}),
    )
    .await
    .unwrap();
    assert!(!out.is_error);
    assert_eq!(buttons(&d).len(), 4);
    let e = describe(&s, TOOL_CLICK, json!({"coordinate": [5000, 10]}))
        .await
        .unwrap_err();
    assert!(e.contains("outside the screenshot"), "{e}");
    let e = describe(&s, TOOL_CLICK, json!({"coordinate": [1, 2, 3]}))
        .await
        .unwrap_err();
    assert!(e.contains("got 3 numbers"), "{e}");
}

#[tokio::test]
async fn input_to_this_app_is_a_hard_limit() {
    let d = desktop();
    let s = server(&d);
    screenshot(&s).await;
    // A click on this app's own window (it would press its own approval card).
    let a = describe(&s, TOOL_CLICK, json!({"coordinate": [700, 300]}))
        .await
        .unwrap();
    assert_eq!(
        a.hard_limit.as_deref(),
        Some(Refusal::SelfPointer.to_string().as_str())
    );
    let grants: BTreeSet<Scope> = BTreeSet::new();
    assert!(matches!(
        evaluate(&a, &ready(), &grants),
        Decision::Denied { .. }
    ));
    // Even if a hard-limited action reached call, nothing would be sent.
    d.clear_events();
    let out = s
        .call(
            TOOL_CLICK,
            &args(json!({"coordinate": [700, 300]})),
            &a,
            ctx(),
        )
        .await;
    assert!(out.is_error);
    assert!(d.events().is_empty());
    // Keyboard with this app in front.
    d.state().foreground = Some(2);
    let a = describe(&s, TOOL_TYPE, json!({"text": "hi"}))
        .await
        .unwrap();
    assert_eq!(
        a.hard_limit.as_deref(),
        Some(Refusal::SelfKeyboard.to_string().as_str())
    );
    let a = describe(&s, TOOL_KEY, json!({"keys": "Enter"}))
        .await
        .unwrap();
    assert!(a.hard_limit.is_some());
}

#[tokio::test]
async fn a_window_of_this_app_appearing_after_approval_is_refused_at_call() {
    let d = desktop();
    let s = server(&d);
    screenshot(&s).await;
    let a = describe(&s, TOOL_CLICK, json!({"coordinate": [100, 100]}))
        .await
        .unwrap();
    assert!(a.hard_limit.is_none());
    // The app's window moves over the point while the card is open.
    {
        let mut st = d.state();
        let w = st.windows.iter_mut().find(|w| w.id == 2).unwrap();
        w.rect = Rect::new(0.0, 0.0, 400.0, 400.0);
    }
    d.clear_events();
    let out = s
        .call(
            TOOL_CLICK,
            &args(json!({"coordinate": [100, 100]})),
            &a,
            ctx(),
        )
        .await;
    assert!(out.is_error);
    assert!(out.joined_text().contains("this app's own window"));
    assert!(d.events().is_empty());
}

#[tokio::test]
async fn the_target_application_is_rechecked_at_call_time() {
    let d = desktop();
    let s = server(&d);
    screenshot(&s).await;
    let a = describe(&s, TOOL_CLICK, json!({"coordinate": [100, 100]}))
        .await
        .unwrap();
    d.push_front(FakeWindow::new(
        8,
        Rect::new(0.0, 0.0, 300.0, 300.0),
        61,
        app("mail"),
    ));
    d.clear_events();
    let out = s
        .call(
            TOOL_CLICK,
            &args(json!({"coordinate": [100, 100]})),
            &a,
            ctx(),
        )
        .await;
    assert!(out.is_error);
    let text = out.joined_text();
    assert!(text.contains("would now go to"), "{text}");
    assert!(text.contains("mail"), "{text}");
    assert!(buttons(&d).is_empty());
    // Keyboard: approved for Notepad, the focus moves to Mail before the call.
    let d = desktop();
    let s = server(&d);
    let a = describe(&s, TOOL_TYPE, json!({"text": "secret"}))
        .await
        .unwrap();
    d.push_front(FakeWindow::new(
        8,
        Rect::new(0.0, 0.0, 300.0, 300.0),
        61,
        app("mail"),
    ));
    let out = s
        .call(TOOL_TYPE, &args(json!({"text": "secret"})), &a, ctx())
        .await;
    assert!(out.is_error);
    assert!(d.events().is_empty());
}

#[tokio::test]
async fn grants_are_bound_to_the_application_d7() {
    let d = desktop();
    let mut mail = FakeWindow::new(8, Rect::new(0.0, 500.0, 300.0, 300.0), 61, app("mail"));
    mail.colour = [9, 9, 9];
    d.state().windows.insert(0, mail);
    let s = server(&d);
    screenshot(&s).await;
    let grants: BTreeSet<Scope> = [Scope::ComputerInput {
        kind: "pointer".into(),
        app: app("notepad").id,
    }]
    .into();
    let in_notepad = describe(&s, TOOL_CLICK, json!({"coordinate": [100, 100]}))
        .await
        .unwrap();
    assert_eq!(
        evaluate(&in_notepad, &ready(), &grants),
        Decision::AllowedByGrant
    );
    // Input to any other application asks again.
    let in_mail = describe(&s, TOOL_CLICK, json!({"coordinate": [100, 600]}))
        .await
        .unwrap();
    assert_eq!(
        evaluate(&in_mail, &ready(), &grants),
        Decision::Ask {
            card: CardKind::Grantable,
            reason: AskReason::Normal
        }
    );
    // A pointer grant does not cover the keyboard.
    let typing = describe(&s, TOOL_TYPE, json!({"text": "x"})).await.unwrap();
    assert!(matches!(
        evaluate(&typing, &ready(), &grants),
        Decision::Ask { .. }
    ));
    // Keyboard input is outbound: after untrusted content it asks even with a grant (D9).
    let kb: BTreeSet<Scope> = [typing.scope.clone()].into();
    assert_eq!(evaluate(&typing, &ready(), &kb), Decision::AllowedByGrant);
    let mut tainted = ready();
    tainted.untrusted_in_turn = true;
    assert!(matches!(
        evaluate(&typing, &tainted, &kb),
        Decision::Ask {
            reason: AskReason::UntrustedContentThenOutbound,
            ..
        }
    ));
}

#[tokio::test]
async fn terminals_settings_and_password_managers_always_ask() {
    for exe in ["cmd", "WindowsTerminal", "SystemSettings", "KeePassXC"] {
        let d = desktop();
        let mut w = FakeWindow::new(9, Rect::new(0.0, 0.0, 500.0, 500.0), 70, app(exe));
        w.colour = [5, 5, 5];
        d.push_front(w);
        let s = server(&d);
        screenshot(&s).await;
        let a = describe(&s, TOOL_CLICK, json!({"coordinate": [100, 100]}))
            .await
            .unwrap();
        assert_eq!(a.risk, Risk::AlwaysAsk, "{exe}");
        assert!(a.preview.contains("Always asks"), "{}", a.preview);
        // Even a matching grant does not cover it.
        let grants: BTreeSet<Scope> = [a.scope.clone()].into();
        assert_eq!(
            evaluate(&a, &ready(), &grants),
            Decision::Ask {
                card: CardKind::OnceOnly,
                reason: AskReason::AlwaysAsk
            }
        );
        let k = describe(&s, TOOL_TYPE, json!({"text": "rm -rf ~"}))
            .await
            .unwrap();
        assert_eq!(k.risk, Risk::AlwaysAsk, "{exe}");
    }
}

#[tokio::test]
async fn elevated_windows_are_refused_with_the_fixed_message() {
    let d = desktop();
    let mut w = FakeWindow::new(9, Rect::new(0.0, 0.0, 500.0, 500.0), 70, app("regedit"));
    w.elevated = true;
    d.push_front(w);
    let s = server(&d);
    screenshot(&s).await;
    let a = describe(&s, TOOL_CLICK, json!({"coordinate": [100, 100]}))
        .await
        .unwrap();
    assert_eq!(a.hard_limit.as_deref(), Some(ELEVATED_REFUSAL));
    let a = describe(&s, TOOL_KEY, json!({"keys": "ctrl+v"}))
        .await
        .unwrap();
    assert_eq!(a.hard_limit.as_deref(), Some(ELEVATED_REFUSAL));
}

#[tokio::test]
async fn typing_goes_out_in_short_checked_runs() {
    let d = desktop();
    let s = server(&d);
    let (a, out) = run(&s, TOOL_TYPE, json!({"text": "hello world\nok"}))
        .await
        .unwrap();
    assert_eq!(
        a.scope,
        Scope::ComputerInput {
            kind: "keyboard".into(),
            app: app("notepad").id
        }
    );
    assert!(a.outbound);
    assert!(a.preview.contains("hello world"));
    assert!(!out.is_error, "{}", out.joined_text());
    assert_eq!(
        d.events(),
        vec![
            InputEvent::Text {
                text: "hello wo".into()
            },
            InputEvent::Text { text: "rld".into() },
            InputEvent::Key {
                key: Key::Enter,
                down: true
            },
            InputEvent::Key {
                key: Key::Enter,
                down: false
            },
            InputEvent::Text { text: "ok".into() },
        ]
    );
    let e = describe(&s, TOOL_TYPE, json!({"text": "a\u{1b}[2J"}))
        .await
        .unwrap_err();
    assert!(e.contains("control character"), "{e}");
    let long = "x".repeat(MAX_TYPE_CHARS + 1);
    assert!(
        describe(&s, TOOL_TYPE, json!({ "text": long }))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn key_combinations_press_and_release_in_order() {
    let d = desktop();
    let s = server(&d);
    let (a, out) = run(&s, TOOL_KEY, json!({"keys": "ctrl+s"})).await.unwrap();
    assert_eq!(a.risk, Risk::Normal);
    assert!(!out.is_error, "{}", out.joined_text());
    let k = |key, down| InputEvent::Key { key, down };
    assert_eq!(
        d.events(),
        vec![
            k(Key::Control, true),
            k(Key::Char('s'), true),
            k(Key::Char('s'), false),
            k(Key::Control, false),
        ]
    );
    assert!(d.state().keys_down.is_empty());
    // Shortcuts the OS handles always ask.
    let a = describe(&s, TOOL_KEY, json!({"keys": "win+r"}))
        .await
        .unwrap();
    assert_eq!(a.risk, Risk::AlwaysAsk);
    assert!(a.preview.contains("Windows key"), "{}", a.preview);
    let e = describe(&s, TOOL_KEY, json!({"keys": "ctrl+nope"}))
        .await
        .unwrap_err();
    assert!(e.contains("unknown key"));
    // Repeat.
    d.clear_events();
    let (_, out) = run(&s, TOOL_KEY, json!({"keys": "Down", "repeat": 3}))
        .await
        .unwrap();
    assert!(!out.is_error);
    assert_eq!(d.events().len(), 6);
}

#[tokio::test]
async fn grid_convention_on_a_monitor_with_a_negative_origin() {
    let d = desktop();
    let s = server(&d);
    s.set_profile(
        CHAT,
        ModelProfile {
            convention: Convention::Grid1000,
            resize: ResizeRule::Smart(SmartResize::qwen3_vl(8, 4096)),
        },
    );
    let mut w = FakeWindow::new(
        4,
        Rect::new(-1280.0, -200.0, 1280.0, 1024.0),
        80,
        app("browser"),
    );
    w.colour = [10, 20, 30];
    d.state().windows.push(w);
    let (_, out) = run(&s, TOOL_SCREENSHOT, json!({"monitor": 2}))
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.joined_text());
    assert!(out.joined_text().contains("0-1000 grid"));
    assert!(out.joined_text().contains("125% scaling"));
    let img = decode(&out);
    assert_eq!(
        img.dimensions(),
        SmartResize::qwen3_vl(8, 4096).target_size(1280, 1024)
    );
    d.clear_events();
    let (a, out) = run(&s, TOOL_CLICK, json!({"coordinate": [500, 500]}))
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.joined_text());
    assert!(matches!(&a.scope, Scope::ComputerInput { app: id, .. } if *id == app("browser").id));
    assert_eq!(
        d.events()[0],
        InputEvent::Move {
            to: GlobalPoint::new(-640.0, 312.0)
        }
    );
    // Boxes: centre of [0, 0, 1000, 1000] is the same point.
    s.set_profile(
        CHAT,
        ModelProfile {
            convention: Convention::Box1000,
            resize: ResizeRule::Smart(SmartResize::qwen3_vl(8, 4096)),
        },
    );
    // Changing the profile drops the old screenshot.
    assert!(s.last_screenshot(CHAT).is_none());
    run(&s, TOOL_SCREENSHOT, json!({"monitor": 2}))
        .await
        .unwrap();
    d.clear_events();
    run(&s, TOOL_MOVE, json!({"coordinate": [0, 0, 1000, 1000]}))
        .await
        .unwrap();
    assert_eq!(
        d.events(),
        vec![InputEvent::Move {
            to: GlobalPoint::new(-640.0, 312.0)
        }]
    );
}

#[tokio::test]
async fn a_screen_layout_change_after_approval_refuses_the_input() {
    let d = desktop();
    let s = server(&d);
    screenshot(&s).await;
    let a = describe(&s, TOOL_CLICK, json!({"coordinate": [100, 100]}))
        .await
        .unwrap();
    d.state().monitors[0].bounds = Rect::new(0.0, 0.0, 2000.0, 1600.0);
    d.clear_events();
    let out = s
        .call(
            TOOL_CLICK,
            &args(json!({"coordinate": [100, 100]})),
            &a,
            ctx(),
        )
        .await;
    assert!(out.is_error);
    assert!(out.joined_text().contains("screen layout changed"));
    assert!(d.events().is_empty());
}

#[tokio::test]
async fn stop_during_a_drag_releases_the_button() {
    let d = desktop();
    let s = server(&d);
    screenshot(&s).await;
    d.state().send_delay = Some(Duration::from_millis(5));
    let a = describe(
        &s,
        TOOL_DRAG,
        json!({"from": [50, 50], "to": [500, 500], "duration_ms": 2000}),
    )
    .await
    .unwrap();
    assert_eq!(a.risk, Risk::Normal);
    let c = ctx();
    let cancel = c.cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        cancel.cancel();
    });
    let started = std::time::Instant::now();
    let out = s.call(TOOL_DRAG, &args(json!({})), &a, c).await;
    assert!(out.is_error);
    assert!(
        out.joined_text().contains("Stopped"),
        "{}",
        out.joined_text()
    );
    assert!(started.elapsed() < Duration::from_millis(1500));
    // The button went down and was released.
    let ev = buttons(&d);
    assert!(ev.contains(&InputEvent::Button {
        button: MouseButton::Left,
        down: true
    }));
    // Give the blocking task a moment to notice and finish.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(d.state().buttons_down.is_empty());
    assert!(d.state().keys_down.is_empty());
    assert_eq!(s.held(), Held::default());
}

#[tokio::test]
async fn release_all_is_what_the_stop_hotkey_calls() {
    let d = desktop();
    let s = server(&d);
    d.state().keys_down.insert(Key::Shift);
    d.state().buttons_down.insert(MouseButton::Left);
    s.release_all();
    for _ in 0..200 {
        if d.state().keys_down.is_empty() && d.state().buttons_down.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(d.state().keys_down.is_empty());
    assert!(d.state().buttons_down.is_empty());
    // Shutdown does the same, synchronously.
    d.state().keys_down.insert(Key::Meta);
    s.shutdown();
    assert!(d.state().keys_down.is_empty());
}

#[tokio::test]
async fn drags_between_applications_always_ask_and_never_cross_this_app() {
    let d = desktop();
    let mut mail = FakeWindow::new(8, Rect::new(0.0, 500.0, 300.0, 300.0), 61, app("mail"));
    mail.colour = [9, 9, 9];
    d.state().windows.insert(0, mail);
    let s = server(&d);
    screenshot(&s).await;
    let a = describe(&s, TOOL_DRAG, json!({"from": [100, 100], "to": [100, 600]}))
        .await
        .unwrap();
    assert_eq!(a.risk, Risk::AlwaysAsk);
    assert!(a.preview.contains("mail"), "{}", a.preview);
    d.clear_events();
    let out = s.call(TOOL_DRAG, &args(json!({})), &a, ctx()).await;
    assert!(!out.is_error, "{}", out.joined_text());
    assert_eq!(buttons(&d).len(), 2);
    // A path across this app's window is refused before anything is pressed.
    let a = describe(&s, TOOL_DRAG, json!({"from": [100, 100], "to": [590, 100]}))
        .await
        .unwrap();
    assert!(a.hard_limit.is_none());
    let a = describe(&s, TOOL_DRAG, json!({"from": [100, 100], "to": [900, 100]}))
        .await
        .unwrap();
    assert!(a.hard_limit.is_some());
}

#[tokio::test]
async fn clicks_with_modifiers_need_the_target_in_front() {
    let d = desktop();
    let mut mail = FakeWindow::new(8, Rect::new(0.0, 500.0, 300.0, 300.0), 61, app("mail"));
    mail.colour = [9, 9, 9];
    d.state().windows.insert(0, mail);
    let s = server(&d);
    screenshot(&s).await;
    // Notepad has the focus: shift-click in Notepad works.
    d.clear_events();
    let (_, out) = run(
        &s,
        TOOL_CLICK,
        json!({"coordinate": [100, 100], "modifiers": ["shift"]}),
    )
    .await
    .unwrap();
    assert!(!out.is_error, "{}", out.joined_text());
    let ev = d.events();
    assert_eq!(moved_to(&ev[0]), (100, 100));
    assert!(ev.contains(&InputEvent::Key {
        key: Key::Shift,
        down: true
    }));
    assert!(d.state().keys_down.is_empty());
    // A shift-click in Mail (behind) would send Shift to Notepad: refused.
    let e = describe(
        &s,
        TOOL_CLICK,
        json!({"coordinate": [100, 600], "modifiers": ["shift"]}),
    )
    .await
    .unwrap_err();
    assert!(e.contains("not the application in front"), "{e}");
    let e = describe(
        &s,
        TOOL_CLICK,
        json!({"coordinate": [100, 100], "modifiers": ["x"]}),
    )
    .await
    .unwrap_err();
    assert!(e.contains("modifiers may only contain"), "{e}");
}

#[tokio::test]
async fn the_ready_check_can_refuse_input() {
    let d = desktop();
    let s = server_with(
        &d,
        ComputerConfig {
            ready_check: Some(Arc::new(|| {
                Err("the indicator is not visible on this Space".to_string())
            })),
            ..Default::default()
        },
    );
    screenshot(&s).await;
    d.clear_events();
    let (_, out) = run(&s, TOOL_CLICK, json!({"coordinate": [10, 10]}))
        .await
        .unwrap();
    assert!(out.is_error);
    assert!(out.joined_text().contains("not visible on this Space"));
    assert!(d.events().is_empty());
}

#[tokio::test]
async fn the_protected_stop_button_is_never_clicked() {
    let d = desktop();
    let me = SelfProcess::new(ME, || vec![WEBVIEW])
        .with_protected_rects(|| vec![Rect::new(10.0, 10.0, 40.0, 20.0)]);
    let s = ComputerServer::new(d.capture(), d.injector(), me, ComputerConfig::default());
    s.set_profile(
        CHAT,
        ModelProfile {
            convention: Convention::Pixels,
            resize: ResizeRule::Original,
        },
    );
    screenshot(&s).await;
    let a = describe(&s, TOOL_CLICK, json!({"coordinate": [20, 15]}))
        .await
        .unwrap();
    assert_eq!(
        a.hard_limit.as_deref(),
        Some(Refusal::Protected.to_string().as_str())
    );
}

#[tokio::test]
async fn a_tampered_action_is_refused() {
    let d = desktop();
    let s = server(&d);
    screenshot(&s).await;
    let mut a = describe(&s, TOOL_CLICK, json!({"coordinate": [100, 100]}))
        .await
        .unwrap();
    // The scope names another application than the plan.
    a.scope = Scope::ComputerInput {
        kind: "pointer".into(),
        app: app("mail").id,
    };
    d.clear_events();
    let out = s.call(TOOL_CLICK, &args(json!({})), &a, ctx()).await;
    assert!(out.is_error);
    // An action for another tool.
    let b = describe(&s, TOOL_MOVE, json!({"coordinate": [1, 1]}))
        .await
        .unwrap();
    let out = s.call(TOOL_CLICK, &args(json!({})), &b, ctx()).await;
    assert!(out.is_error);
    assert!(d.events().is_empty());
    let e = describe(&s, "computer_teleport", json!({}))
        .await
        .unwrap_err();
    assert!(e.contains("no tool named"));
}

#[tokio::test]
async fn scroll_moves_then_sends_notches() {
    let d = desktop();
    let s = server(&d);
    screenshot(&s).await;
    d.clear_events();
    let (_, out) = run(
        &s,
        TOOL_SCROLL,
        json!({"coordinate": [100, 100], "direction": "up", "amount": 2}),
    )
    .await
    .unwrap();
    assert!(!out.is_error, "{}", out.joined_text());
    assert_eq!(
        d.events()[1..],
        [
            InputEvent::Scroll { dx: 0, dy: -1 },
            InputEvent::Scroll { dx: 0, dy: -1 }
        ]
    );
    let e = describe(
        &s,
        TOOL_SCROLL,
        json!({"coordinate": [100, 100], "direction": "sideways"}),
    )
    .await
    .unwrap_err();
    assert!(e.contains("direction"));
}

#[test]
fn card_text_is_sanitised() {
    // A window title with a bidi override and a newline cannot spoof the card.
    let t = Target {
        pids: vec![1],
        app: AppInfo::from_windows_path("C:\\x\\evil\u{202E}txt.exe"),
        elevated: false,
        window_title: Some("Pay\nnow\u{200B}".into()),
        window_id: 1,
        window_rect: None,
        console: false,
    };
    let lines = ComputerServer::target_lines(&t);
    assert!(!lines.contains('\u{202E}'));
    assert!(!lines.contains('\u{200B}'));
    assert!(lines.contains("\\u{202E}"));
    assert!(lines.contains("Pay\\nnow"));
}

// ---- Fixes from the adversarial review ----

/// A window of Python: `console` makes it a console window (Windows reports a console
/// window as owned by the program running in it).
fn python_window(id: u64, rect: Rect, console: bool) -> FakeWindow {
    let mut w = FakeWindow::new(id, rect, 90, app("python"));
    w.console = console;
    w.colour = [7, 7, 7];
    w
}

#[tokio::test]
async fn a_grant_for_an_application_never_reaches_its_console_window() {
    // Keyboard: granted for Python's GUI window, then its console window takes the focus.
    let d = desktop();
    d.push_front(python_window(20, Rect::new(0.0, 0.0, 300.0, 300.0), false));
    let s = server(&d);
    let a = describe(&s, TOOL_TYPE, json!({"text": "print(1)"}))
        .await
        .unwrap();
    assert_eq!(a.risk, Risk::Normal);
    let grants: BTreeSet<Scope> = [a.scope.clone()].into();
    assert_eq!(evaluate(&a, &ready(), &grants), Decision::AllowedByGrant);
    d.push_front(python_window(21, Rect::new(0.0, 0.0, 300.0, 300.0), true));
    d.clear_events();
    let out = s.call(TOOL_TYPE, &args(json!({})), &a, ctx()).await;
    assert!(out.is_error);
    assert!(
        out.joined_text()
            .contains("always needs the user's approval"),
        "{}",
        out.joined_text()
    );
    assert!(d.events().is_empty());
    // Approved on the always-ask card itself, it goes through.
    let a = describe(&s, TOOL_TYPE, json!({"text": "print(1)"}))
        .await
        .unwrap();
    assert_eq!(a.risk, Risk::AlwaysAsk);
    assert!(a.preview.contains("console"), "{}", a.preview);
    let out = s.call(TOOL_TYPE, &args(json!({})), &a, ctx()).await;
    assert!(!out.is_error, "{}", out.joined_text());

    // Pointer: the console window appears under an approved click.
    let d = desktop();
    d.push_front(python_window(20, Rect::new(0.0, 0.0, 300.0, 300.0), false));
    let s = server(&d);
    screenshot(&s).await;
    let a = describe(&s, TOOL_CLICK, json!({"coordinate": [100, 100]}))
        .await
        .unwrap();
    assert_eq!(a.risk, Risk::Normal);
    d.push_front(python_window(21, Rect::new(0.0, 0.0, 300.0, 300.0), true));
    d.clear_events();
    let out = s.call(TOOL_CLICK, &args(json!({})), &a, ctx()).await;
    assert!(out.is_error);
    assert!(buttons(&d).is_empty());
}

#[tokio::test]
async fn typing_into_the_windows_shell_always_asks_but_clicking_can_be_granted() {
    let d = desktop();
    let mut explorer = FakeWindow::new(
        30,
        Rect::new(0.0, 0.0, 500.0, 500.0),
        95,
        AppInfo::from_windows_path(r"C:\Windows\explorer.exe"),
    );
    explorer.colour = [3, 3, 3];
    d.push_front(explorer);
    let s = server(&d);
    screenshot(&s).await;
    // Typed text in the Run box or the address bar runs as a command.
    let t = describe(&s, TOOL_TYPE, json!({"text": "cmd /c calc"}))
        .await
        .unwrap();
    assert_eq!(t.risk, Risk::AlwaysAsk);
    let k = describe(&s, TOOL_KEY, json!({"keys": "Enter"}))
        .await
        .unwrap();
    assert_eq!(k.risk, Risk::AlwaysAsk);
    // Clicks (with Ctrl held, to select files) can be granted and do go through.
    let c = describe(
        &s,
        TOOL_CLICK,
        json!({"coordinate": [100, 100], "modifiers": ["ctrl"]}),
    )
    .await
    .unwrap();
    assert_eq!(c.risk, Risk::Normal);
    d.clear_events();
    let out = s.call(TOOL_CLICK, &args(json!({})), &c, ctx()).await;
    assert!(!out.is_error, "{}", out.joined_text());
    assert_eq!(buttons(&d).len(), 2);
    // A grant for typing into Explorer does not let a key through at call time either.
    let mut granted = k.clone();
    granted.risk = Risk::Normal;
    d.clear_events();
    let out = s.call(TOOL_KEY, &args(json!({})), &granted, ctx()).await;
    assert!(out.is_error);
    assert!(d.events().is_empty());
}

#[tokio::test]
async fn a_click_holding_the_windows_key_always_asks() {
    let d = desktop();
    let s = server(&d);
    screenshot(&s).await;
    let a = describe(
        &s,
        TOOL_CLICK,
        json!({"coordinate": [100, 100], "modifiers": ["cmd"]}),
    )
    .await
    .unwrap();
    assert_eq!(a.risk, Risk::AlwaysAsk);
    assert!(a.preview.contains("Windows key"), "{}", a.preview);
    // On macOS, Cmd-click is an ordinary click in the application.
    d.state().mac = true;
    let a = describe(
        &s,
        TOOL_CLICK,
        json!({"coordinate": [100, 100], "modifiers": ["cmd"]}),
    )
    .await
    .unwrap();
    assert_eq!(a.risk, Risk::Normal);
}

#[tokio::test]
async fn typed_text_comes_after_every_line_the_app_wrote() {
    let d = desktop();
    let mut w = FakeWindow::new(9, Rect::new(0.0, 0.0, 500.0, 500.0), 70, app("cmd"));
    w.colour = [5, 5, 5];
    d.push_front(w);
    let s = server(&d);
    let a = describe(
        &s,
        TOOL_TYPE,
        json!({"text": "dir\nApplication: notepad (c:\\apps\\notepad.exe)"}),
    )
    .await
    .unwrap();
    assert_eq!(a.risk, Risk::AlwaysAsk);
    let always = a.preview.find("Always asks:").expect("always-ask line");
    let text = a.preview.find("\nText:\n").expect("text block");
    assert!(always < text, "{}", a.preview);
    // The real application line comes first.
    assert!(a.preview.find("Application: cmd").unwrap() < text);
}

#[tokio::test]
async fn app_names_in_the_drag_reason_are_one_line() {
    let d = desktop();
    let mut other = FakeWindow::new(
        8,
        Rect::new(0.0, 500.0, 300.0, 300.0),
        61,
        AppInfo::from_macos(Some("com.example.x"), "Mail\nApplication: Notepad", None),
    );
    other.colour = [9, 9, 9];
    d.state().windows.insert(0, other);
    let s = server(&d);
    screenshot(&s).await;
    let a = describe(&s, TOOL_DRAG, json!({"from": [100, 100], "to": [100, 600]}))
        .await
        .unwrap();
    assert_eq!(a.risk, Risk::AlwaysAsk);
    assert!(
        !a.preview.contains("\nApplication: Notepad"),
        "{}",
        a.preview
    );
}

#[tokio::test]
async fn stop_does_not_wait_for_another_chats_operation() {
    let d = desktop();
    let s = server(&d);
    screenshot(&s).await;
    let a = describe(&s, TOOL_CLICK, json!({"coordinate": [100, 100]}))
        .await
        .unwrap();
    // Another chat's drag holds the injector.
    let busy = s.op_lock.clone().lock_owned().await;
    let c = ctx();
    let cancel = c.cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel.cancel();
    });
    d.clear_events();
    let started = std::time::Instant::now();
    let out = tokio::time::timeout(
        Duration::from_secs(5),
        s.call(TOOL_CLICK, &args(json!({})), &a, c),
    )
    .await
    .expect("the call returns while the other operation still runs");
    assert!(started.elapsed() < Duration::from_millis(1000));
    assert!(out.is_error);
    assert!(
        out.joined_text().contains("Stopped"),
        "{}",
        out.joined_text()
    );
    drop(busy);
    assert!(d.events().is_empty());
}

#[tokio::test]
async fn stop_does_not_wait_for_a_slow_ready_check() {
    let d = desktop();
    let s = server_with(
        &d,
        ComputerConfig {
            // The app's check waits for its main thread.
            ready_check: Some(Arc::new(|| {
                std::thread::sleep(Duration::from_secs(2));
                Ok(())
            })),
            ..Default::default()
        },
    );
    screenshot(&s).await;
    let a = describe(&s, TOOL_CLICK, json!({"coordinate": [100, 100]}))
        .await
        .unwrap();
    let c = ctx();
    let cancel = c.cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel.cancel();
    });
    d.clear_events();
    let started = std::time::Instant::now();
    let out = s.call(TOOL_CLICK, &args(json!({})), &a, c).await;
    assert!(started.elapsed() < Duration::from_millis(1000));
    assert!(out.is_error);
    assert!(d.events().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn screenshots_never_overlap() {
    let d = desktop();
    let s = Arc::new(server(&d));
    d.state().capture_delay = Some(Duration::from_millis(100));
    let a = describe(&s, TOOL_SCREENSHOT, json!({})).await.unwrap();
    let (s1, s2, a1, a2) = (s.clone(), s.clone(), a.clone(), a.clone());
    let t1 =
        tokio::spawn(async move { s1.call(TOOL_SCREENSHOT, &args(json!({})), &a1, ctx()).await });
    let t2 =
        tokio::spawn(async move { s2.call(TOOL_SCREENSHOT, &args(json!({})), &a2, ctx()).await });
    let (o1, o2) = (t1.await.unwrap(), t2.await.unwrap());
    assert!(!o1.is_error && !o2.is_error);
    // On Windows a capture hides this app's windows and shows them again afterwards; an
    // overlapping capture would see them reappear halfway.
    assert_eq!(d.state().max_captures_in_flight, 1);
}

// ---- Allow all (0.0.4) -------------------------------------------------------------

fn allow_all(untrusted: bool) -> TurnContext {
    TurnContext {
        allow_all: true,
        untrusted_in_turn: untrusted,
        ..ready()
    }
}

const STILL_ASKS: Decision = Decision::Ask {
    card: CardKind::OnceOnly,
    reason: AskReason::StillAsksInAllowAll,
};

/// Ordinary input to an ordinary application runs without a card in Allow all, also
/// after untrusted content (a screenshot is one). Screenshots too. Input to this app
/// stays a hard limit.
#[tokio::test]
async fn allow_all_lets_ordinary_input_to_ordinary_applications_through() {
    let d = desktop();
    let s = server(&d);
    let shot = describe(&s, TOOL_SCREENSHOT, json!({})).await.unwrap();
    let none = BTreeSet::new();
    assert_eq!(
        evaluate(&shot, &allow_all(true), &none),
        Decision::AllowedByMode
    );
    screenshot(&s).await;
    for (tool, a) in [
        (TOOL_CLICK, json!({"coordinate": [100, 100]})),
        (TOOL_MOVE, json!({"coordinate": [100, 100]})),
        (
            TOOL_SCROLL,
            json!({"coordinate": [100, 100], "direction": "down"}),
        ),
        (TOOL_DRAG, json!({"from": [100, 100], "to": [200, 200]})),
        (TOOL_TYPE, json!({"text": "hello"})),
        (TOOL_KEY, json!({"keys": "ctrl+s"})),
    ] {
        let action = describe(&s, tool, a).await.unwrap();
        assert_eq!(action.risk, Risk::Normal, "{tool}");
        assert_eq!(action.keep_asking, None, "{tool}");
        assert!(!action.network, "{tool}");
        for untrusted in [false, true] {
            assert_eq!(
                evaluate(&action, &allow_all(untrusted), &none),
                Decision::AllowedByMode,
                "{tool}"
            );
        }
    }
    // This app's own window: denied, never a card, never allowed by the mode.
    let own = describe(&s, TOOL_CLICK, json!({"coordinate": [700, 300]}))
        .await
        .unwrap();
    assert!(matches!(
        evaluate(&own, &allow_all(false), &none),
        Decision::Denied { .. }
    ));
}

/// Deleting files by key in a file manager asks in every mode. Other keyboard input to
/// the same window is ordinary input. Typing into a web browser is typing into a page: it
/// asks once untrusted content is in the turn, as the browser tool's typing does.
#[tokio::test]
async fn allow_all_still_asks_before_deleting_files_by_key_and_typing_into_a_browser() {
    let none = BTreeSet::new();
    let d = desktop();
    let mut w = FakeWindow::new(9, Rect::new(0.0, 0.0, 500.0, 500.0), 70, app("nautilus"));
    w.colour = [5, 5, 5];
    d.push_front(w);
    let s = server(&d);
    screenshot(&s).await;

    let delete = describe(&s, TOOL_KEY, json!({"keys": "Delete"}))
        .await
        .unwrap();
    assert_eq!(delete.risk, Risk::AlwaysAsk);
    assert!(delete.preview.contains("deletes the selected files"));
    for untrusted in [false, true] {
        assert_eq!(evaluate(&delete, &allow_all(untrusted), &none), STILL_ASKS);
    }

    let enter = describe(&s, TOOL_KEY, json!({"keys": "Enter"}))
        .await
        .unwrap();
    let typing = describe(&s, TOOL_TYPE, json!({"text": "notes"}))
        .await
        .unwrap();
    for action in [&enter, &typing] {
        assert_eq!(action.keep_asking, None);
        assert!(!action.network);
        for untrusted in [false, true] {
            assert_eq!(
                evaluate(action, &allow_all(untrusted), &none),
                Decision::AllowedByMode
            );
        }
    }

    // A web browser in front: what is typed can go to any site.
    for exe in ["chrome", "msedge", "firefox"] {
        let d = desktop();
        let mut w = FakeWindow::new(9, Rect::new(0.0, 0.0, 500.0, 500.0), 70, app(exe));
        w.colour = [5, 5, 5];
        d.push_front(w);
        let s = server(&d);
        screenshot(&s).await;
        for (tool, a) in [
            (TOOL_TYPE, json!({"text": "notes"})),
            (TOOL_KEY, json!({"keys": "Enter"})),
        ] {
            let action = describe(&s, tool, a).await.unwrap();
            assert_eq!(action.keep_asking, None, "{exe} {tool}");
            assert!(action.network, "{exe} {tool}");
            assert_eq!(
                evaluate(&action, &allow_all(false), &none),
                Decision::AllowedByMode,
                "{exe} {tool}"
            );
            assert!(
                matches!(
                    evaluate(&action, &allow_all(true), &none),
                    Decision::Ask { .. }
                ),
                "{exe} {tool}"
            );
        }
        // A click sends no text.
        let click = describe(&s, TOOL_CLICK, json!({"coordinate": [100, 100]}))
            .await
            .unwrap();
        assert!(!click.network);
        assert_eq!(
            evaluate(&click, &allow_all(true), &none),
            Decision::AllowedByMode
        );
    }

    // The Delete key in an application that is not a file manager is ordinary input.
    let d = desktop();
    let mut w = FakeWindow::new(9, Rect::new(0.0, 0.0, 500.0, 500.0), 70, app("notepad"));
    w.colour = [5, 5, 5];
    d.push_front(w);
    let s = server(&d);
    screenshot(&s).await;
    let in_editor = describe(&s, TOOL_KEY, json!({"keys": "Delete"}))
        .await
        .unwrap();
    assert_eq!(in_editor.risk, Risk::Normal);
    assert_eq!(in_editor.keep_asking, None);
}

/// D7's never-covered list keeps asking in Allow all: terminals and shell hosts, system
/// settings, password managers, the Windows shell's command entry, the system's own
/// shortcuts, and a drag into another application.
#[tokio::test]
async fn allow_all_still_asks_for_protected_applications() {
    let none = BTreeSet::new();
    let check = |action: &Action, what: &str| {
        assert_eq!(action.risk, Risk::AlwaysAsk, "{what}");
        assert_eq!(
            action.keep_asking,
            Some(policy::KeepAsking::ProtectedApp),
            "{what}"
        );
        for untrusted in [false, true] {
            assert_eq!(
                evaluate(action, &allow_all(untrusted), &none),
                STILL_ASKS,
                "{what}"
            );
        }
    };
    for exe in [
        "cmd",
        "powershell",
        "WindowsTerminal",
        "SystemSettings",
        "KeePassXC",
    ] {
        let d = desktop();
        let mut w = FakeWindow::new(9, Rect::new(0.0, 0.0, 500.0, 500.0), 70, app(exe));
        w.colour = [5, 5, 5];
        d.push_front(w);
        let s = server(&d);
        screenshot(&s).await;
        let click = describe(&s, TOOL_CLICK, json!({"coordinate": [100, 100]}))
            .await
            .unwrap();
        check(&click, exe);
        let typing = describe(&s, TOOL_TYPE, json!({"text": "del x"}))
            .await
            .unwrap();
        check(&typing, exe);
        let key = describe(&s, TOOL_KEY, json!({"keys": "Enter"}))
            .await
            .unwrap();
        check(&key, exe);
    }

    // A console window, whatever program runs in it.
    let d = desktop();
    d.push_front(python_window(20, Rect::new(0.0, 0.0, 500.0, 500.0), true));
    let s = server(&d);
    screenshot(&s).await;
    let click = describe(&s, TOOL_CLICK, json!({"coordinate": [100, 100]}))
        .await
        .unwrap();
    check(&click, "a console window");

    // Typing into the Windows shell (the Run box, the address bar).
    let d = desktop();
    let mut explorer = FakeWindow::new(
        30,
        Rect::new(0.0, 0.0, 500.0, 500.0),
        95,
        AppInfo::from_windows_path(r"C:\Windows\explorer.exe"),
    );
    explorer.colour = [3, 3, 3];
    d.push_front(explorer);
    let s = server(&d);
    screenshot(&s).await;
    let typing = describe(&s, TOOL_TYPE, json!({"text": "cmd /c calc"}))
        .await
        .unwrap();
    check(&typing, "typing into the Windows shell");

    // The system's own shortcuts, and the Windows key held during a click.
    let d = desktop();
    let s = server(&d);
    screenshot(&s).await;
    let run_box = describe(&s, TOOL_KEY, json!({"keys": "win+r"}))
        .await
        .unwrap();
    check(&run_box, "Win+R");
    let win_click = describe(
        &s,
        TOOL_CLICK,
        json!({"coordinate": [100, 100], "modifiers": ["cmd"]}),
    )
    .await
    .unwrap();
    check(&win_click, "a click holding the Windows key");

    // A drag from one application into another.
    let d = desktop();
    let mut mail = FakeWindow::new(8, Rect::new(0.0, 500.0, 300.0, 300.0), 61, app("mail"));
    mail.colour = [9, 9, 9];
    d.state().windows.insert(0, mail);
    let s = server(&d);
    screenshot(&s).await;
    let drag = describe(&s, TOOL_DRAG, json!({"from": [100, 100], "to": [100, 600]}))
        .await
        .unwrap();
    check(&drag, "a drag into another application");
}

/// The guard lets input reach a target that always asks only for an action the user
/// approved on its own card. An always-ask action that does not keep asking could have
/// been let through by Allow all, so it does not count as approved.
#[tokio::test]
async fn an_input_that_allow_all_could_let_through_never_reaches_a_protected_target() {
    let d = desktop();
    d.push_front(python_window(20, Rect::new(0.0, 0.0, 500.0, 500.0), true));
    let s = server(&d);
    screenshot(&s).await;
    let approved = describe(&s, TOOL_CLICK, json!({"coordinate": [100, 100]}))
        .await
        .unwrap();
    assert_eq!(approved.keep_asking, Some(policy::KeepAsking::ProtectedApp));
    // Without the mark that keeps it asking, the same action is refused at the console.
    let mut unmarked = approved.clone();
    unmarked.keep_asking = None;
    assert_eq!(
        evaluate(&unmarked, &allow_all(false), &BTreeSet::new()),
        Decision::AllowedByMode
    );
    d.clear_events();
    let out = s.call(TOOL_CLICK, &args(json!({})), &unmarked, ctx()).await;
    assert!(out.is_error, "{}", out.joined_text());
    assert!(buttons(&d).is_empty());
    // Approved on its card, it goes through, as before.
    d.clear_events();
    let out = s.call(TOOL_CLICK, &args(json!({})), &approved, ctx()).await;
    assert!(!out.is_error, "{}", out.joined_text());
    assert_eq!(buttons(&d).len(), 2);
}
