//! What the tools do to a page: read it, find elements, click, type, press keys, take
//! screenshots, navigate (ARCHITECTURE.md 9.3). Element work runs in an isolated world,
//! so the page's own scripts cannot see or change it.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::intent::ElementInfo;
use crate::keys::KeyPress;
use crate::netpolicy;
use crate::session::{Browser, CMD, RefTarget, Wait, isolated_world, resolve_node};
use crate::snapshot::{self, Rect, RenderContext};

/// What the tool learns about an element, run on the element in an isolated world. Form
/// properties are read through the prototypes, because a form's named controls shadow its
/// own properties (`<input name="action">` would replace `form.action`).
pub(crate) const ELEMENT_INFO_JS: &str = r#"function() {
  let el = this.nodeType === 1 ? this : this.parentElement;
  if (!el) return null;
  const sel = 'button, input, select, textarea, a[href], label, summary, [role=button], [role=link], [role=menuitem], [role=menuitemcheckbox], [role=menuitemradio], [role=tab], [role=option], [role=checkbox], [role=radio], [role=switch], [contenteditable=""], [contenteditable=true]';
  const hit = el.closest(sel);
  if (hit) el = hit;
  if (el.tagName === 'LABEL' && el.control) el = el.control;
  const attr = (e, n) => Element.prototype.getAttribute.call(e, n);
  const tag = el.tagName.toLowerCase();
  // Buttons and inputs: the type the browser acts on (a missing or unknown button type is
  // "submit", an unknown input type is "text"), not the attribute as written.
  const type = ((tag === 'button' || tag === 'input') ? String(el.type || '') : (attr(el, 'type') || '')).toLowerCase();
  const role = (attr(el, 'role') || '').toLowerCase();
  const form = el.form || el.closest('form');
  const isInput = tag === 'input';
  const submit = !!form && ((tag === 'button' && (type === '' || type === 'submit')) || (isInput && (type === 'submit' || type === 'image')));
  const buttonLike = tag === 'button' || tag === 'summary' || (isInput && ['button', 'submit', 'reset', 'image'].includes(type)) || ['button', 'menuitem', 'menuitemcheckbox', 'menuitemradio'].includes(role);
  const textTypes = ['', 'text', 'email', 'password', 'search', 'tel', 'url', 'number', 'date', 'datetime-local', 'month', 'week', 'time'];
  const editable = (isInput && textTypes.includes(type)) || tag === 'textarea' || el.isContentEditable;
  let method = null, action = null;
  if (form) {
    const fp = HTMLFormElement.prototype;
    const get = (n) => Object.getOwnPropertyDescriptor(fp, n).get.call(form);
    method = get('method');
    action = get('action');
    if (submit && el.hasAttribute('formmethod')) method = el.formMethod;
    if (submit && el.hasAttribute('formaction')) action = el.formAction;
  }
  const text = (s) => (s || '').replace(/\s+/g, ' ').trim().slice(0, 160);
  const label = text(attr(el, 'aria-label')) || (isInput ? text(el.value) || text(el.placeholder) : '') || text(el.innerText) || text(attr(el, 'title')) || text(attr(el, 'alt')) || text(el.textContent);
  // The form's default button (its first submit control, in tree order): Enter in one of
  // the form's fields presses it.
  let submitLabel = '';
  if (form) {
    const controls = Object.getOwnPropertyDescriptor(HTMLFormElement.prototype, 'elements').get.call(form);
    for (let i = 0; i < controls.length && i < 2000; i++) {
      const b = controls[i];
      const bTag = b.tagName.toLowerCase();
      const bType = String(b.type || '').toLowerCase();
      const isSubmit = (bTag === 'button' && (bType === '' || bType === 'submit')) || (bTag === 'input' && (bType === 'submit' || bType === 'image'));
      if (isSubmit && !b.disabled) {
        submitLabel = text(attr(b, 'aria-label')) || (bTag === 'input' ? text(b.value) : '') || text(b.innerText) || text(attr(b, 'title')) || text(attr(b, 'alt')) || text(b.textContent);
        break;
      }
    }
  }
  return {
    tag, inputType: type, role, label, formSubmitLabel: submitLabel,
    isSubmitControl: submit, inForm: !!form, formMethod: method, formAction: action,
    isButtonLike: buttonLike, isEditable: editable,
    isLink: tag === 'a' && el.hasAttribute('href'), href: tag === 'a' ? el.href : null,
    disabled: !!el.disabled || attr(el, 'aria-disabled') === 'true',
    checked: (type === 'checkbox' || type === 'radio') ? !!el.checked : null
  };
}"#;

/// Divides screenshot (device) pixels by the device pixel ratio to get CSS pixels.
pub fn device_to_css(x: f64, y: f64, dpr: f64) -> (f64, f64) {
    let dpr = if dpr.is_finite() && dpr > 0.0 {
        dpr
    } else {
        1.0
    };
    (x / dpr, y / dpr)
}

/// The centre of the first quad (8 numbers, CSS pixels) that has a real area.
pub fn quad_center(quads: &[Vec<f64>]) -> Option<(f64, f64)> {
    quads.iter().find_map(|q| {
        if q.len() < 8 {
            return None;
        }
        let xs = [q[0], q[2], q[4], q[6]];
        let ys = [q[1], q[3], q[5], q[7]];
        // Shoelace area.
        let area = 0.5
            * ((xs[0] * ys[1] - xs[1] * ys[0])
                + (xs[1] * ys[2] - xs[2] * ys[1])
                + (xs[2] * ys[3] - xs[3] * ys[2])
                + (xs[3] * ys[0] - xs[0] * ys[3]))
                .abs();
        (area >= 1.0).then(|| (xs.iter().sum::<f64>() / 4.0, ys.iter().sum::<f64>() / 4.0))
    })
}

/// The bounding box of one quad (8 numbers, CSS pixels).
pub fn quad_bounds(q: &[f64]) -> Option<Rect> {
    if q.len() < 8 {
        return None;
    }
    let xs = [q[0], q[2], q[4], q[6]];
    let ys = [q[1], q[3], q[5], q[7]];
    let min = |v: [f64; 4]| v.into_iter().fold(f64::INFINITY, f64::min);
    let max = |v: [f64; 4]| v.into_iter().fold(f64::NEG_INFINITY, f64::max);
    let (x, y) = (min(xs), min(ys));
    Some(Rect {
        x,
        y,
        w: max(xs) - x,
        h: max(ys) - y,
    })
}

/// Width and height from a base64 PNG's header.
pub fn png_size(b64: &str) -> Option<(u32, u32)> {
    use base64::Engine;
    let head: String = b64.chars().take(44).collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(head.as_bytes())
        .ok()?;
    if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((w, h))
}

fn cdp_err(e: crate::cdp::CdpError) -> String {
    match e {
        crate::cdp::CdpError::Closed => {
            "The browser closed. Call browser_navigate to open it again.".into()
        }
        other => other.to_string(),
    }
}

/// The focused element and the frame that holds it.
#[derive(Debug, Clone)]
pub(crate) struct Focus {
    pub info: ElementInfo,
    /// Session of the frame holding the focused element.
    pub session: String,
    /// That frame's id (the tab id for the main frame).
    pub frame: String,
}

/// Where the main frame of a tab is.
#[derive(Debug, Clone)]
pub(crate) struct TabInfo {
    pub session: String,
    pub url: String,
}

impl Browser {
    pub(crate) fn tab_info(&self, tab: &str) -> Result<TabInfo, String> {
        let s = self.lock();
        let t = s
            .tab(tab)
            .ok_or("That browser tab was closed. Take a new snapshot.")?;
        if t.crashed {
            return Err("The page crashed. Navigate again to reload it.".into());
        }
        let url = s.main_frame(tab).map(|f| f.url.clone()).unwrap_or_default();
        Ok(TabInfo {
            session: t.session.clone(),
            url,
        })
    }

    /// The session of a tab's main frame, crashed or not: navigating is what reloads a
    /// crashed tab, so navigation must not be refused for it.
    pub(crate) fn tab_session(&self, tab: &str) -> Result<String, String> {
        self.lock()
            .tab(tab)
            .map(|t| t.session.clone())
            .ok_or_else(|| "That browser tab was closed. Take a new snapshot.".into())
    }

    /// The frame's address and origin as the page itself reports them right now (13.3:
    /// each read, click and type is checked against the frame's actual origin).
    pub(crate) async fn actual_origin(
        &self,
        session: &str,
        frame_id: &str,
    ) -> Result<(String, String), String> {
        let ctx = isolated_world(&self.cdp, session, frame_id)
            .await
            .map_err(|e| format!("could not read the frame's address ({})", cdp_err(e)))?;
        let r = self
            .cdp
            .call(
                "Runtime.evaluate",
                json!({"expression": "[location.href, self.origin]", "contextId": ctx, "returnByValue": true}),
                Some(session),
                CMD,
            )
            .await
            .map_err(cdp_err)?;
        let v = r.pointer("/result/value").cloned().unwrap_or(Value::Null);
        let href = v.get(0).and_then(Value::as_str).unwrap_or("").to_string();
        let self_origin = v.get(1).and_then(Value::as_str).unwrap_or("null");
        let mut origin = netpolicy::origin_of_str(&href);
        if origin == "null" {
            origin = netpolicy::origin_of_str(self_origin);
        }
        Ok((href, origin))
    }

    async fn call_on(
        &self,
        target: &RefTarget,
        function: &str,
        args: Vec<Value>,
    ) -> Result<Value, String> {
        let ctx = isolated_world(&self.cdp, &target.session, &target.frame_id)
            .await
            .map_err(|e| format!("the element's frame is gone ({})", cdp_err(e)))?;
        let object = resolve_node(&self.cdp, &target.session, target.backend, ctx)
            .await
            .map_err(|_| {
                "That element is no longer on the page. Take a new snapshot.".to_string()
            })?;
        let r = self
            .cdp
            .call(
                "Runtime.callFunctionOn",
                json!({"objectId": object, "functionDeclaration": function, "arguments": args, "returnByValue": true}),
                Some(&target.session),
                CMD,
            )
            .await
            .map_err(cdp_err)?;
        if let Some(ex) = r.get("exceptionDetails") {
            let text = ex
                .pointer("/exception/description")
                .or_else(|| ex.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("script error");
            return Err(format!("could not inspect the element ({text})"));
        }
        Ok(r.pointer("/result/value").cloned().unwrap_or(Value::Null))
    }

    /// What an element is (for D8 intent and the card).
    pub(crate) async fn element_info(
        &self,
        target: &RefTarget,
    ) -> Result<(ElementInfo, Option<bool>), String> {
        let v = self.call_on(target, ELEMENT_INFO_JS, vec![]).await?;
        let checked = v.get("checked").and_then(Value::as_bool);
        let info: ElementInfo = serde_json::from_value(v)
            .map_err(|_| "That element is not something the tool can act on.".to_string())?;
        Ok((info, checked))
    }

    /// The focused element of the tab and the frame that holds it, looking through shadow
    /// roots and frames, cross-site ones included (their own session is asked in turn), so
    /// a key press is judged and scoped where it lands. Focus inside a frame that cannot
    /// be looked into is marked `opaque_frame`.
    pub(crate) async fn focused_info(&self, tab: &str) -> Result<Option<Focus>, String> {
        let info = self.tab_info(tab)?;
        let mut session = info.session;
        let mut frame = tab.to_string();
        let opaque = |session: String, frame: String| Focus {
            info: ElementInfo {
                opaque_frame: true,
                ..Default::default()
            },
            session,
            frame,
        };
        // Returns null, `{info}`, or the element of a frame whose document this world
        // cannot read (looked into next, through its own session).
        let expression = format!(
            r#"(() => {{
  const INFO = {ELEMENT_INFO_JS};
  let a = document.activeElement;
  for (let depth = 0; a && depth < 10; depth++) {{
    if (a.shadowRoot && a.shadowRoot.activeElement) {{ a = a.shadowRoot.activeElement; continue; }}
    if (a.tagName === 'IFRAME' || a.tagName === 'FRAME') {{
      let d = null;
      try {{ d = a.contentDocument; }} catch (e) {{}}
      if (d && d.activeElement && d.activeElement !== d.body) {{ a = d.activeElement; continue; }}
      if (!d) return a;
      return null;
    }}
    break;
  }}
  if (!a || a === document.body || a === document.documentElement) return null;
  return {{ info: INFO.call(a) }};
}})()"#
        );
        for _ in 0..8 {
            let ctx = isolated_world(&self.cdp, &session, &frame)
                .await
                .map_err(cdp_err)?;
            let r = self
                .cdp
                .call(
                    "Runtime.evaluate",
                    json!({"expression": expression, "contextId": ctx, "returnByValue": false}),
                    Some(&session),
                    CMD,
                )
                .await
                .map_err(cdp_err)?;
            let result = r.get("result").cloned().unwrap_or(Value::Null);
            let Some(object) = result.get("objectId").and_then(Value::as_str) else {
                return Ok(None);
            };
            if result.get("subtype").and_then(Value::as_str) == Some("node") {
                let child = self
                    .cdp
                    .call(
                        "DOM.describeNode",
                        json!({"objectId": object}),
                        Some(&session),
                        CMD,
                    )
                    .await
                    .ok()
                    .and_then(|n| {
                        n.pointer("/node/frameId")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    });
                let known = child.and_then(|c| {
                    let s = self.lock();
                    s.frames.get(&c).map(|f| (c.clone(), f.session.clone()))
                });
                match known {
                    Some((child, child_session)) => {
                        session = child_session;
                        frame = child;
                        continue;
                    }
                    None => return Ok(Some(opaque(session, frame))),
                }
            }
            let v = self
                .cdp
                .call(
                    "Runtime.callFunctionOn",
                    json!({"objectId": object, "functionDeclaration": "function(){ return this.info; }", "returnByValue": true}),
                    Some(&session),
                    CMD,
                )
                .await
                .map_err(cdp_err)?;
            let v = v.pointer("/result/value").cloned().unwrap_or(Value::Null);
            return Ok(serde_json::from_value(v).ok().map(|info| Focus {
                info,
                session,
                frame,
            }));
        }
        Ok(Some(opaque(session, frame)))
    }

    /// The element at a point in the tab's viewport (CSS pixels): what a click there
    /// reaches.
    ///
    /// `DOM.getNodeForLocation` hit-tests in document coordinates of the session's root
    /// frame, so the frame's scroll offset is added. A hit on the element that holds a
    /// frame running in another process (a cross-site iframe) continues inside that
    /// frame, so a click is judged, scoped and re-checked on the element it lands on, not
    /// on the `<iframe>` around it.
    pub(crate) async fn element_at(&self, tab: &str, x: f64, y: f64) -> Result<RefTarget, String> {
        let nothing = || "There is no element at that point of the page.".to_string();
        let info = self.tab_info(tab)?;
        let mut session = info.session;
        let mut root = tab.to_string();
        // The point in the viewport of `root`.
        let (mut vx, mut vy) = (x, y);
        for _ in 0..8 {
            let (sx, sy) = self.scroll_offset(&session, &root).await;
            let r = self
                .cdp
                .call(
                    "DOM.getNodeForLocation",
                    json!({
                        "x": (vx + sx).round() as i64,
                        "y": (vy + sy).round() as i64,
                        "includeUserAgentShadowDOM": false,
                    }),
                    Some(&session),
                    CMD,
                )
                .await
                .map_err(|_| nothing())?;
            let backend = r
                .get("backendNodeId")
                .and_then(Value::as_i64)
                .ok_or_else(nothing)?;
            let frame_id = r
                .get("frameId")
                .and_then(Value::as_str)
                .unwrap_or(&root)
                .to_string();
            let hit = RefTarget {
                session: session.clone(),
                frame_id,
                backend,
            };
            let Some((child, child_session)) = self.remote_frame_of(&hit).await else {
                return Ok(hit);
            };
            let content = self.content_box(&hit).await?;
            if !content.contains(vx, vy) {
                // The frame element's border or padding: the click stays outside the frame.
                return Ok(hit);
            }
            vx -= content.x;
            vy -= content.y;
            session = child_session;
            root = child;
        }
        Err(nothing())
    }

    /// The frame scroll offset of `frame` (CSS pixels), 0 when it cannot be read.
    async fn scroll_offset(&self, session: &str, frame: &str) -> (f64, f64) {
        let Ok(ctx) = isolated_world(&self.cdp, session, frame).await else {
            return (0.0, 0.0);
        };
        let v = self
            .cdp
            .call(
                "Runtime.evaluate",
                json!({"expression": "[scrollX, scrollY]", "contextId": ctx, "returnByValue": true}),
                Some(session),
                CMD,
            )
            .await
            .ok()
            .and_then(|r| r.pointer("/result/value").cloned())
            .unwrap_or(Value::Null);
        let n = |i: usize| v.get(i).and_then(Value::as_f64).unwrap_or(0.0);
        (n(0), n(1))
    }

    /// If `owner` is the element of a frame that runs in another session (process), that
    /// frame's id and session.
    async fn remote_frame_of(&self, owner: &RefTarget) -> Option<(String, String)> {
        let node = self
            .cdp
            .call(
                "DOM.describeNode",
                json!({"backendNodeId": owner.backend}),
                Some(&owner.session),
                CMD,
            )
            .await
            .ok()?;
        let child = node.pointer("/node/frameId").and_then(Value::as_str)?;
        let s = self.lock();
        let frame = s.frames.get(child)?;
        (frame.session != owner.session).then(|| (child.to_string(), frame.session.clone()))
    }

    /// A frame element's content box, in the viewport of the session's root frame.
    async fn content_box(&self, owner: &RefTarget) -> Result<Rect, String> {
        let model = self
            .cdp
            .call(
                "DOM.getBoxModel",
                json!({"backendNodeId": owner.backend}),
                Some(&owner.session),
                CMD,
            )
            .await
            .map_err(cdp_err)?;
        let content: Vec<f64> = model
            .pointer("/model/content")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_f64).collect())
            .unwrap_or_default();
        quad_bounds(&content).ok_or_else(|| "could not find the frame on the page".into())
    }

    /// The device pixel ratio of the tab's main frame.
    pub(crate) async fn device_pixel_ratio(&self, tab: &str) -> f64 {
        let Ok(info) = self.tab_info(tab) else {
            return 1.0;
        };
        let Ok(ctx) = isolated_world(&self.cdp, &info.session, tab).await else {
            return 1.0;
        };
        self.cdp
            .call(
                "Runtime.evaluate",
                json!({"expression": "window.devicePixelRatio", "contextId": ctx, "returnByValue": true}),
                Some(&info.session),
                CMD,
            )
            .await
            .ok()
            .and_then(|r| r.pointer("/result/value").and_then(Value::as_f64))
            .unwrap_or(1.0)
    }

    // ----- reading -----------------------------------------------------------------------

    async fn ax_tree(
        &self,
        session: &str,
        frame_id: Option<&str>,
    ) -> Result<Vec<snapshot::AxNode>, String> {
        let params = match frame_id {
            Some(f) => json!({"frameId": f}),
            None => json!({}),
        };
        let r = self
            .cdp
            .call(
                "Accessibility.getFullAXTree",
                params,
                Some(session),
                Duration::from_secs(30),
            )
            .await
            .map_err(|e| format!("could not read the page ({})", cdp_err(e)))?;
        Ok(snapshot::parse_ax_tree(&r))
    }

    async fn viewport_and_bounds(&self, session: &str) -> (Option<Rect>, HashMap<i64, Rect>) {
        let viewport = self
            .cdp
            .call("Page.getLayoutMetrics", json!({}), Some(session), CMD)
            .await
            .ok()
            .and_then(|m| {
                let v = m.get("cssVisualViewport")?;
                Some(Rect {
                    x: v.get("pageX")?.as_f64()?,
                    y: v.get("pageY")?.as_f64()?,
                    w: v.get("clientWidth")?.as_f64()?,
                    h: v.get("clientHeight")?.as_f64()?,
                })
            });
        let bounds = self
            .cdp
            .call(
                "DOMSnapshot.captureSnapshot",
                json!({"computedStyles": []}),
                Some(session),
                Duration::from_secs(30),
            )
            .await
            .map(|r| snapshot::parse_layout_bounds(&r))
            .unwrap_or_default();
        (viewport, bounds)
    }

    /// The accessibility snapshot of the tab, limited to the viewport unless `full`.
    pub(crate) async fn snapshot(
        &self,
        tab: &str,
        full: bool,
        budget: usize,
    ) -> Result<String, String> {
        let info = self.tab_info(tab)?;
        let main = self.ax_tree(&info.session, None).await?;
        let (viewport, bounds) = if full {
            (None, HashMap::new())
        } else {
            self.viewport_and_bounds(&info.session).await
        };
        let title = main.first().map(|n| n.name.clone()).unwrap_or_default();
        // TODO(M6): only frames whose element is outside the view are left out; the
        // content of a visible frame is listed whole (and cut by the result cap).
        let mut children = self.lock().child_frames(tab);
        let mut hidden_frames = 0;
        if let Some(view) = viewport {
            let outside = self
                .frames_outside_view(&info.session, tab, &children, &bounds, view)
                .await;
            hidden_frames = outside.len();
            children.retain(|(id, _)| !outside.contains(id));
        }
        let mut child_trees = Vec::new();
        for (id, frame) in children.iter().take(10) {
            if let Some(why) = &frame.blocked {
                child_trees.push((
                    frame.origin.clone(),
                    Err(why.clone()),
                    id.clone(),
                    frame.session.clone(),
                ));
                continue;
            }
            let same_process = frame.session == info.session;
            let tree = self
                .ax_tree(&frame.session, same_process.then_some(id.as_str()))
                .await;
            child_trees.push((
                frame.origin.clone(),
                tree,
                id.clone(),
                frame.session.clone(),
            ));
        }
        let mut lines = Vec::new();
        let mut hidden = 0;
        {
            let mut s = self.lock();
            let t = s.tab_mut(tab).ok_or("That browser tab was closed.")?;
            let refs = &mut t.refs;
            let main_session = info.session.clone();
            let rendered = snapshot::render_tree(
                &main,
                RenderContext {
                    bounds: &bounds,
                    viewport,
                    depth_offset: 0,
                },
                &mut |b| refs.alloc(&main_session, tab, b),
            );
            lines.extend(rendered.lines);
            hidden += rendered.hidden_interactive;
            let no_bounds = HashMap::new();
            for (origin, tree, frame_id, session) in &child_trees {
                match tree {
                    Err(why) => lines.push(format!(
                        "- iframe \"{}\" [not shown: {}]",
                        snapshot::clean_text(origin, 100),
                        snapshot::clean_text(why, 100)
                    )),
                    Ok(nodes) => {
                        lines.push(format!(
                            "- iframe \"{}\":",
                            snapshot::clean_text(origin, 100)
                        ));
                        let rendered = snapshot::render_tree(
                            nodes,
                            RenderContext {
                                bounds: &no_bounds,
                                viewport: None,
                                depth_offset: 1,
                            },
                            &mut |b| refs.alloc(session, frame_id, b),
                        );
                        lines.extend(rendered.lines);
                    }
                }
            }
        }
        let header = format!(
            "Page \"{}\" at {}\n",
            snapshot::clean_text(&title, 150),
            snapshot::clean_text(&info.url, 300)
        );
        let mut footer = String::new();
        if hidden > 0 || hidden_frames > 0 {
            let frames = if hidden_frames > 0 {
                format!(" and {hidden_frames} embedded frame(s)")
            } else {
                String::new()
            };
            footer.push_str(&format!(
                "[{hidden} more interactive elements{frames} are outside the view: scroll with browser_press_key PageDown, look for one with browser_find, or call browser_snapshot with full=true]\n"
            ));
        }
        if lines.is_empty() {
            lines.push("(nothing readable in view)".into());
        }
        let room = budget.saturating_sub(header.len() + footer.len() + 120);
        let (body, cut) = snapshot::fit_lines(&lines, room);
        let mut out = header;
        out.push_str(&body);
        if cut {
            out.push_str(
                "[snapshot cut to fit; use browser_find to look for something specific]\n",
            );
        }
        out.push_str(&footer);
        Ok(out)
    }

    /// Frames (and their descendants) whose element in the page lies outside `view`.
    async fn frames_outside_view(
        &self,
        main_session: &str,
        tab: &str,
        children: &[(String, crate::session::Frame)],
        bounds: &HashMap<i64, Rect>,
        view: Rect,
    ) -> std::collections::HashSet<String> {
        let mut outside = std::collections::HashSet::new();
        if bounds.is_empty() {
            return outside;
        }
        for (id, frame) in children {
            if frame.parent.as_deref() != Some(tab) {
                continue;
            }
            let owner = self
                .cdp
                .call(
                    "DOM.getFrameOwner",
                    json!({"frameId": id}),
                    Some(main_session),
                    CMD,
                )
                .await
                .ok()
                .and_then(|r| r.get("backendNodeId").and_then(Value::as_i64));
            let visible = owner
                .and_then(|b| bounds.get(&b))
                .is_some_and(|r| r.intersects(&view));
            if !visible {
                outside.insert(id.clone());
            }
        }
        // Descendants of a frame that is left out are left out too.
        loop {
            let more: Vec<String> = children
                .iter()
                .filter(|(id, f)| {
                    !outside.contains(id) && f.parent.as_ref().is_some_and(|p| outside.contains(p))
                })
                .map(|(id, _)| id.clone())
                .collect();
            if more.is_empty() {
                break;
            }
            outside.extend(more);
        }
        outside
    }

    /// Elements matching `query`, across the page and its frames.
    pub(crate) async fn find(
        &self,
        tab: &str,
        query: &str,
        budget: usize,
    ) -> Result<String, String> {
        let info = self.tab_info(tab)?;
        let main = self.ax_tree(&info.session, None).await?;
        let (viewport, bounds) = self.viewport_and_bounds(&info.session).await;
        let children = self.lock().child_frames(tab);
        let mut child_trees = Vec::new();
        for (id, frame) in children.iter().take(10) {
            if frame.blocked.is_some() {
                continue;
            }
            let same = frame.session == info.session;
            if let Ok(tree) = self
                .ax_tree(&frame.session, same.then_some(id.as_str()))
                .await
            {
                child_trees.push((tree, id.clone(), frame.session.clone()));
            }
        }
        let limit = (budget / 80).clamp(5, 50);
        let mut lines = Vec::new();
        let mut total = 0;
        {
            let mut s = self.lock();
            let t = s.tab_mut(tab).ok_or("That browser tab was closed.")?;
            let refs = &mut t.refs;
            let session = info.session.clone();
            let (found, n) = snapshot::find_nodes(
                &main,
                query,
                RenderContext {
                    bounds: &bounds,
                    viewport,
                    depth_offset: 0,
                },
                limit,
                &mut |b| refs.alloc(&session, tab, b),
            );
            lines.extend(found);
            total += n;
            let none = HashMap::new();
            for (tree, frame_id, session) in &child_trees {
                let (found, n) = snapshot::find_nodes(
                    tree,
                    query,
                    RenderContext {
                        bounds: &none,
                        viewport: None,
                        depth_offset: 0,
                    },
                    limit.saturating_sub(lines.len()),
                    &mut |b| refs.alloc(session, frame_id, b),
                );
                lines.extend(found.into_iter().map(|l| format!("{l} (in a frame)")));
                total += n;
            }
        }
        if lines.is_empty() {
            return Ok(format!(
                "Nothing on {} matches \"{}\".",
                snapshot::clean_text(&info.url, 200),
                snapshot::clean_text(query, 100)
            ));
        }
        let (body, _) = snapshot::fit_lines(&lines, budget.saturating_sub(200));
        let mut out = format!(
            "{total} element(s) on {} match \"{}\":\n{body}",
            snapshot::clean_text(&info.url, 200),
            snapshot::clean_text(query, 100)
        );
        if total > lines.len() {
            out.push_str(&format!(
                "[{} more not listed; use more specific words]\n",
                total - lines.len()
            ));
        }
        Ok(out)
    }

    /// Readable text of the page (its main content when it has one) or of one element.
    pub(crate) async fn read_text(
        &self,
        tab: &str,
        target: Option<&RefTarget>,
        offset: usize,
        budget: usize,
    ) -> Result<String, String> {
        let info = self.tab_info(tab)?;
        let text = match target {
            Some(t) => self
                .call_on(
                    t,
                    "function(){ const e = this.nodeType === 1 ? this : this.parentElement; return e ? e.innerText : ''; }",
                    vec![],
                )
                .await?
                .as_str()
                .unwrap_or("")
                .to_string(),
            None => {
                let ctx = isolated_world(&self.cdp, &info.session, tab)
                    .await
                    .map_err(cdp_err)?;
                let expr = "(() => { const b = document.body; if (!b) return ''; const m = document.querySelector('main, article, [role=main]'); const e = (m && m.innerText && m.innerText.trim().length > 400) ? m : b; return e.innerText; })()";
                let r = self
                    .cdp
                    .call(
                        "Runtime.evaluate",
                        json!({"expression": expr, "contextId": ctx, "returnByValue": true}),
                        Some(&info.session),
                        Duration::from_secs(30),
                    )
                    .await
                    .map_err(cdp_err)?;
                r.pointer("/result/value")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string()
            }
        };
        let text = normalise_text(&text);
        let total = text.chars().count();
        let header_room = 300;
        let (page, next) =
            snapshot::page_of_text(&text, offset, budget.saturating_sub(header_room));
        let shown = page.chars().count();
        let mut out = format!(
            "Text of {} (characters {}–{} of {total}):\n{page}\n",
            snapshot::clean_text(&info.url, 200),
            offset.min(total),
            offset.min(total) + shown
        );
        if let Some(next) = next {
            out.push_str(&format!(
                "[more text follows: call browser_read_text with offset={next}]\n"
            ));
        }
        Ok(out)
    }

    // ----- acting ------------------------------------------------------------------------

    /// The offset of a frame's viewport inside the tab's viewport: the content boxes of
    /// the owner iframes, added at every process boundary.
    async fn frame_offset(&self, frame_id: &str) -> Result<(f64, f64), String> {
        let (mut x, mut y) = (0.0, 0.0);
        let mut current = frame_id.to_string();
        for _ in 0..16 {
            let (frame, parent) = {
                let s = self.lock();
                let Some(f) = s.frames.get(&current).cloned() else {
                    break;
                };
                let parent = f.parent.as_ref().and_then(|p| s.frames.get(p).cloned());
                (f, parent)
            };
            let (Some(parent_id), Some(parent)) = (frame.parent.clone(), parent) else {
                break;
            };
            if frame.session != parent.session {
                let owner = self
                    .cdp
                    .call(
                        "DOM.getFrameOwner",
                        json!({"frameId": current}),
                        Some(&parent.session),
                        CMD,
                    )
                    .await
                    .map_err(cdp_err)?;
                let backend = owner
                    .get("backendNodeId")
                    .and_then(Value::as_i64)
                    .ok_or("could not find the frame on the page")?;
                let model = self
                    .cdp
                    .call(
                        "DOM.getBoxModel",
                        json!({"backendNodeId": backend}),
                        Some(&parent.session),
                        CMD,
                    )
                    .await
                    .map_err(cdp_err)?;
                let content: Vec<f64> = model
                    .pointer("/model/content")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_f64).collect())
                    .unwrap_or_default();
                if content.len() >= 2 {
                    x += content[0];
                    y += content[1];
                }
            }
            current = parent_id;
        }
        Ok((x, y))
    }

    /// Scrolls the element into view and returns its centre in the tab's viewport.
    pub(crate) async fn point_of(&self, target: &RefTarget) -> Result<(f64, f64), String> {
        let gone = |_| "That element is no longer on the page. Take a new snapshot.".to_string();
        let _ = self
            .cdp
            .call(
                "DOM.scrollIntoViewIfNeeded",
                json!({"backendNodeId": target.backend}),
                Some(&target.session),
                CMD,
            )
            .await;
        self.wait_for_paint(&target.session, &target.frame_id).await;
        let quads = self
            .cdp
            .call(
                "DOM.getContentQuads",
                json!({"backendNodeId": target.backend}),
                Some(&target.session),
                CMD,
            )
            .await
            .map_err(gone)?;
        let quads: Vec<Vec<f64>> = quads
            .get("quads")
            .and_then(Value::as_array)
            .map(|qs| {
                qs.iter()
                    .filter_map(|q| {
                        q.as_array()
                            .map(|a| a.iter().filter_map(Value::as_f64).collect())
                    })
                    .collect()
            })
            .unwrap_or_default();
        let (cx, cy) = quad_center(&quads)
            .ok_or("That element is not visible on the page (it has no size).")?;
        let (ox, oy) = self.frame_offset(&target.frame_id).await?;
        Ok((cx + ox, cy + oy))
    }

    /// Waits until the frame has painted twice (two animation frames, at most 1 s), so
    /// the browser routes input to what the frame shows now: a frame in another process
    /// that has not painted yet would let a click fall through to the page behind it.
    async fn wait_for_paint(&self, session: &str, frame_id: &str) {
        let Ok(ctx) = isolated_world(&self.cdp, session, frame_id).await else {
            return;
        };
        let _ = self
            .cdp
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": "new Promise(r => { const t = setTimeout(r, 1000); requestAnimationFrame(() => requestAnimationFrame(() => { clearTimeout(t); r(); })); })",
                    "contextId": ctx,
                    "awaitPromise": true,
                }),
                Some(session),
                Duration::from_secs(2),
            )
            .await;
    }

    /// Refuses a click whose point is covered by another element (an overlay could
    /// otherwise take a click meant for something harmless).
    pub(crate) async fn check_not_covered(
        &self,
        tab: &str,
        target: &RefTarget,
        x: f64,
        y: f64,
    ) -> Result<(), String> {
        let hit = self.element_at(tab, x, y).await?;
        if hit.session == target.session && hit.backend == target.backend {
            return Ok(());
        }
        if hit.session != target.session {
            // The hit test stopped at an element of another process. That is fine only if
            // it is the element holding the target's own frame (the frame was not
            // attached when the hit test ran); anything else covers the target.
            let mut current = target.frame_id.clone();
            loop {
                let parent = self
                    .lock()
                    .frames
                    .get(&current)
                    .and_then(|f| f.parent.clone());
                let Some(parent) = parent else { break };
                let parent_session = self.lock().frames.get(&parent).map(|f| f.session.clone());
                if parent_session.as_deref() == Some(hit.session.as_str()) {
                    let owner = self
                        .cdp
                        .call(
                            "DOM.getFrameOwner",
                            json!({"frameId": current}),
                            Some(&hit.session),
                            CMD,
                        )
                        .await
                        .map_err(cdp_err)?;
                    if owner.get("backendNodeId").and_then(Value::as_i64) == Some(hit.backend) {
                        return Ok(());
                    }
                    break;
                }
                current = parent;
            }
            return Err("Another element covers that spot of the page. Scroll, or close what covers it, and take a new snapshot.".into());
        }
        if hit.frame_id != target.frame_id {
            return Err(
                "Another frame covers that element. Close what covers it and take a new snapshot."
                    .into(),
            );
        }
        let ctx = isolated_world(&self.cdp, &target.session, &target.frame_id)
            .await
            .map_err(cdp_err)?;
        let t = resolve_node(&self.cdp, &target.session, target.backend, ctx)
            .await
            .map_err(cdp_err)?;
        let h = resolve_node(&self.cdp, &target.session, hit.backend, ctx)
            .await
            .map_err(cdp_err)?;
        let r = self
            .cdp
            .call(
                "Runtime.callFunctionOn",
                json!({
                    "objectId": t,
                    "functionDeclaration": "function(h){ const me = this.nodeType === 1 ? this : this.parentElement; if (!me) return false; if (me === h || me.contains(h)) return true; const l = h.nodeType === 1 ? h.closest('label') : null; return !!(l && l.control === me); }",
                    "arguments": [{"objectId": h}],
                    "returnByValue": true
                }),
                Some(&target.session),
                CMD,
            )
            .await
            .map_err(cdp_err)?;
        if r.pointer("/result/value").and_then(Value::as_bool) == Some(true) {
            Ok(())
        } else {
            Err("Another element covers that element on the page. Scroll, or close what covers it, and take a new snapshot.".into())
        }
    }

    /// A left click at a point of the tab's viewport (CSS pixels).
    pub(crate) async fn click_at(&self, tab: &str, x: f64, y: f64) -> Result<(), String> {
        let info = self.tab_info(tab)?;
        for (kind, buttons, count) in [
            ("mouseMoved", 0, 0),
            ("mousePressed", 1, 1),
            ("mouseReleased", 0, 1),
        ] {
            let mut params = json!({"type": kind, "x": x, "y": y, "buttons": buttons});
            if kind != "mouseMoved" {
                params["button"] = json!("left");
                params["clickCount"] = json!(count);
            }
            self.cdp
                .call("Input.dispatchMouseEvent", params, Some(&info.session), CMD)
                .await
                .map_err(cdp_err)?;
        }
        Ok(())
    }

    /// Clicks `target` at `(x, y)`. In a cross-site frame (another process) the browser
    /// routes input by what that process last painted; a frame that has not painted yet
    /// lets the click fall through to the page behind it. There the click is confirmed by
    /// a `mousedown` listener on the element (in the isolated world) and tried once more
    /// if it did not arrive. A click that falls through reaches only the frame's own
    /// element in the page, which does nothing.
    pub(crate) async fn click_element(
        &self,
        tab: &str,
        target: &RefTarget,
        x: f64,
        y: f64,
    ) -> Result<(), String> {
        let info = self.tab_info(tab)?;
        if target.session == info.session {
            return self.click_at(tab, x, y).await;
        }
        for attempt in 0..2 {
            let armed = self.arm_mousedown(target).await;
            self.click_at(tab, x, y).await?;
            let Some(promise) = armed else {
                return Ok(());
            };
            let landed = self
                .cdp
                .call(
                    "Runtime.awaitPromise",
                    json!({"promiseObjectId": promise, "returnByValue": true}),
                    Some(&target.session),
                    Duration::from_secs(3),
                )
                .await
                .ok()
                .and_then(|r| r.pointer("/result/value").and_then(Value::as_bool))
                .unwrap_or(true);
            if landed {
                return Ok(());
            }
            if attempt == 0 {
                self.wait_for_paint(&target.session, &target.frame_id).await;
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
        }
        Err("The click did not reach the element inside the embedded frame (it was not ready). Take a new snapshot and try again.".into())
    }

    /// Installs a one-shot `mousedown` listener on the element and returns the id of a
    /// promise that resolves to whether it fired (false after 1.5 s).
    async fn arm_mousedown(&self, target: &RefTarget) -> Option<String> {
        let ctx = isolated_world(&self.cdp, &target.session, &target.frame_id)
            .await
            .ok()?;
        let object = resolve_node(&self.cdp, &target.session, target.backend, ctx)
            .await
            .ok()?;
        let r = self
            .cdp
            .call(
                "Runtime.callFunctionOn",
                json!({
                    "objectId": object,
                    "functionDeclaration": "function(){ const me = this.nodeType === 1 ? this : this.parentElement; return new Promise(r => { const t = setTimeout(() => r(false), 1500); me.addEventListener('mousedown', () => { clearTimeout(t); r(true); }, {once: true, capture: true}); }); }",
                    "returnByValue": false,
                    "awaitPromise": false,
                }),
                Some(&target.session),
                CMD,
            )
            .await
            .ok()?;
        r.pointer("/result/objectId")
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// Focuses the element, optionally selects what it holds, and inserts `text`.
    pub(crate) async fn type_into(
        &self,
        tab: &str,
        target: &RefTarget,
        text: &str,
        clear: bool,
    ) -> Result<(), String> {
        let info = self.tab_info(tab)?;
        let _ = self
            .cdp
            .call(
                "DOM.scrollIntoViewIfNeeded",
                json!({"backendNodeId": target.backend}),
                Some(&target.session),
                CMD,
            )
            .await;
        self.cdp
            .call(
                "DOM.focus",
                json!({"backendNodeId": target.backend}),
                Some(&target.session),
                CMD,
            )
            .await
            .map_err(|e| format!("could not focus that field ({})", cdp_err(e)))?;
        let focused = self
            .call_on(
                target,
                "function(clear){ let a = document.activeElement; while (a && a.shadowRoot && a.shadowRoot.activeElement) a = a.shadowRoot.activeElement; const me = this.nodeType === 1 ? this : this.parentElement; if (a !== me && !(me && me.contains(a))) return false; if (clear) { if (typeof me.select === 'function' && 'value' in me) { me.select(); } else if (me.isContentEditable) { const r = document.createRange(); r.selectNodeContents(me); const s = getSelection(); s.removeAllRanges(); s.addRange(r); } } return true; }",
                vec![json!({"value": clear})],
            )
            .await?;
        if focused.as_bool() != Some(true) {
            return Err(
                "That field did not take the focus (the page moved it). Take a new snapshot."
                    .into(),
            );
        }
        if text.is_empty() {
            if clear {
                self.press(tab, &crate::keys::parse_key("Delete")?).await?;
            }
            return Ok(());
        }
        self.cdp
            .call(
                "Input.insertText",
                json!({"text": text}),
                Some(&info.session),
                CMD,
            )
            .await
            .map_err(cdp_err)?;
        Ok(())
    }

    /// Chooses an option of a `<select>` by value or label.
    pub(crate) async fn select_option(
        &self,
        target: &RefTarget,
        value: &str,
    ) -> Result<(), String> {
        let ok = self
            .call_on(
                target,
                "function(v){ const s = this; if (!s.options) return 'not-select'; const o = [...s.options].find(o => o.value === v || o.label === v || o.text.trim() === v); if (!o) return 'no-option'; s.value = o.value; s.dispatchEvent(new Event('input', {bubbles: true})); s.dispatchEvent(new Event('change', {bubbles: true})); return 'ok'; }",
                vec![json!({"value": value})],
            )
            .await?;
        match ok.as_str() {
            Some("ok") => Ok(()),
            Some("no-option") => Err(format!(
                "that list has no option \"{}\"",
                snapshot::clean_text(value, 80)
            )),
            _ => Err("that element is not a drop-down list".into()),
        }
    }

    /// Presses and releases one key (with modifiers) in the tab.
    pub(crate) async fn press(&self, tab: &str, key: &KeyPress) -> Result<(), String> {
        let info = self.tab_info(tab)?;
        for params in key.events() {
            self.cdp
                .call("Input.dispatchKeyEvent", params, Some(&info.session), CMD)
                .await
                .map_err(cdp_err)?;
        }
        Ok(())
    }

    /// A PNG of the viewport (base64) and the device pixel ratio.
    pub(crate) async fn screenshot(&self, tab: &str) -> Result<(String, f64), String> {
        let info = self.tab_info(tab)?;
        let r = self
            .cdp
            .call(
                "Page.captureScreenshot",
                json!({"format": "png", "captureBeyondViewport": false}),
                Some(&info.session),
                Duration::from_secs(30),
            )
            .await
            .map_err(cdp_err)?;
        let data = r
            .get("data")
            .and_then(Value::as_str)
            .ok_or("the browser returned no image")?
            .to_string();
        let dpr = self.device_pixel_ratio(tab).await;
        if let Some(t) = self.lock().tab_mut(tab) {
            t.screenshot_dpr = Some(dpr);
        }
        Ok((data, dpr))
    }

    /// Navigates the tab and waits for the page to load.
    pub(crate) async fn navigate(
        &self,
        tab: &str,
        url: &str,
        cancel: &CancellationToken,
    ) -> Result<Option<String>, String> {
        let session = self.tab_session(tab)?;
        let started = Instant::now();
        let stop_before = self.lock().tab(tab).map(|t| t.stop_seq).unwrap_or(0);
        let budget = self.config.navigation_timeout + self.config.approval_timeout;
        let r = self
            .cdp
            .call_cancellable(
                "Page.navigate",
                json!({"url": url}),
                Some(&session),
                budget,
                cancel,
            )
            .await
            .map_err(cdp_err)?;
        if let Some(error) = r
            .get("errorText")
            .and_then(Value::as_str)
            .filter(|e| !e.is_empty())
        {
            return Err(self.explain_failure(error, started));
        }
        let Some(loader) = r
            .get("loaderId")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            return Ok(None);
        };
        let tab_id = tab.to_string();
        let done = self
            .wait_with_holds(
                tab,
                move |s| {
                    s.tab(&tab_id).is_some_and(|t| {
                        t.loaded_loader.as_deref() == Some(loader.as_str())
                            || t.stop_seq > stop_before
                    })
                },
                cancel,
            )
            .await;
        match done {
            Wait::Done => Ok(None),
            Wait::Timeout => Ok(Some(format!(
                "The page is still loading after {} s; what is shown may be incomplete.",
                self.config.navigation_timeout.as_secs()
            ))),
            Wait::Cancelled => Err("Stopped by the user.".into()),
            Wait::Closed => Err("The browser closed.".into()),
        }
    }

    /// Why a navigation failed, in words.
    pub(crate) fn explain_failure(&self, error: &str, since: Instant) -> String {
        let block = self.lock().block_since(since);
        match (error, block) {
            (_, Some(why)) => format!("The navigation was blocked: {why}."),
            ("net::ERR_NAME_NOT_RESOLVED", None) => {
                "That address does not exist (the name could not be resolved).".into()
            }
            (other, None) => format!("The page could not be loaded ({other})."),
        }
    }

    /// Waits for `cond` with the navigation timeout, extended while a request of the tab
    /// is held for approval.
    pub(crate) async fn wait_with_holds(
        &self,
        tab: &str,
        cond: impl Fn(&crate::session::State) -> bool,
        cancel: &CancellationToken,
    ) -> Wait {
        let mut deadline = Instant::now() + self.config.navigation_timeout;
        let hard_stop = deadline + self.config.approval_timeout;
        loop {
            match self.wait_for(&cond, deadline, cancel).await {
                Wait::Timeout
                    if self.lock().held_for_tab(tab) > 0 && Instant::now() < hard_stop =>
                {
                    deadline = Instant::now() + Duration::from_secs(1);
                }
                other => return other,
            }
        }
    }

    /// After a click or key press: if the main frame started navigating within a short
    /// window, waits for it to settle. Returns whether it navigated.
    pub(crate) async fn settle_after_action(
        &self,
        tab: &str,
        nav_before: u64,
        stop_before: u64,
        cancel: &CancellationToken,
    ) -> Result<bool, String> {
        let tab_id = tab.to_string();
        let started = self
            .wait_for(
                |s| {
                    s.tab(&tab_id).is_some_and(|t| t.nav_seq > nav_before)
                        || s.held_for_tab(&tab_id) > 0
                },
                Instant::now() + Duration::from_millis(700),
                cancel,
            )
            .await;
        match started {
            Wait::Cancelled => return Err("Stopped by the user.".into()),
            Wait::Closed => return Err("The browser closed.".into()),
            Wait::Timeout => return Ok(false),
            Wait::Done => {}
        }
        // Settled: nothing held, the main frame not loading, and either it stopped
        // loading since the action or nothing happened for a moment (a download, a
        // cancelled or blocked navigation never stops loading because it never started).
        let quiet = Duration::from_millis(800);
        let mut deadline = Instant::now() + self.config.navigation_timeout;
        let hard_stop = deadline + self.config.approval_timeout;
        loop {
            let (settled, held) = {
                let s = self.lock();
                match s.tab(tab) {
                    None => (true, 0),
                    Some(t) => {
                        let held = s.held_for_tab(tab);
                        let calm = t.last_nav_event.is_none_or(|at| at.elapsed() >= quiet);
                        (
                            held == 0 && !t.loading && (t.stop_seq > stop_before || calm),
                            held,
                        )
                    }
                }
            };
            if settled {
                return Ok(true);
            }
            if self.cdp.is_closed() {
                return Err("The browser closed.".into());
            }
            let now = Instant::now();
            if now >= deadline {
                if held > 0 && now < hard_stop {
                    deadline = now + Duration::from_secs(1);
                } else {
                    return Ok(true);
                }
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                _ = cancel.cancelled() => return Err("Stopped by the user.".into()),
            }
        }
    }

    /// The previous history entry of the tab: (entry id, URL).
    pub(crate) async fn previous_entry(&self, tab: &str) -> Result<Option<(i64, String)>, String> {
        let info = self.tab_info(tab)?;
        let h = self
            .cdp
            .call(
                "Page.getNavigationHistory",
                json!({}),
                Some(&info.session),
                CMD,
            )
            .await
            .map_err(cdp_err)?;
        let index = h.get("currentIndex").and_then(Value::as_i64).unwrap_or(0);
        if index <= 0 {
            return Ok(None);
        }
        let entry = h
            .get("entries")
            .and_then(Value::as_array)
            .and_then(|e| e.get((index - 1) as usize));
        Ok(entry.and_then(|e| Some((e.get("id")?.as_i64()?, e.get("url")?.as_str()?.to_string()))))
    }

    /// Goes to a history entry and waits for it to settle.
    pub(crate) async fn go_to_entry(
        &self,
        tab: &str,
        entry: i64,
        cancel: &CancellationToken,
    ) -> Result<(), String> {
        let info = self.tab_info(tab)?;
        let (nav_before, stop_before) = {
            let s = self.lock();
            s.tab(tab)
                .map(|t| (t.nav_seq, t.stop_seq))
                .unwrap_or((0, 0))
        };
        self.cdp
            .call_cancellable(
                "Page.navigateToHistoryEntry",
                json!({"entryId": entry}),
                Some(&info.session),
                CMD,
                cancel,
            )
            .await
            .map_err(cdp_err)?;
        self.settle_after_action(tab, nav_before, stop_before, cancel)
            .await
            .map(|_| ())
    }
}

/// Collapses runs of blank lines and trailing spaces in page text.
pub fn normalise_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank = 0;
    for line in text.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_pixels_are_divided_by_the_ratio() {
        assert_eq!(device_to_css(200.0, 100.0, 2.0), (100.0, 50.0));
        assert_eq!(device_to_css(150.0, 75.0, 1.5), (100.0, 50.0));
        assert_eq!(
            device_to_css(10.0, 10.0, 0.0),
            (10.0, 10.0),
            "bad ratio means 1"
        );
        assert_eq!(device_to_css(10.0, 10.0, f64::NAN), (10.0, 10.0));
    }

    #[test]
    fn quad_centres_skip_empty_quads() {
        let empty = vec![5.0, 5.0, 5.0, 5.0, 5.0, 5.0, 5.0, 5.0];
        let real = vec![10.0, 10.0, 30.0, 10.0, 30.0, 20.0, 10.0, 20.0];
        assert_eq!(quad_center(&[empty.clone(), real]), Some((20.0, 15.0)));
        assert_eq!(quad_center(&[empty]), None);
        assert_eq!(quad_center(&[vec![1.0, 2.0]]), None);
        assert_eq!(quad_center(&[]), None);
    }

    #[test]
    fn quad_bounds_and_containment() {
        let r = quad_bounds(&[100.0, 50.0, 300.0, 50.0, 300.0, 150.0, 100.0, 150.0]).unwrap();
        assert_eq!(
            r,
            Rect {
                x: 100.0,
                y: 50.0,
                w: 200.0,
                h: 100.0
            }
        );
        assert!(r.contains(100.0, 50.0));
        assert!(r.contains(299.0, 149.0));
        assert!(!r.contains(300.0, 100.0), "the right edge is outside");
        assert!(!r.contains(99.0, 100.0));
        assert!(!r.contains(150.0, 151.0));
        assert_eq!(quad_bounds(&[1.0, 2.0]), None);
    }

    #[test]
    fn png_header_size() {
        use base64::Engine;
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&1280u32.to_be_bytes());
        png.extend_from_slice(&800u32.to_be_bytes());
        png.extend_from_slice(&[8, 6, 0, 0, 0]);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        assert_eq!(png_size(&b64), Some((1280, 800)));
        assert_eq!(png_size("bm90IGEgcG5n"), None);
    }

    #[test]
    fn text_is_normalised() {
        assert_eq!(normalise_text("a  \n\n\n\nb\n  \nc  "), "a\n\nb\n\nc");
    }
}
