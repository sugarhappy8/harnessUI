//! Accessibility-tree snapshots with element references (ARCHITECTURE.md 9.3):
//! `- button "Sign in" [ref=e12]`, limited to the viewport by default and sized to the
//! per-result cap. Pure logic: the session fetches the tree
//! (`Accessibility.getFullAXTree`) and the layout boxes (`DOMSnapshot.captureSnapshot`)
//! and hands them here.
//!
//! Everything in a snapshot is page content: untrusted data, returned as plain tool output.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

/// Characters per token used to size page text to the per-result cap.
pub const CHARS_PER_TOKEN: f64 = 3.5;

/// The character budget for a result, from the chat's per-result cap in tokens.
pub fn budget_chars(result_cap_tokens: usize) -> usize {
    let tokens = result_cap_tokens.max(500) as f64;
    (tokens * CHARS_PER_TOKEN) as usize
}

/// A box in CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    /// Left.
    pub x: f64,
    /// Top.
    pub y: f64,
    /// Width.
    pub w: f64,
    /// Height.
    pub h: f64,
}

impl Rect {
    /// Whether the point lies in the box (left and top edges in, right and bottom out).
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    /// Whether the two boxes overlap (an empty box overlaps nothing).
    pub fn intersects(&self, other: &Rect) -> bool {
        self.w > 0.0
            && self.h > 0.0
            && other.w > 0.0
            && other.h > 0.0
            && self.x < other.x + other.w
            && other.x < self.x + self.w
            && self.y < other.y + other.h
            && other.y < self.y + self.h
    }

    /// The smallest box holding both.
    pub fn union(&self, other: &Rect) -> Rect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let right = (self.x + self.w).max(other.x + other.w);
        let bottom = (self.y + self.h).max(other.y + other.h);
        Rect {
            x,
            y,
            w: right - x,
            h: bottom - y,
        }
    }
}

/// One node of the accessibility tree.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AxNode {
    /// The tree's own node id.
    pub id: String,
    /// Ignored for accessibility (its children may not be).
    pub ignored: bool,
    /// The role (`button`, `link`, `StaticText`, `RootWebArea`…).
    pub role: String,
    /// The accessible name.
    pub name: String,
    /// The value (text fields, sliders).
    pub value: Option<String>,
    /// Properties such as `focusable`, `checked`, `level`, as strings.
    pub props: Vec<(String, String)>,
    /// Child node ids, in order.
    pub children: Vec<String>,
    /// The DOM node behind it, used for clicks and typing.
    pub backend_id: Option<i64>,
}

impl AxNode {
    fn prop(&self, name: &str) -> Option<&str> {
        self.props
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    fn flag(&self, name: &str) -> bool {
        self.prop(name) == Some("true")
    }
}

fn value_string(v: &Value) -> Option<String> {
    match v.get("value")? {
        Value::String(s) => Some(s.clone()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Parses the result of `Accessibility.getFullAXTree`. Nodes it cannot read are skipped.
pub fn parse_ax_tree(result: &Value) -> Vec<AxNode> {
    let Some(nodes) = result.get("nodes").and_then(Value::as_array) else {
        return Vec::new();
    };
    nodes
        .iter()
        .filter_map(|n| {
            let id = n.get("nodeId")?.as_str()?.to_string();
            let props = n
                .get("properties")
                .and_then(Value::as_array)
                .map(|ps| {
                    ps.iter()
                        .filter_map(|p| {
                            let name = p.get("name")?.as_str()?.to_string();
                            let value = p.get("value").and_then(value_string)?;
                            Some((name, value))
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(AxNode {
                id,
                ignored: n.get("ignored").and_then(Value::as_bool).unwrap_or(false),
                role: n.get("role").and_then(value_string).unwrap_or_default(),
                name: n.get("name").and_then(value_string).unwrap_or_default(),
                value: n
                    .get("value")
                    .and_then(value_string)
                    .filter(|v| !v.is_empty()),
                props,
                children: n
                    .get("childIds")
                    .and_then(Value::as_array)
                    .map(|c| {
                        c.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default(),
                backend_id: n.get("backendDOMNodeId").and_then(Value::as_i64),
            })
        })
        .collect()
}

/// Layout boxes by backend node id, from `DOMSnapshot.captureSnapshot` (document
/// coordinates in CSS pixels). Only the first document (the frame itself) is read.
pub fn parse_layout_bounds(result: &Value) -> HashMap<i64, Rect> {
    let mut out: HashMap<i64, Rect> = HashMap::new();
    let Some(doc) = result
        .get("documents")
        .and_then(Value::as_array)
        .and_then(|d| d.first())
    else {
        return out;
    };
    let backend: Vec<i64> = doc
        .pointer("/nodes/backendNodeId")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|v| v.as_i64().unwrap_or(-1)).collect())
        .unwrap_or_default();
    let (Some(index), Some(bounds)) = (
        doc.pointer("/layout/nodeIndex").and_then(Value::as_array),
        doc.pointer("/layout/bounds").and_then(Value::as_array),
    ) else {
        return out;
    };
    for (i, node_index) in index.iter().enumerate() {
        let Some(node_index) = node_index.as_u64() else {
            continue;
        };
        let Some(&id) = backend.get(node_index as usize) else {
            continue;
        };
        let Some(b) = bounds.get(i).and_then(Value::as_array) else {
            continue;
        };
        let n = |k: usize| b.get(k).and_then(Value::as_f64).unwrap_or(0.0);
        let rect = Rect {
            x: n(0),
            y: n(1),
            w: n(2),
            h: n(3),
        };
        out.entry(id)
            .and_modify(|r| *r = r.union(&rect))
            .or_insert(rect);
    }
    out
}

/// Roles that get a reference: things the agent can click or type into.
const INTERACTIVE: &[&str] = &[
    "button",
    "link",
    "textbox",
    "searchbox",
    "combobox",
    "checkbox",
    "radio",
    "switch",
    "slider",
    "spinbutton",
    "tab",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "option",
    "treeitem",
    "listbox",
    "textarea",
    "ListBoxOption",
    "MenuListOption",
    "PopUpButton",
    "ToggleButton",
    "DisclosureTriangle",
];

/// Roles that add nothing to read on their own.
const TRANSPARENT: &[&str] = &[
    "none",
    "generic",
    "presentation",
    "InlineTextBox",
    "LineBreak",
    "GenericContainer",
    "LayoutTable",
    "LayoutTableRow",
    "LayoutTableCell",
    "Section",
    "paragraph",
    "group",
    "Div",
];

/// Roles whose values are worth showing.
const VALUED: &[&str] = &[
    "textbox",
    "searchbox",
    "combobox",
    "slider",
    "spinbutton",
    "textarea",
    "progressbar",
    "meter",
];

/// The longest name or value shown, in characters.
const MAX_NAME: usize = 120;

fn is_interactive(node: &AxNode) -> bool {
    INTERACTIVE.contains(&node.role.as_str())
        || (node.flag("focusable")
            && !matches!(
                node.role.as_str(),
                "RootWebArea" | "WebArea" | "Iframe" | "IframePresentational" | "ScrollArea"
            ))
}

/// Collapses whitespace, drops control characters, escapes quotes and cuts to `max`.
pub fn clean_text(text: &str, max: usize) -> String {
    let mut out = String::new();
    let mut space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        if c.is_control() {
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        if c == '"' {
            out.push('\\');
        }
        out.push(c);
    }
    if out.chars().count() > max {
        let cut: String = out.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    } else {
        out
    }
}

/// What the renderer needs besides the tree.
#[derive(Debug, Clone, Copy)]
pub struct RenderContext<'a> {
    /// Layout boxes by backend node id.
    pub bounds: &'a HashMap<i64, Rect>,
    /// Only nodes in this box are listed; `None` lists everything.
    pub viewport: Option<Rect>,
    /// Indentation added to every line (frames nested in a page).
    pub depth_offset: usize,
}

/// A rendered tree.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rendered {
    /// One line per element.
    pub lines: Vec<String>,
    /// Interactive elements left out because they are outside the viewport.
    pub hidden_interactive: usize,
}

struct Renderer<'a, 'r> {
    by_id: HashMap<&'a str, &'a AxNode>,
    ctx: RenderContext<'a>,
    refs: &'r mut dyn FnMut(i64) -> String,
    hidden: usize,
    visiting: HashSet<&'a str>,
}

impl<'a> Renderer<'a, '_> {
    fn visible(&self, node: &AxNode) -> bool {
        let Some(viewport) = self.ctx.viewport else {
            return true;
        };
        if self.ctx.bounds.is_empty() {
            return true;
        }
        node.backend_id
            .and_then(|id| self.ctx.bounds.get(&id))
            .is_some_and(|b| b.intersects(&viewport))
    }

    fn line(&mut self, node: &AxNode, depth: usize) -> String {
        let indent = "  ".repeat(depth + self.ctx.depth_offset);
        let name = clean_text(&node.name, MAX_NAME);
        if node.role == "StaticText" {
            return format!("{indent}- text: \"{name}\"");
        }
        let mut s = format!("{indent}- {}", node.role);
        if !name.is_empty() {
            s.push_str(&format!(" \"{name}\""));
        }
        if let Some(level) = node.prop("level") {
            s.push_str(&format!(" [level={level}]"));
        }
        for (prop, shown) in [
            ("checked", "checked"),
            ("pressed", "pressed"),
            ("selected", "selected"),
        ] {
            match node.prop(prop) {
                Some("true") => s.push_str(&format!(" [{shown}]")),
                Some("mixed") => s.push_str(&format!(" [{shown}=mixed]")),
                _ => {}
            }
        }
        match node.prop("expanded") {
            Some("true") => s.push_str(" [expanded]"),
            Some("false") => s.push_str(" [collapsed]"),
            _ => {}
        }
        for flag in ["disabled", "required", "focused", "readonly"] {
            if node.flag(flag) {
                s.push_str(&format!(" [{flag}]"));
            }
        }
        if VALUED.contains(&node.role.as_str())
            && let Some(value) = &node.value
        {
            s.push_str(&format!(" [value=\"{}\"]", clean_text(value, MAX_NAME)));
        }
        if node.role == "link"
            && let Some(url) = node.prop("url")
        {
            s.push_str(&format!(" [url={}]", clean_text(url, 80)));
        }
        if is_interactive(node)
            && let Some(id) = node.backend_id
        {
            let r = (self.refs)(id);
            s.push_str(&format!(" [ref={r}]"));
        }
        s
    }

    /// The lines for `id` and its subtree at `depth`.
    fn visit(&mut self, id: &'a str, depth: usize, parent_name: &str) -> Vec<String> {
        let Some(node) = self.by_id.get(id).copied() else {
            return Vec::new();
        };
        if !self.visiting.insert(id) {
            return Vec::new();
        }
        let role = node.role.as_str();
        let transparent = node.ignored
            || role == "RootWebArea"
            || role == "WebArea"
            || (TRANSPARENT.contains(&role)
                && node.name.trim().is_empty()
                && !is_interactive(node));
        let mut out = Vec::new();
        if role == "StaticText" {
            let text = clean_text(&node.name, MAX_NAME);
            if !text.is_empty() && text != clean_text(parent_name, MAX_NAME) && self.visible(node) {
                out.push(self.line(node, depth));
            }
            self.visiting.remove(id);
            return out;
        }
        let child_depth = if transparent { depth } else { depth + 1 };
        let name_for_children = if transparent {
            parent_name
        } else {
            node.name.as_str()
        };
        let mut children = Vec::new();
        for child in &node.children {
            children.extend(self.visit(child.as_str(), child_depth, name_for_children));
        }
        if transparent {
            self.visiting.remove(id);
            return children;
        }
        let interactive = is_interactive(node);
        let visible = self.visible(node);
        if !children.is_empty() || (visible && (interactive || !node.name.trim().is_empty())) {
            out.push(self.line(node, depth));
            out.extend(children);
        } else if interactive && !visible {
            self.hidden += 1;
        }
        self.visiting.remove(id);
        out
    }
}

/// Renders the tree (the first node is the root) as indented lines with references.
/// `refs` turns a backend node id into a reference such as `e12`.
pub fn render_tree(
    nodes: &[AxNode],
    ctx: RenderContext<'_>,
    refs: &mut dyn FnMut(i64) -> String,
) -> Rendered {
    let Some(root) = nodes.first() else {
        return Rendered::default();
    };
    let mut r = Renderer {
        by_id: nodes.iter().map(|n| (n.id.as_str(), n)).collect(),
        ctx,
        refs,
        hidden: 0,
        visiting: HashSet::new(),
    };
    let lines = r.visit(root.id.as_str(), 0, "");
    Rendered {
        lines,
        hidden_interactive: r.hidden,
    }
}

/// The elements whose role, name or value contains every word of `query`
/// (case-insensitive), one line each with a reference, at most `limit`. Elements outside
/// the viewport are marked.
pub fn find_nodes(
    nodes: &[AxNode],
    query: &str,
    ctx: RenderContext<'_>,
    limit: usize,
    refs: &mut dyn FnMut(i64) -> String,
) -> (Vec<String>, usize) {
    let words: Vec<String> = query
        .to_lowercase()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    if words.is_empty() {
        return (Vec::new(), 0);
    }
    let mut out = Vec::new();
    let mut total = 0;
    let mut seen_text: HashSet<String> = HashSet::new();
    for node in nodes {
        if node.ignored || TRANSPARENT.contains(&node.role.as_str()) && node.name.is_empty() {
            continue;
        }
        if matches!(
            node.role.as_str(),
            "RootWebArea" | "WebArea" | "InlineTextBox"
        ) {
            continue;
        }
        let hay = format!(
            "{} {} {}",
            node.role.to_lowercase(),
            node.name.to_lowercase(),
            node.value.as_deref().unwrap_or("").to_lowercase()
        );
        if !words.iter().all(|w| hay.contains(w.as_str())) {
            continue;
        }
        // The same sentence often appears as a paragraph and as its text node.
        if !seen_text.insert(format!("{}|{}", node.role, node.name)) {
            continue;
        }
        total += 1;
        if out.len() >= limit {
            continue;
        }
        let name = clean_text(&node.name, MAX_NAME);
        let role = if node.role == "StaticText" {
            "text"
        } else {
            node.role.as_str()
        };
        let mut s = format!("- {role}");
        if !name.is_empty() {
            s.push_str(&format!(" \"{name}\""));
        }
        if let Some(value) = &node.value {
            s.push_str(&format!(" [value=\"{}\"]", clean_text(value, MAX_NAME)));
        }
        if let Some(id) = node.backend_id {
            let r = refs(id);
            s.push_str(&format!(" [ref={r}]"));
            let offscreen = match ctx.viewport {
                Some(v) if !ctx.bounds.is_empty() => {
                    !ctx.bounds.get(&id).is_some_and(|b| b.intersects(&v))
                }
                _ => false,
            };
            if offscreen {
                s.push_str(" (outside the view)");
            }
        }
        out.push(s);
    }
    (out, total)
}

/// Joins `lines` into at most `budget` characters, cutting at a line boundary. Returns the
/// text and whether lines were left out.
pub fn fit_lines(lines: &[String], budget: usize) -> (String, bool) {
    let mut out = String::new();
    for line in lines {
        if out.len() + line.len() + 1 > budget {
            return (out, true);
        }
        out.push_str(line);
        out.push('\n');
    }
    (out, false)
}

/// Cuts plain text to `budget` characters starting at character `offset`, at a word
/// boundary where possible. Returns the slice and the offset after it, if more remains.
pub fn page_of_text(text: &str, offset: usize, budget: usize) -> (String, Option<usize>) {
    let chars: Vec<char> = text.chars().collect();
    if offset >= chars.len() {
        return (String::new(), None);
    }
    let end = (offset + budget).min(chars.len());
    let mut cut = end;
    if end < chars.len() {
        // Back up to the last whitespace in the final tenth of the page.
        let floor = offset + budget * 9 / 10;
        if let Some(pos) = (floor..end).rev().find(|&i| chars[i].is_whitespace()) {
            cut = pos;
        }
    }
    let slice: String = chars[offset..cut].iter().collect();
    let next = (cut < chars.len()).then_some(cut);
    (slice, next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tree() -> Value {
        json!({"nodes": [
            {"nodeId": "1", "ignored": false, "role": {"type": "internalRole", "value": "RootWebArea"},
             "name": {"type": "computedString", "value": "Shop"}, "childIds": ["2", "3", "6", "8", "10", "12"], "backendDOMNodeId": 1},
            {"nodeId": "2", "ignored": false, "role": {"type": "role", "value": "heading"},
             "name": {"type": "computedString", "value": "Welcome"},
             "properties": [{"name": "level", "value": {"type": "integer", "value": 1}}],
             "childIds": ["20"], "backendDOMNodeId": 2},
            {"nodeId": "20", "ignored": false, "role": {"type": "role", "value": "StaticText"},
             "name": {"type": "computedString", "value": "Welcome"}, "childIds": [], "backendDOMNodeId": 20},
            {"nodeId": "3", "ignored": true, "role": {"type": "role", "value": "none"}, "childIds": ["4"], "backendDOMNodeId": 3},
            {"nodeId": "4", "ignored": false, "role": {"type": "role", "value": "generic"}, "childIds": ["5"], "backendDOMNodeId": 4},
            {"nodeId": "5", "ignored": false, "role": {"type": "role", "value": "button"},
             "name": {"type": "computedString", "value": "Sign \"in\"\n now"},
             "properties": [{"name": "focusable", "value": {"type": "booleanOrUndefined", "value": true}}],
             "childIds": [], "backendDOMNodeId": 5},
            {"nodeId": "6", "ignored": false, "role": {"type": "role", "value": "textbox"},
             "name": {"type": "computedString", "value": "Email"},
             "value": {"type": "string", "value": "a@b.c"},
             "properties": [{"name": "required", "value": {"type": "boolean", "value": true}},
                            {"name": "focused", "value": {"type": "boolean", "value": true}}],
             "childIds": [], "backendDOMNodeId": 6},
            {"nodeId": "8", "ignored": false, "role": {"type": "role", "value": "checkbox"},
             "name": {"type": "computedString", "value": "Remember me"},
             "properties": [{"name": "checked", "value": {"type": "tristate", "value": "true"}}],
             "childIds": [], "backendDOMNodeId": 8},
            {"nodeId": "10", "ignored": false, "role": {"type": "role", "value": "link"},
             "name": {"type": "computedString", "value": "Far away"},
             "properties": [{"name": "url", "value": {"type": "string", "value": "https://shop.example/far"}}],
             "childIds": [], "backendDOMNodeId": 10},
            {"nodeId": "12", "ignored": false, "role": {"type": "role", "value": "paragraph"},
             "childIds": ["13"], "backendDOMNodeId": 12},
            {"nodeId": "13", "ignored": false, "role": {"type": "role", "value": "StaticText"},
             "name": {"type": "computedString", "value": "Free   shipping"}, "childIds": [], "backendDOMNodeId": 13}
        ]})
    }

    fn bounds() -> HashMap<i64, Rect> {
        let r = |x: f64, y: f64| Rect {
            x,
            y,
            w: 100.0,
            h: 20.0,
        };
        [
            (
                1,
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 1000.0,
                    h: 5000.0,
                },
            ),
            (2, r(0.0, 0.0)),
            (20, r(0.0, 0.0)),
            (5, r(0.0, 40.0)),
            (6, r(0.0, 80.0)),
            (8, r(0.0, 120.0)),
            (10, r(0.0, 3000.0)),
            (12, r(0.0, 160.0)),
            (13, r(0.0, 160.0)),
        ]
        .into_iter()
        .collect()
    }

    fn refs() -> impl FnMut(i64) -> String {
        |id| format!("e{id}")
    }

    #[test]
    fn renders_roles_names_states_and_refs() {
        let nodes = parse_ax_tree(&tree());
        let b = bounds();
        let mut alloc = refs();
        let out = render_tree(
            &nodes,
            RenderContext {
                bounds: &b,
                viewport: None,
                depth_offset: 0,
            },
            &mut alloc,
        );
        assert_eq!(
            out.lines,
            vec![
                "- heading \"Welcome\" [level=1]",
                "- button \"Sign \\\"in\\\" now\" [ref=e5]",
                "- textbox \"Email\" [required] [focused] [value=\"a@b.c\"] [ref=e6]",
                "- checkbox \"Remember me\" [checked] [ref=e8]",
                "- link \"Far away\" [url=https://shop.example/far] [ref=e10]",
                "- text: \"Free shipping\"",
            ]
        );
        assert_eq!(out.hidden_interactive, 0);
    }

    #[test]
    fn the_viewport_limits_what_is_listed_and_counts_the_rest() {
        let nodes = parse_ax_tree(&tree());
        let b = bounds();
        let mut alloc = refs();
        let out = render_tree(
            &nodes,
            RenderContext {
                bounds: &b,
                viewport: Some(Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 1280.0,
                    h: 800.0,
                }),
                depth_offset: 1,
            },
            &mut alloc,
        );
        assert!(out.lines.iter().all(|l| !l.contains("Far away")));
        assert_eq!(out.hidden_interactive, 1);
        assert!(out.lines[0].starts_with("  - heading"));
    }

    #[test]
    fn find_matches_every_word_and_marks_offscreen_elements() {
        let nodes = parse_ax_tree(&tree());
        let b = bounds();
        let mut alloc = refs();
        let viewport = Some(Rect {
            x: 0.0,
            y: 0.0,
            w: 1280.0,
            h: 800.0,
        });
        let ctx = RenderContext {
            bounds: &b,
            viewport,
            depth_offset: 0,
        };
        let (lines, total) = find_nodes(&nodes, "far", ctx, 10, &mut alloc);
        assert_eq!(total, 1);
        assert_eq!(
            lines,
            vec!["- link \"Far away\" [ref=e10] (outside the view)"]
        );
        let (lines, _) = find_nodes(&nodes, "EMAIL textbox", ctx, 10, &mut alloc);
        assert_eq!(
            lines,
            vec!["- textbox \"Email\" [value=\"a@b.c\"] [ref=e6]"]
        );
        let (lines, total) = find_nodes(&nodes, "welcome", ctx, 10, &mut alloc);
        // The heading and its text node with the same words, both listed once each.
        assert_eq!(total, 2, "{lines:?}");
        let (lines, total) = find_nodes(&nodes, "e", ctx, 1, &mut alloc);
        assert_eq!(lines.len(), 1);
        assert!(total > 1);
        assert!(find_nodes(&nodes, "  ", ctx, 10, &mut alloc).0.is_empty());
    }

    #[test]
    fn layout_bounds_map_backend_ids() {
        let snap = json!({"documents": [{
            "nodes": {"backendNodeId": [1, 2, 3]},
            "layout": {"nodeIndex": [0, 2, 2], "bounds": [[0, 0, 10, 10], [5, 5, 1, 1], [7, 7, 3, 3]]}
        }], "strings": []});
        let b = parse_layout_bounds(&snap);
        assert_eq!(b.len(), 2);
        assert_eq!(
            b[&3],
            Rect {
                x: 5.0,
                y: 5.0,
                w: 5.0,
                h: 5.0
            }
        );
        assert!(parse_layout_bounds(&json!({})).is_empty());
    }

    #[test]
    fn rect_math() {
        let a = Rect {
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
        };
        assert!(a.intersects(&Rect {
            x: 9.0,
            y: 9.0,
            w: 5.0,
            h: 5.0
        }));
        assert!(!a.intersects(&Rect {
            x: 10.0,
            y: 0.0,
            w: 5.0,
            h: 5.0
        }));
        assert!(!a.intersects(&Rect {
            x: 1.0,
            y: 1.0,
            w: 0.0,
            h: 5.0
        }));
    }

    #[test]
    fn budgets_and_cutting() {
        assert_eq!(budget_chars(1000), 3500);
        assert_eq!(budget_chars(10), 1750, "at least 500 tokens");
        let lines: Vec<String> = (0..10).map(|i| format!("line {i}")).collect();
        let (text, cut) = fit_lines(&lines, 20);
        assert_eq!(text, "line 0\nline 1\n");
        assert!(cut);
        let (text, cut) = fit_lines(&lines[..2], 1000);
        assert_eq!(text, "line 0\nline 1\n");
        assert!(!cut);
    }

    #[test]
    fn text_pages_cut_at_words() {
        let text = "alpha beta gamma delta epsilon";
        let (page, next) = page_of_text(text, 0, 12);
        assert_eq!(page, "alpha beta");
        assert_eq!(next, Some(10));
        let (page, next) = page_of_text(text, 10, 100);
        assert_eq!(page, " gamma delta epsilon");
        assert_eq!(next, None);
        assert_eq!(page_of_text(text, 100, 10), (String::new(), None));
    }

    #[test]
    fn clean_text_collapses_and_escapes() {
        assert_eq!(clean_text("  a\n\tb  \"c\"\u{7}", 50), "a b \\\"c\\\"");
        assert_eq!(clean_text("abcdef", 4), "abc…");
    }

    #[test]
    fn cycles_in_a_hostile_tree_do_not_hang() {
        let t = json!({"nodes": [
            {"nodeId": "1", "role": {"value": "RootWebArea"}, "childIds": ["2"]},
            {"nodeId": "2", "role": {"value": "list"}, "name": {"value": "L"}, "childIds": ["1", "2"]}
        ]});
        let nodes = parse_ax_tree(&t);
        let b = HashMap::new();
        let mut alloc = refs();
        let out = render_tree(
            &nodes,
            RenderContext {
                bounds: &b,
                viewport: None,
                depth_offset: 0,
            },
            &mut alloc,
        );
        assert_eq!(out.lines, vec!["- list \"L\""]);
    }
}
