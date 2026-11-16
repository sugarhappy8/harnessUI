//! The copy's `index.html`, inert by construction (ARCHITECTURE.md 9.3, 13.4).
//!
//! The page script's own clean-up (`js/collect.js`) is a first pass only. A page controls
//! its own DOM: it can make elements in invented namespaces, replace its root, or give an
//! element an attribute named like a tag, and a serialised DOM does not always parse back
//! into the tree it came from. So the HTML that pass produced is parsed here the way a
//! browser parses the saved file (html5ever, scripting enabled), which builds exactly the
//! elements a browser opening the file would build, and that tree is cleaned:
//!
//! - elements that run code, load other documents or change how addresses are read are
//!   removed (`script`, `noscript`, `iframe` (kept as an empty box), `embed`, `object`,
//!   `frame`, `base`, SVG `foreignObject`, animations that set an address, legacy raw-text
//!   elements, `<meta>` other than the copy's own, `<link>` other than style sheets and
//!   icons that are the copy's own files), and so are comments;
//! - attributes are removed when they are event handlers, have a made-up name or make the
//!   browser send something (`ping`, `action`, `attributionsrc`…); an attribute that holds a
//!   resource address keeps it only when it names one of the copy's own files (or, for a
//!   picture, a `data:image/` address); a link keeps only a fragment or an `http(s)`,
//!   `mailto:` or `tel:` address; any other attribute whose value would reach outside the
//!   folder (`file:`, `//host`, `http:`, `javascript:`…) is removed; CSS in `style`
//!   attributes, style elements and presentation attributes goes through
//!   [`css::confine`].
//!
//! The result is written with a real `<html><head>` whose first child is the copy's policy
//! ([`COPY_POLICY`], kept as defence in depth). Then the last-resort check: the output is
//! parsed again and must have that root and that head, and cleaning it again must change
//! nothing. Otherwise the copy is not saved.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use dom_query::{Document, NodeData, NodeId, NodeRef};
use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::tree_builder::{QuirksMode, TreeBuilderOpts};
#[allow(unused_imports)]
use html5ever::namespace_url;
use html5ever::{Attribute, LocalName, ParseOpts, QualName, ns};
use url::Url;

use crate::capture::css;
use crate::capture::html::COPY_POLICY;

/// The comment the copy's head carries after its policy.
pub const COMMENT: &str = " Saved by harnessUI for studying the design. The page's scripts are switched off in this copy. ";

/// Where the copy is taken to be when its addresses are checked: `index.html` in a folder
/// of its own, so a relative address that leaves that folder (or names a host) is seen.
const ROOT: &str = "file:///hcap-copy/";

/// The sub-folders of the capture a page may point at.
const LINKABLE: &[&str] = &["assets/", "styles/"];

/// Time the parser gets for a page; what it has not read by then is left out.
const PARSE_BUDGET: Duration = Duration::from_secs(20);
const PARSE_CHUNK: usize = 64 * 1024;

/// Elements removed with everything in them, in any namespace.
const DROPPED: &[&str] = &[
    "script",
    "noscript",
    "embed",
    "object",
    "applet",
    "frame",
    "frameset",
    "portal",
    "fencedframe",
    "base",
    "param",
    "xmp",
    "noembed",
    "noframes",
    "plaintext",
    "foreignobject",
    "handler",
    "listener",
];

/// Attributes removed wherever they are: they send something, run something, or only
/// matter for what is not in the copy.
const DROPPED_ATTRS: &[&str] = &[
    "action",
    "formaction",
    "ping",
    "attributionsrc",
    "srcdoc",
    "nonce",
    "integrity",
    "crossorigin",
    "longdesc",
    "lowsrc",
    "dynsrc",
    "manifest",
    "codebase",
    "classid",
    "archive",
    "profile",
    "icon",
    "sharedstoragewritable",
    "browsingtopics",
    "adauctionheaders",
    "privatetoken",
    "http-equiv",
    "imagesrcset",
    "imagesizes",
    "download",
    "shadowrootdelegatesfocus",
    "shadowrootclonable",
    "shadowrootserializable",
    "shadowrootcustomelementregistry",
];

/// SVG animation elements, which may set any attribute of another element.
const SVG_ANIMATIONS: &[&str] = &[
    "set",
    "animate",
    "animatetransform",
    "animatemotion",
    "animatecolor",
];

/// Values that reach outside the copy's folder or run something when used as an address.
const OUTSIDE: &[&str] = &[
    "javascript:",
    "vbscript:",
    "file:",
    "http:",
    "https:",
    "ftp:",
    "ws:",
    "wss:",
    "blob:",
    "filesystem:",
    "data:text/html",
    "data:application/xhtml",
    "data:text/xml",
    "data:application/xml",
];

/// The doctype the copy starts with, so it renders in the mode the page did.
fn doctype(mode: QuirksMode) -> &'static str {
    match mode {
        QuirksMode::NoQuirks => "<!DOCTYPE html>\n",
        QuirksMode::LimitedQuirks => {
            "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Transitional//EN\" \"http://www.w3.org/TR/xhtml1/DTD/xhtml1-transitional.dtd\">\n"
        }
        QuirksMode::Quirks => "",
    }
}

/// Parses HTML the way a browser parses a file it opens (scripting enabled, so the inside
/// of `<noscript>` is text). `false` with the tree when the time ran out first.
pub fn parse(html: &str) -> (Document, bool) {
    let opts = ParseOpts {
        tree_builder: TreeBuilderOpts {
            scripting_enabled: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut parser = html5ever::parse_document(Document::default(), opts);
    let started = Instant::now();
    let mut rest = html;
    let mut whole = true;
    while !rest.is_empty() {
        let mut cut = rest.len().min(PARSE_CHUNK);
        while !rest.is_char_boundary(cut) {
            cut += 1;
        }
        let (chunk, tail) = rest.split_at(cut);
        parser.process(StrTendril::from(chunk));
        rest = tail;
        if !rest.is_empty() && started.elapsed() > PARSE_BUDGET {
            whole = false;
            break;
        }
    }
    (parser.finish(), whole)
}

/// The copy's `index.html` from the page script's HTML: parsed again, cleaned, with the
/// given SVG files (already cleaned) copied in as hidden shapes, and checked. `files` are
/// the capture's files (paths inside the capture folder, with `/`). `Err` says why no
/// safe copy could be made; then none is saved.
pub fn make_inert(html: &str, sprites: &[String], files: &HashSet<String>) -> Result<Inert, String> {
    let (doc, whole) = parse(html);
    let mode = doc.quirks_mode.get();
    let (_, _, body) = structure(&doc)?;
    if !sprites.is_empty() {
        let holder = doc.tree.new_element("div");
        holder.set_attr("hidden", "");
        holder.set_attr("aria-hidden", "true");
        holder.set_attr(
            "style",
            "position:absolute;width:0;height:0;overflow:hidden",
        );
        body.prepend_child(&holder);
        for svg in sprites {
            holder.append_html(svg.as_str());
        }
    }
    let out = finish(&doc, files, mode)?;
    verify(&out, files, mode)?;
    Ok(Inert { html: out, whole })
}

/// A copy made by [`make_inert`].
#[derive(Debug)]
pub struct Inert {
    pub html: String,
    /// False when the page was too large to parse in time and its end was left out.
    pub whole: bool,
}

/// Cleans the tree, puts the policy first in the head and writes it out.
fn finish(doc: &Document, files: &HashSet<String>, mode: QuirksMode) -> Result<String, String> {
    clean(doc, files);
    let (html, head, _) = structure(doc)?;
    // Only the root element stays at the top (the doctype is written below).
    for child in doc.root().children() {
        if child.id != html.id {
            child.remove_from_parent();
        }
    }
    for child in head.children() {
        let ours = child.query(|n| match &n.data {
            NodeData::Comment { contents } => contents.as_ref() == COMMENT,
            NodeData::Element(e) => {
                e.name.local.as_ref() == "meta"
                    && e.attrs.iter().any(|a| {
                        let n = a.name.local.as_ref();
                        n.eq_ignore_ascii_case("charset") || n.eq_ignore_ascii_case("http-equiv")
                    })
            }
            _ => false,
        });
        if ours == Some(true) {
            child.remove_from_parent();
        }
    }
    let comment = doc.tree.create_node(NodeData::Comment {
        contents: StrTendril::from(COMMENT),
    });
    head.prepend_child(&comment);
    let charset = doc.tree.new_element("meta");
    charset.set_attr("charset", "utf-8");
    head.prepend_child(&charset);
    let policy = doc.tree.new_element("meta");
    policy.set_attr("http-equiv", "Content-Security-Policy");
    policy.set_attr("content", COPY_POLICY);
    head.prepend_child(&policy);
    let mut out = String::from(doctype(mode));
    out.push_str(&doc.html());
    Ok(out)
}

/// The last-resort check of a finished copy.
fn verify(out: &str, files: &HashSet<String>, mode: QuirksMode) -> Result<(), String> {
    let (doc, whole) = parse(out);
    if !whole {
        return Err("the copy was too large to check".into());
    }
    if doc.quirks_mode.get() != mode {
        return Err("the copy would be shown in another mode than the page".into());
    }
    let elements: Vec<NodeRef<'_>> = doc
        .root()
        .children()
        .into_iter()
        .filter(NodeRef::is_element)
        .collect();
    if elements.len() != 1 || !is_html(&elements[0], "html") {
        return Err("the copy's root is not an html element".into());
    }
    let html = &elements[0];
    let head = html
        .first_child()
        .filter(|h| is_html(h, "head"))
        .ok_or("the copy's head is not the first thing in it")?;
    let first = head.first_child().ok_or("the copy's head is empty")?;
    let policy_first = first.query(|n| match &n.data {
        NodeData::Element(e) => {
            e.name.ns == ns!(html)
                && e.name.local.as_ref() == "meta"
                && e.attrs.len() == 2
                && e.attrs.iter().any(|a| {
                    a.name.local.as_ref() == "http-equiv"
                        && a.value.as_ref() == "Content-Security-Policy"
                })
                && e.attrs
                    .iter()
                    .any(|a| a.name.local.as_ref() == "content" && a.value.as_ref() == COPY_POLICY)
        }
        _ => false,
    });
    if policy_first != Some(true) {
        return Err("the copy's policy is not the first thing in its head".into());
    }
    let again = finish(&doc, files, mode)?;
    if again != out {
        return Err("cleaning the copy a second time changed it".into());
    }
    Ok(())
}

fn is_html(node: &NodeRef<'_>, local: &str) -> bool {
    node.query(|n| match &n.data {
        NodeData::Element(e) => e.name.ns == ns!(html) && e.name.local.as_ref() == local,
        _ => false,
    })
    .unwrap_or(false)
}

/// The `<html>`, `<head>` and `<body>` of a parsed document, made when missing (a page
/// whose body was a frameset has none once that is removed). The head becomes the first
/// child of the root element.
fn structure(doc: &Document) -> Result<(NodeRef<'_>, NodeRef<'_>, NodeRef<'_>), String> {
    let html = doc
        .root()
        .children()
        .into_iter()
        .find(|n| is_html(n, "html"))
        .ok_or("the page has no html element")?;
    let head = match html.children().into_iter().find(|n| is_html(n, "head")) {
        Some(h) => h,
        None => doc.tree.new_element("head"),
    };
    html.prepend_child(&head);
    let body = match html.children().into_iter().find(|n| is_html(n, "body")) {
        Some(b) => b,
        None => {
            let b = doc.tree.new_element("body");
            html.append_child(&b);
            b
        }
    };
    Ok((html, head, body))
}

/// What happens to an element.
enum Rule {
    Keep,
    Drop,
    /// An `<iframe>`: kept as an empty box of its size.
    Frame,
    /// A made-up element name: the element goes, what it holds stays.
    Unwrap,
}

/// Cleans every node below the document, template contents included, without using the
/// call stack (a page may nest as deep as it likes).
fn clean(doc: &Document, files: &HashSet<String>) {
    let tree = &doc.tree;
    let mut stack: Vec<NodeId> = vec![doc.root().id];
    while let Some(id) = stack.pop() {
        let node = NodeRef::new(id, tree);
        enum Kind {
            Container,
            Element(QualName, Option<NodeId>),
            Comment(bool),
            Other,
            Text,
        }
        let kind = node.query(|n| match &n.data {
            NodeData::Document | NodeData::Fragment => Kind::Container,
            NodeData::Element(e) => Kind::Element(e.name.clone(), e.template_contents),
            NodeData::Comment { contents } => Kind::Comment(contents.as_ref() == COMMENT),
            NodeData::Text { .. } => Kind::Text,
            _ => Kind::Other,
        });
        match kind {
            None | Some(Kind::Text) => {}
            Some(Kind::Container) => stack.extend(node.children().iter().map(|c| c.id)),
            Some(Kind::Comment(ours)) => {
                if !ours {
                    node.remove_from_parent();
                }
            }
            Some(Kind::Other) => node.remove_from_parent(),
            Some(Kind::Element(name, template)) => {
                match rule(&node, &name, files) {
                    Rule::Drop => {
                        node.remove_from_parent();
                        continue;
                    }
                    Rule::Frame => {
                        frame_box(&node, files);
                        continue;
                    }
                    Rule::Unwrap => {
                        let children: Vec<NodeId> = node.children().iter().map(|c| c.id).collect();
                        if let Some(first) = node.first_child() {
                            // The children (the first and its siblings) move up in its place.
                            node.insert_siblings_before(&first);
                        }
                        node.remove_from_parent();
                        stack.extend(children);
                        continue;
                    }
                    Rule::Keep => {}
                }
                let local = name.local.to_ascii_lowercase();
                if !(local == "meta" && is_our_policy(&node)) {
                    clean_attributes(&node, &name, files);
                }
                if local == "style" {
                    let text = node.text().to_string();
                    match css::confine(&text, |t| local_file(t, "index.html", files)) {
                        Some(clean)
                            if !(name.ns == ns!(html)
                                && clean.to_ascii_lowercase().contains("</style")) =>
                        {
                            let only_text = node.children().iter().all(NodeRef::is_text);
                            if clean != text || !only_text {
                                node.set_text(clean);
                            }
                        }
                        _ => {
                            node.remove_from_parent();
                            continue;
                        }
                    }
                }
                stack.extend(node.children().iter().map(|c| c.id));
                if let Some(t) = template {
                    stack.push(t);
                }
            }
        }
    }
}

fn rule(node: &NodeRef<'_>, name: &QualName, files: &HashSet<String>) -> Rule {
    let local = name.local.to_ascii_lowercase();
    if !plain_name(&local) {
        return Rule::Unwrap;
    }
    if DROPPED.contains(&local.as_str()) {
        return Rule::Drop;
    }
    match local.as_str() {
        "iframe" if name.ns == ns!(html) => Rule::Frame,
        "iframe" => Rule::Drop,
        "meta" => {
            if name.ns == ns!(html) && meta_kept(node) {
                Rule::Keep
            } else {
                Rule::Drop
            }
        }
        "link" => {
            if name.ns == ns!(html) && link_kept(node, files) {
                Rule::Keep
            } else {
                Rule::Drop
            }
        }
        l if name.ns == ns!(svg) && SVG_ANIMATIONS.contains(&l) => {
            let target = attr(node, "attributeName")
                .or_else(|| attr(node, "attributename"))
                .unwrap_or_default();
            let target = target.trim().to_ascii_lowercase();
            let target = target.rsplit(':').next().unwrap_or("");
            if target == "href" || target.starts_with("on") {
                Rule::Drop
            } else {
                Rule::Keep
            }
        }
        _ => Rule::Keep,
    }
}

/// The value of an attribute (no namespace), by its exact local name.
fn attr(node: &NodeRef<'_>, local: &str) -> Option<String> {
    node.query(|n| match &n.data {
        NodeData::Element(e) => e
            .attrs
            .iter()
            .find(|a| a.name.ns == ns!() && a.name.local.as_ref() == local)
            .map(|a| a.value.to_string()),
        _ => None,
    })
    .flatten()
}

fn attrs_of(node: &NodeRef<'_>) -> Vec<(String, String)> {
    node.query(|n| match &n.data {
        NodeData::Element(e) => e
            .attrs
            .iter()
            .map(|a| (a.name.local.to_ascii_lowercase(), a.value.to_string()))
            .collect(),
        _ => Vec::new(),
    })
    .unwrap_or_default()
}

/// The copy's own policy, exactly as [`finish`] writes it.
fn is_our_policy(node: &NodeRef<'_>) -> bool {
    let attrs = attrs_of(node);
    attrs.len() == 2
        && attrs
            .iter()
            .any(|(n, v)| n == "http-equiv" && v.eq_ignore_ascii_case("content-security-policy"))
        && attrs.iter().any(|(n, v)| n == "content" && v == COPY_POLICY)
}

/// `<meta>` kept: the copy's policy, `charset="utf-8"`, and a few that describe the page.
fn meta_kept(node: &NodeRef<'_>) -> bool {
    let attrs = attrs_of(node);
    if is_our_policy(node) {
        return true;
    }
    if attrs.iter().any(|(n, _)| n == "http-equiv") {
        return false;
    }
    if let Some((_, v)) = attrs.iter().find(|(n, _)| n == "charset") {
        return attrs.len() == 1 && v.eq_ignore_ascii_case("utf-8");
    }
    attrs.iter().any(|(n, v)| {
        n == "name"
            && matches!(
                v.to_ascii_lowercase().as_str(),
                "viewport" | "theme-color" | "color-scheme" | "description"
            )
    })
}

/// `<link>` kept: a style sheet or an icon that is one of the copy's own files.
fn link_kept(node: &NodeRef<'_>, files: &HashSet<String>) -> bool {
    let rel = attr(node, "rel").unwrap_or_default().to_ascii_lowercase();
    let tokens: Vec<&str> = rel.split_ascii_whitespace().collect();
    let allowed = |t: &&str| {
        matches!(
            *t,
            "stylesheet"
                | "icon"
                | "shortcut"
                | "apple-touch-icon"
                | "apple-touch-icon-precomposed"
                | "mask-icon"
        )
    };
    !tokens.is_empty()
        && tokens.iter().all(allowed)
        && tokens.iter().any(|t| *t != "shortcut")
        && attr(node, "href").is_some_and(|h| local_file(&h, "index.html", files))
}

/// An `<iframe>` becomes an empty `<div>` of the frame's size, with nothing in it.
fn frame_box(node: &NodeRef<'_>, files: &HashSet<String>) {
    let size = |v: Option<String>, default: &str| -> String {
        let v = v.unwrap_or_default();
        let v = v.trim();
        if !v.is_empty() && v.len() <= 8 && v.bytes().all(|b| b.is_ascii_digit()) {
            format!("{v}px")
        } else if v.len() > 1
            && v.len() <= 8
            && v.ends_with('%')
            && v[..v.len() - 1].bytes().all(|b| b.is_ascii_digit())
        {
            v.to_string()
        } else {
            default.to_string()
        }
    };
    let width = size(attr(node, "width"), "300px");
    let height = size(attr(node, "height"), "150px");
    let mut style = format!("display:inline-block;width:{width};height:{height};");
    if let Some(own) = attr(node, "style")
        && let Some(clean) = css::confine(&own, |t| local_file(t, "index.html", files))
    {
        style.push_str(&clean);
    }
    let keep: Vec<(String, String)> = ["id", "class", "title"]
        .iter()
        .filter_map(|k| attr(node, k).map(|v| ((*k).to_string(), v)))
        .filter(|(_, v)| !leaves_folder(v))
        .collect();
    node.remove_children();
    node.update(|n| {
        if let NodeData::Element(e) = &mut n.data {
            e.name = QualName::new(None, ns!(html), LocalName::from("div"));
            e.attrs.clear();
            e.template_contents = None;
            for (k, v) in &keep {
                e.attrs.push(plain_attr(k, v));
            }
            e.attrs.push(plain_attr("style", &style));
            e.attrs.push(plain_attr("data-captured-frame", ""));
        }
    });
}

fn plain_attr(name: &str, value: &str) -> Attribute {
    Attribute {
        name: QualName::new(None, ns!(), LocalName::from(name)),
        value: StrTendril::from(value),
    }
}

/// A name of letters, digits and `-_.:`, starting with a letter or `_`, of sensible length.
fn plain_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && name.len() <= 200
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
}

fn clean_attributes(node: &NodeRef<'_>, el: &QualName, files: &HashSet<String>) {
    let el_local = el.local.to_ascii_lowercase();
    let is_link = el.ns == ns!(html) && el_local == "link";
    let is_meta = el.ns == ns!(html) && el_local == "meta";
    node.update(|n| {
        let NodeData::Element(e) = &mut n.data else {
            return;
        };
        let old = std::mem::take(&mut e.attrs);
        let mut seen: HashSet<(String, String)> = HashSet::new();
        for mut a in old {
            let local = a.name.local.to_ascii_lowercase();
            if is_link && !matches!(local.as_str(), "rel" | "href" | "media" | "type" | "sizes" | "color") {
                continue;
            }
            if is_meta && !matches!(local.as_str(), "name" | "content" | "media" | "charset") {
                continue;
            }
            let Some(value) = attribute_value(&a.name, &local, a.value.as_ref(), el, &el_local, files)
            else {
                continue;
            };
            let key = (a.name.ns.to_string(), local);
            if !seen.insert(key) {
                continue;
            }
            a.value = StrTendril::from(value.as_str());
            e.attrs.push(a);
        }
    });
}

/// The value an attribute keeps, or `None` when it goes.
fn attribute_value(
    name: &QualName,
    local: &str,
    value: &str,
    el: &QualName,
    el_local: &str,
    files: &HashSet<String>,
) -> Option<String> {
    let in_no_ns = name.ns == ns!();
    let is_xlink = name.ns == ns!(xlink);
    if !(in_no_ns || is_xlink || name.ns == ns!(xml)) {
        return None;
    }
    if name.ns == ns!(xml) && local == "base" {
        return None;
    }
    if !plain_name(local) || local.starts_with("on") || DROPPED_ATTRS.contains(&local) {
        return None;
    }
    let local_ok = |v: &str| local_file(v, "index.html", files);
    let picture_ok = |v: &str| local_ok(v) || is_data_image(v);
    let svg = el.ns == ns!(svg);
    match local {
        "style" if in_no_ns => return css::confine(value, local_ok),
        "href" if in_no_ns || is_xlink => {
            let t = css::url_text(value);
            return match el_local {
                "a" | "area" if el.ns == ns!(html) || svg => Some(link_value(&t)),
                "use" if svg => t.starts_with('#').then_some(t),
                "image" | "feimage" if svg => {
                    (t.starts_with('#') || picture_ok(&t)).then_some(t)
                }
                _ => (t.starts_with('#') || local_ok(&t)).then_some(t),
            };
        }
        "src" if in_no_ns => {
            let t = css::url_text(value);
            let picture = matches!(el_local, "img" | "input" | "source" | "image");
            return (local_ok(&t) || (picture && is_data_image(&t))).then_some(t);
        }
        "srcset" if in_no_ns => {
            let kept = srcset(value, &picture_ok);
            return (!kept.is_empty()).then_some(kept);
        }
        "poster" | "background" if in_no_ns => {
            let t = css::url_text(value);
            return picture_ok(&t).then_some(t);
        }
        "usemap" if in_no_ns => return value.trim().starts_with('#').then(|| value.to_string()),
        "shadowrootmode" if in_no_ns => {
            return (el.ns == ns!(html)
                && el_local == "template"
                && matches!(value, "open" | "closed"))
            .then(|| value.to_string());
        }
        _ => {}
    }
    // Anything else: CSS in it is confined, and a value that reaches outside goes.
    let value = if value.contains('(') {
        css::confine(value, local_ok)?
    } else {
        value.to_string()
    };
    (!leaves_folder(&value)).then_some(value)
}

/// A link's address: a fragment, or an absolute `http(s)`, `mailto:` or `tel:` address.
/// Anything else becomes `#`, which keeps the element a link without going anywhere.
fn link_value(t: &str) -> String {
    if t.starts_with('#') {
        return t.to_string();
    }
    match Url::parse(t) {
        Ok(u) if matches!(u.scheme(), "http" | "https" | "mailto" | "tel") => t.to_string(),
        _ => "#".to_string(),
    }
}

/// A `data:image/…` address.
fn is_data_image(t: &str) -> bool {
    css::url_text(t).to_ascii_lowercase().starts_with("data:image/")
}

/// Whether a value used as an address would reach outside the copy's folder or run code.
pub(crate) fn leaves_folder(v: &str) -> bool {
    let t = css::url_text(v).to_ascii_lowercase();
    let b = t.as_bytes();
    (b.len() >= 2 && matches!(b[0], b'/' | b'\\') && matches!(b[1], b'/' | b'\\'))
        || OUTSIDE.iter().any(|p| t.starts_with(p))
}

/// Whether `raw`, written in the copy file `from` (a path inside the capture folder),
/// names one of the capture's files in `assets/` or `styles/` by a relative address that
/// stays inside the folder.
pub(crate) fn local_file(raw: &str, from: &str, files: &HashSet<String>) -> bool {
    let t = css::url_text(raw);
    if t.is_empty() || Url::parse(&t).is_ok() {
        // Empty, or absolute (a scheme of its own): never one of the copy's files.
        return false;
    }
    let Ok(base) = Url::parse(ROOT).and_then(|r| r.join(from)) else {
        return false;
    };
    let Ok(u) = base.join(&t) else {
        return false;
    };
    if u.scheme() != "file" || u.host_str().is_some_and(|h| !h.is_empty()) {
        return false;
    }
    u.path()
        .strip_prefix("/hcap-copy/")
        .is_some_and(|rel| LINKABLE.iter().any(|p| rel.starts_with(p)) && files.contains(rel))
}

/// A srcset value with only the candidates `keep` accepts, and plain descriptors.
fn srcset(value: &str, keep: &dyn Fn(&str) -> bool) -> String {
    let mut out: Vec<String> = Vec::new();
    let chars: Vec<char> = value.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        while i < chars.len() && (chars[i].is_ascii_whitespace() || chars[i] == ',') {
            i += 1;
        }
        let start = i;
        while i < chars.len() && !chars[i].is_ascii_whitespace() {
            i += 1;
        }
        if start == i {
            break;
        }
        let mut url: String = chars[start..i].iter().collect();
        let mut descriptor = String::new();
        if url.ends_with(',') {
            url = url.trim_end_matches(',').to_string();
        } else {
            let d_start = i;
            while i < chars.len() && chars[i] != ',' {
                i += 1;
            }
            descriptor = chars[d_start..i].iter().collect::<String>().trim().to_string();
        }
        if url.is_empty() || !keep(&url) {
            continue;
        }
        let plain = descriptor.len() <= 12
            && descriptor.len() > 1
            && descriptor.ends_with(['x', 'w'])
            && descriptor[..descriptor.len() - 1]
                .chars()
                .all(|c| c.is_ascii_digit() || c == '.');
        out.push(if plain {
            format!("{url} {descriptor}")
        } else {
            url
        });
        if out.len() >= 30 {
            break;
        }
    }
    out.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files() -> HashSet<String> {
        [
            "assets/hero.png",
            "assets/dot.svg",
            "assets/studio.woff2",
            "styles/site.css",
            "scripts/app.js.txt",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn inert(html: &str) -> String {
        make_inert(html, &[], &files()).expect("a copy").html
    }

    /// No element or attribute in the copy can load, run or send anything.
    fn assert_quiet(out: &str) {
        let lower = out.to_ascii_lowercase();
        for bad in [
            "<script",
            "<embed",
            "<object",
            "<iframe",
            "<frame",
            "<base",
            "<noscript",
            "<foreignobject",
            "javascript:",
            "file:",
            "//attacker",
            "93.184.215.14",
            "onerror",
            "onload",
            "refresh",
        ] {
            assert!(!lower.contains(bad), "{bad} is in the copy:\n{out}");
        }
    }

    #[test]
    fn a_replaced_root_and_look_alike_elements_become_a_proper_inert_page() {
        // What the page script wrote for a page that swapped its root for a <div> holding
        // elements made in another namespace (they serialise as the real HTML elements).
        let svg = "PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIG9ubG9hZD0iZmV0Y2goJ2h0dHA6Ly85My4xODQuMjE1LjE0OjgwODAvQkVBQ09OJykiPjwvc3ZnPg==";
        let html = format!(
            "<!DOCTYPE html>\n<div><head><meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\" content=\"script-src 'none'\"></head><h1>Copy of the page</h1><embed src=\"data:image/svg+xml;base64,{svg}\" type=\"image/svg+xml\"></embed><iframe src=\"data:image/svg+xml;base64,{svg}\" width=\"100\" height=\"100\"></iframe><img src=\"http://93.184.215.14:8080/BEACON-A-ns-img.png\"></img><link rel=\"stylesheet\" href=\"http://93.184.215.14:8080/BEACON-A-svg-link.css\"></link><body></body></div>"
        );
        let out = inert(&html);
        assert!(
            out.starts_with(&format!(
                "<!DOCTYPE html>\n<html><head><meta http-equiv=\"Content-Security-Policy\" content=\"{}\"><meta charset=\"utf-8\"><!--{COMMENT}--></head><body>",
                COPY_POLICY.replace('\'', "'")
            )),
            "{out}"
        );
        assert!(out.contains("<h1>Copy of the page</h1>"), "{out}");
        assert!(
            out.contains("<div style=\"display:inline-block;width:100px;height:100px;\" data-captured-frame=\"\"></div>"),
            "the frame keeps only its box: {out}"
        );
        assert!(out.contains("<img>"), "the picture loses its address: {out}");
        assert_quiet(&out);
        assert!(!out.contains("svg+xml"), "{out}");
    }

    #[test]
    fn an_attribute_named_like_a_tag_cannot_move_the_shapes_before_the_policy() {
        let html = "<!DOCTYPE html>\n<html <body=\"1\" lang=\"en\"><head><title>Static page</title></head><body><h1>A static page</h1><svg width=\"40\" height=\"40\"><use href=\"#dot\"></use></svg></body></html>";
        let sprite = "<svg xmlns=\"http://www.w3.org/2000/svg\"><symbol id=\"dot\" viewBox=\"0 0 10 10\"><circle cx=\"5\" cy=\"5\" r=\"4\" fill=\"#33f\"></circle></symbol><1 a=\"<img x='>' src=x onerror=fetch('http://93.184.215.14:8080/BEACON-B')>\"/></svg>";
        let out = make_inert(html, &[sprite.to_string()], &files())
            .unwrap()
            .html;
        let policy = out.find("Content-Security-Policy").unwrap();
        let shapes = out.find("<symbol id=\"dot\"").expect("the shapes are copied in");
        assert!(policy < shapes, "{out}");
        assert!(
            out.contains("<html lang=\"en\"><head>"),
            "the made-up attribute is gone: {out}"
        );
        assert!(
            out.contains("<body><div hidden=\"\" aria-hidden=\"true\" style=\"position:absolute;width:0;height:0;overflow:hidden\"><svg"),
            "{out}"
        );
        assert!(out.contains("<use href=\"#dot\"></use>"), "{out}");
        assert_quiet(&out);
    }

    #[test]
    fn addresses_keep_only_the_copy_s_own_files() {
        let html = r##"<!DOCTYPE html><html><head><link rel="stylesheet" href="styles/site.css"><link rel="stylesheet" href="https://cdn.example/x.css"><link rel="preload" href="assets/hero.png"><link rel="icon" href="assets/dot.svg"><meta http-equiv="refresh" content="0;url=javascript:alert(1)"><meta name="viewport" content="width=device-width"><base href="https://evil.example/"></head>
<body background="file://attacker.example/share/bg.png">
<img src="assets/hero.png" srcset="assets/hero.png 1x, https://cdn.example/h2.png 2x, assets/dot.svg 3x" alt="Hero">
<img src="file://attacker.example/share/pic.png"><img src="//attacker.example/x.png"><img src="/assets/hero.png"><img src="assets/../../hero.png"><img src="data:image/png;base64,AAAA">
<div style="width:10px;background:url('file://attacker.example/share/bg.png')"></div>
<div style="background:u\72l(//attacker.example/y.png)"></div>
<video poster="file://attacker.example/share/poster.png" src="assets/hero.png"><source src="https://cdn.example/v.mp4"><track src="https://cdn.example/t.vtt"></video>
<a href="javascript:alert(1)">x</a><a href="https://example.com/about" ping="https://track.example/">about</a><a href="#top">top</a><a href="file:///etc/passwd">f</a>
<form action="https://evil.example/post"><button formaction="https://evil.example/b">go</button><input type="image" src="assets/hero.png"></form>
<p title="file://attacker.example/x" data-x="//attacker.example/y" class="md:flex hover:bg-blue" aria-label="Photo: sunset">text</p>
<svg><a href="javascript:alert(1)"><text>t</text></a><image href="https://cdn.example/i.png"></image><use href="assets/dot.svg#dot"></use><rect fill="url(#g)" filter="url(//attacker.example/f.svg#f)"></rect><animate attributeName="href" values="javascript:alert(1)"></animate><set attributeName="xlink:href" to="javascript:alert(1)"></set><animate attributeName="opacity" values="0;1"></animate></svg>
<math><mi href="https://evil.example/">x</mi></math>
<img attributionsrc="https://evil.example/r" src="assets/hero.png" crossorigin="anonymous" integrity="sha256-x">
</body></html>"##;
        let out = inert(html);
        assert_quiet(&out);
        assert!(!out.contains("evil.example") && !out.contains("cdn.example") && !out.contains("track.example"), "{out}");
        assert!(out.contains("<link rel=\"stylesheet\" href=\"styles/site.css\">"), "{out}");
        assert!(out.contains("<link rel=\"icon\" href=\"assets/dot.svg\">"), "{out}");
        assert!(out.contains("<meta name=\"viewport\" content=\"width=device-width\">"), "{out}");
        assert!(
            out.contains("<img src=\"assets/hero.png\" srcset=\"assets/hero.png 1x, assets/dot.svg 3x\" alt=\"Hero\">"),
            "{out}"
        );
        assert_eq!(out.matches("<img>").count(), 4, "four pictures lose their address: {out}");
        assert!(out.contains("<img src=\"data:image/png;base64,AAAA\">"), "{out}");
        assert!(out.contains("width:10px;background:url(&quot;about:invalid&quot;)"), "{out}");
        assert!(!out.contains("u\\72l"), "{out}");
        assert!(out.contains("<video src=\"assets/hero.png\"><source><track></video>"), "{out}");
        assert!(out.contains("<a href=\"#\">x</a>"), "{out}");
        assert!(out.contains("<a href=\"https://example.com/about\">about</a>"), "{out}");
        assert!(out.contains("<a href=\"#top\">top</a>"), "{out}");
        assert!(out.contains("<a href=\"#\">f</a>"), "{out}");
        assert!(out.contains("<form><button>go</button><input type=\"image\" src=\"assets/hero.png\"></form>"), "{out}");
        assert!(
            out.contains("<p class=\"md:flex hover:bg-blue\" aria-label=\"Photo: sunset\">text</p>"),
            "{out}"
        );
        assert!(out.contains("<a href=\"#\"><text>t</text></a>"), "{out}");
        assert!(out.contains("<image></image><use></use>"), "{out}");
        assert!(out.contains("<rect fill=\"url(#g)\" filter=\"url(&quot;about:invalid&quot;)\"></rect>"), "{out}");
        assert!(out.contains("<animate attributeName=\"opacity\" values=\"0;1\"></animate></svg>"), "{out}");
        assert!(out.contains("<math><mi>x</mi></math>"), "{out}");
        assert!(out.contains("<img src=\"assets/hero.png\">\n</body>"), "{out}");
    }

    #[test]
    fn hidden_markup_in_raw_text_and_templates_is_cleaned_too() {
        let html = r#"<html><head><noscript><p title="</noscript><img src=x onerror=alert(1)>"></p></noscript></head><body>
<template shadowrootmode="open" shadowrootclonable=""><style>:host{background:url(file://attacker.example/a.png)}</style><img src="https://cdn.example/x.png" onload="alert(1)"><slot></slot></template>
<template><script>alert(1)</script><p onclick="x">p</p></template>
<svg><style>.a{fill:url(#g)} .b{background:url(//attacker.example/b.png)}</style></svg>
<style>@import url(https://cdn.example/x.css); a{color:red}</style>
<!-- a comment --><textarea>&lt;script&gt;</textarea><title>&lt;/title&gt;</title>
<my-widget is="x-y"><x:embed src="assets/hero.png">kept text</x:embed></my-widget>
</body></html>"#;
        let out = inert(html);
        assert_quiet(&out);
        assert!(!out.contains("cdn.example"), "{out}");
        assert!(out.contains("<template shadowrootmode=\"open\"><style>:host{background:url(\"about:invalid\")}</style><img><slot></slot></template>"), "{out}");
        assert!(out.contains("<template><p>p</p></template>"), "{out}");
        assert!(out.contains(".a{fill:url(#g)} .b{background:url(\"about:invalid\")}"), "{out}");
        assert!(out.contains("<style>@import url(\"about:invalid\"); a{color:red}</style>"), "{out}");
        assert!(!out.contains("a comment"), "{out}");
        assert!(out.contains("<textarea>&lt;script&gt;</textarea>"), "{out}");
        assert!(out.contains("kept text"), "{out}");
    }

    #[test]
    fn quirks_mode_and_odd_structure_survive_the_round_trip() {
        // No doctype: quirks mode, kept so; a frameset page gets a body of its own.
        let out = inert("<html><body><p>old page</p></body></html>");
        assert!(out.starts_with("<html><head><meta http-equiv="), "{out}");
        let out = inert("<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Transitional//EN\" \"http://www.w3.org/TR/xhtml1/DTD/xhtml1-transitional.dtd\"><html><body><p>x</p></body></html>");
        assert!(out.starts_with("<!DOCTYPE html PUBLIC"), "{out}");
        let out = inert("<!DOCTYPE html><html><frameset><frame src=\"https://evil.example/\"></frameset></html>");
        assert!(out.ends_with("</head><body></body></html>"), "{out}");
        let out = inert("<!DOCTYPE html><title>t</title>text before<meta http-equiv=\"Content-Security-Policy\" content=\"default-src *\"><p>x");
        assert_quiet(&out);
        assert_eq!(out.matches("Content-Security-Policy").count(), 1, "{out}");
    }

    #[test]
    fn the_last_check_refuses_a_copy_that_is_not_settled() {
        let ok = inert("<!DOCTYPE html><p>fine</p>");
        assert!(verify(&ok, &files(), QuirksMode::NoQuirks).is_ok());
        // The policy not first in the head, or a copy that would change when cleaned again.
        let moved = ok.replacen("<head><meta http-equiv", "<head><title>x</title><meta http-equiv", 1);
        assert!(verify(&moved, &files(), QuirksMode::NoQuirks).is_err());
        let dirty = ok.replace("<p>fine</p>", "<p onclick=\"x\">fine</p>");
        assert!(verify(&dirty, &files(), QuirksMode::NoQuirks).is_err());
        assert!(verify(&ok, &files(), QuirksMode::Quirks).is_err());
        let root = ok.replace("<!DOCTYPE html>\n<html>", "<!DOCTYPE html>\n<div>");
        assert!(verify(&root, &files(), QuirksMode::NoQuirks).is_err());
    }

    #[test]
    fn local_files_are_named_relatively_inside_the_folder_only() {
        let f = files();
        assert!(local_file("assets/hero.png", "index.html", &f));
        assert!(local_file("./assets/hero.png?v=1#x", "index.html", &f));
        assert!(local_file("../assets/hero.png", "styles/site.css", &f));
        assert!(local_file("site.css", "styles/other.css", &f));
        for no in [
            "",
            "/assets/hero.png",
            "//attacker.example/assets/hero.png",
            "\\\\attacker.example\\share\\hero.png",
            "file:///hcap-copy/assets/hero.png",
            "C:/assets/hero.png",
            "assets/../../assets/hero.png",
            "scripts/app.js.txt",
            "assets/missing.png",
            "Assets/hero.png",
        ] {
            assert!(!local_file(no, "index.html", &f), "{no}");
        }
        assert!(leaves_folder(" \\\\host\\share"));
        assert!(leaves_folder("JaVa\tScRiPt:x"));
        assert!(!leaves_folder("md:flex"));
    }
}
