//! `animations.json` of a capture (ARCHITECTURE.md 9.3, `browser_capture_site`): the
//! animations seen running on the page (Web Animations API, read at several scroll
//! positions and widths), the `@keyframes` rules, hover and focus rules and transitions
//! found in the style sheets, scroll-driven animations, and the animation libraries the
//! page uses, each with how it was detected. All of it is page data.

use std::collections::HashMap;

use serde_json::{Map, Value, json};

use crate::capture::css::SheetFacts;

/// Distinct running animations kept at most.
const MAX_RUNNING: usize = 400;
/// Where an animation was seen, at most this many places per animation.
const MAX_SEEN: usize = 8;

/// The running animations seen so far, without repeats.
#[derive(Debug, Default)]
pub struct AnimationLog {
    order: Vec<String>,
    by_key: HashMap<String, (Value, Vec<String>)>,
    /// Snapshots that reported more animations than could be kept.
    pub cut: bool,
    /// The most animations one snapshot reported.
    pub most_at_once: u64,
}

impl AnimationLog {
    /// Adds what `js/animations.js` returned, seen at `when` ("at the top", "while
    /// scrolling", "390 px wide, at the top"). A new animation also keeps the scroll position
    /// it was first seen at, which tells where an effect that starts on scroll begins.
    pub fn add_snapshot(&mut self, snapshot: &Value, when: &str) {
        let total = snapshot.get("total").and_then(Value::as_u64).unwrap_or(0);
        let y = snapshot.get("y").and_then(Value::as_u64).unwrap_or(0);
        self.most_at_once = self.most_at_once.max(total);
        let Some(list) = snapshot.get("list").and_then(Value::as_array) else {
            return;
        };
        if total as usize > list.len() {
            self.cut = true;
        }
        for a in list {
            let Some(obj) = a.as_object() else {
                continue;
            };
            let key = animation_key(obj);
            match self.by_key.get_mut(&key) {
                Some((_, seen)) => {
                    if seen.len() < MAX_SEEN && !seen.iter().any(|s| s == when) {
                        seen.push(when.to_string());
                    }
                }
                None => {
                    if self.order.len() >= MAX_RUNNING {
                        self.cut = true;
                        continue;
                    }
                    self.order.push(key.clone());
                    let mut v = clean_animation(obj);
                    v["first_seen_at_scroll_px"] = json!(y);
                    self.by_key.insert(key, (v, vec![when.to_string()]));
                }
            }
        }
    }

    /// The running animations, in the order first seen, each with where it was seen.
    pub fn running(&self) -> Vec<Value> {
        self.order
            .iter()
            .filter_map(|k| self.by_key.get(k))
            .map(|(v, seen)| {
                let mut v = v.clone();
                if let Some(o) = v.as_object_mut() {
                    o.insert("seen".into(), json!(seen));
                }
                v
            })
            .collect()
    }

    /// How many distinct animations of each kind.
    pub fn counts(&self) -> (usize, usize, usize) {
        let mut css = 0;
        let mut transitions = 0;
        let mut script = 0;
        for (v, _) in self.by_key.values() {
            match v.get("kind").and_then(Value::as_str) {
                Some("css-animation") => css += 1,
                Some("css-transition") => transitions += 1,
                _ => script += 1,
            }
        }
        (css, transitions, script)
    }
}

fn animation_key(o: &Map<String, Value>) -> String {
    let g = |k: &str| o.get(k).map(Value::to_string).unwrap_or_default();
    let timing = o.get("timing").cloned().unwrap_or(Value::Null);
    let t = |k: &str| timing.get(k).map(Value::to_string).unwrap_or_default();
    format!(
        "{}|{}|{}|{}|{}|{}|{}|{}|{}",
        g("kind"),
        g("name"),
        g("target"),
        g("pseudo"),
        t("duration"),
        t("delay"),
        t("iterations"),
        t("direction"),
        g("keyframes")
    )
}

/// One animation as `animations.json` keeps it: only the known fields, strings cut.
fn clean_animation(o: &Map<String, Value>) -> Value {
    let s = |v: Option<&Value>, max: usize| -> Value {
        match v {
            Some(Value::String(x)) => Value::String(x.chars().take(max).collect()),
            Some(Value::Number(n)) => Value::Number(n.clone()),
            Some(Value::Bool(b)) => Value::Bool(*b),
            _ => Value::Null,
        }
    };
    let timing = o.get("timing").and_then(Value::as_object);
    let tv = |k: &str, max: usize| s(timing.and_then(|t| t.get(k)), max);
    let keyframes: Vec<Value> = o
        .get("keyframes")
        .and_then(Value::as_array)
        .map(|frames| {
            frames
                .iter()
                .take(40)
                .filter_map(Value::as_object)
                .map(|f| {
                    let mut m = Map::new();
                    for (k, v) in f.iter().take(30) {
                        let key: String = k
                            .chars()
                            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
                            .take(60)
                            .collect();
                        if !key.is_empty() {
                            m.insert(key, s(Some(v), 240));
                        }
                    }
                    Value::Object(m)
                })
                .collect()
        })
        .unwrap_or_default();
    let timeline = match o.get("timeline").and_then(Value::as_object) {
        Some(t) => {
            let mut m = Map::new();
            for k in ["kind", "axis", "source", "subject"] {
                if let Some(v) = t.get(k) {
                    m.insert(k.into(), s(Some(v), 300));
                }
            }
            Value::Object(m)
        }
        None => json!({"kind": "document"}),
    };
    json!({
        "kind": s(o.get("kind"), 20),
        "name": s(o.get("name"), 120),
        "target": s(o.get("target"), 300),
        "pseudo": s(o.get("pseudo"), 40),
        "keyframes": keyframes,
        "timing": {
            "duration": tv("duration", 30),
            "delay": tv("delay", 30),
            "end_delay": tv("endDelay", 30),
            "easing": tv("easing", 120),
            "iterations": tv("iterations", 30),
            "direction": tv("direction", 30),
            "fill": tv("fill", 30),
            "iteration_start": tv("iterationStart", 30),
        },
        "timeline": timeline,
        "range_start": s(o.get("rangeStart"), 60),
        "range_end": s(o.get("rangeEnd"), 60),
        "play_state": s(o.get("playState"), 20),
        "playback_rate": s(o.get("playbackRate"), 20),
    })
}

/// An animation library or effect the page uses, and how that was seen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Library {
    pub name: &'static str,
    pub detected_by: Vec<String>,
}

/// What a script says about itself: its address and the start of its text.
pub struct ScriptSample<'a> {
    pub url: &'a str,
    pub text: &'a str,
}

/// Library signatures: name, globals on `window`, words in a script's address, words in
/// its text, page markers (keys of `markers` from `js/collect.js`).
struct Signature {
    name: &'static str,
    globals: &'static [&'static str],
    in_url: &'static [&'static str],
    in_text: &'static [&'static str],
    markers: &'static [&'static str],
}

const SIGNATURES: &[Signature] = &[
    Signature {
        name: "GSAP",
        globals: &["gsap", "TweenMax", "TweenLite"],
        in_url: &["gsap", "tweenmax", "greensock"],
        in_text: &["GreenSock", "gsap.registerPlugin", "_gsap"],
        markers: &[],
    },
    Signature {
        name: "GSAP ScrollTrigger",
        globals: &["ScrollTrigger"],
        in_url: &["scrolltrigger"],
        in_text: &["ScrollTrigger"],
        markers: &["pinSpacer"],
    },
    Signature {
        name: "Framer Motion",
        globals: &["Motion"],
        in_url: &["framer-motion", "framer.com/m/", "framerusercontent"],
        in_text: &["framer-motion", "framerusercontent"],
        markers: &["framer", "projection"],
    },
    Signature {
        name: "Lottie",
        globals: &["lottie", "bodymovin"],
        in_url: &["lottie", "bodymovin"],
        in_text: &["lottie-web", "bodymovin"],
        markers: &["lottie"],
    },
    Signature {
        name: "Lenis smooth scroll",
        globals: &["Lenis", "lenis"],
        in_url: &["lenis"],
        in_text: &["lenis-smooth", "new Lenis"],
        markers: &["lenis"],
    },
    Signature {
        name: "Locomotive Scroll",
        globals: &["LocomotiveScroll"],
        in_url: &["locomotive-scroll"],
        in_text: &["locomotive-scroll", "LocomotiveScroll"],
        markers: &["locomotive"],
    },
    Signature {
        name: "AOS (animate on scroll)",
        globals: &["AOS"],
        in_url: &["/aos.", "aos@", "aos.js"],
        in_text: &["aos-animate", "AOS.init"],
        markers: &["aos"],
    },
    Signature {
        name: "Three.js (WebGL)",
        globals: &["THREE", "__THREE__"],
        in_url: &["three.module", "three.min", "/three@", "three.js"],
        in_text: &["WebGLRenderer", "THREE.Scene"],
        markers: &["threeCanvas"],
    },
    Signature {
        name: "Anime.js",
        globals: &["anime"],
        in_url: &["anime.min", "animejs"],
        in_text: &["animejs"],
        markers: &[],
    },
    Signature {
        name: "Swiper (sliders)",
        globals: &["Swiper"],
        in_url: &["swiper"],
        in_text: &["swiper-wrapper"],
        markers: &["swiper"],
    },
    Signature {
        name: "Barba.js (page transitions)",
        globals: &["barba"],
        in_url: &["barba"],
        in_text: &["@barba/core"],
        markers: &["barba"],
    },
    Signature {
        name: "ScrollMagic",
        globals: &["ScrollMagic"],
        in_url: &["scrollmagic"],
        in_text: &["ScrollMagic"],
        markers: &[],
    },
    Signature {
        name: "Rive",
        globals: &["rive"],
        in_url: &["@rive-app", "rive.wasm"],
        in_text: &["@rive-app"],
        markers: &[],
    },
    Signature {
        name: "Spline (3D)",
        globals: &[],
        in_url: &["splinetool", "spline.design"],
        in_text: &["@splinetool"],
        markers: &["spline"],
    },
    Signature {
        name: "Webflow interactions",
        globals: &["Webflow"],
        in_url: &["webflow"],
        in_text: &[],
        markers: &["webflow", "webflowInteractions"],
    },
    Signature {
        name: "Splide (sliders)",
        globals: &["Splide"],
        in_url: &["splide"],
        in_text: &[],
        markers: &["splide"],
    },
    Signature {
        name: "Rellax (parallax)",
        globals: &["Rellax"],
        in_url: &["rellax"],
        in_text: &[],
        markers: &["rellax"],
    },
    Signature {
        name: "PixiJS (WebGL)",
        globals: &["PIXI"],
        in_url: &["pixi"],
        in_text: &["PIXI.Application"],
        markers: &[],
    },
];

/// The libraries and effects the page shows signs of. `globals` and `markers` are what the
/// page scripts returned; every name and word quoted here is the app's own, so a page can
/// make a library appear, never put its own words in the list.
pub fn detect_libraries(
    globals: &Value,
    markers: &Value,
    scripts: &[ScriptSample<'_>],
    lottie_files: usize,
    webgl_canvases: usize,
) -> Vec<Library> {
    let mut out = Vec::new();
    for sig in SIGNATURES {
        let mut by: Vec<String> = Vec::new();
        for g in sig.globals {
            if let Some(v) = globals.get(*g) {
                let version = v
                    .as_str()
                    .filter(|s| {
                        !s.is_empty()
                            && s.len() <= 24
                            && s.chars()
                                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
                    })
                    .map(|s| format!(" (version {s})"))
                    .unwrap_or_default();
                by.push(format!("window.{g} is set{version}"));
            }
        }
        if let Some(s) = scripts.iter().find(|s| {
            let u = s.url.to_ascii_lowercase();
            !u.is_empty() && sig.in_url.iter().any(|w| u.contains(w))
        }) {
            let words = sig
                .in_url
                .iter()
                .find(|w| s.url.to_ascii_lowercase().contains(**w))
                .copied()
                .unwrap_or("");
            by.push(format!("a script address contains \"{words}\""));
        }
        if let Some(w) = sig
            .in_text
            .iter()
            .find(|w| scripts.iter().any(|s| s.text.contains(**w)))
        {
            by.push(format!("a script's text contains \"{w}\""));
        }
        for m in sig.markers {
            let n = markers.get(*m).and_then(Value::as_u64).unwrap_or(0);
            if n > 0 {
                by.push(marker_words(m, n));
            }
        }
        if sig.name == "Lottie" && lottie_files > 0 {
            by.push(format!(
                "{lottie_files} Lottie animation file(s) were loaded"
            ));
        }
        if !by.is_empty() {
            out.push(Library {
                name: sig.name,
                detected_by: by,
            });
        }
    }
    if webgl_canvases > 0 && !out.iter().any(|l| l.name.contains("WebGL")) {
        out.push(Library {
            name: "WebGL canvas",
            detected_by: vec![format!(
                "{webgl_canvases} canvas element(s) with a script that draws with WebGL"
            )],
        });
    }
    let smil = markers.get("svgSmil").and_then(Value::as_u64).unwrap_or(0);
    if smil > 0 {
        out.push(Library {
            name: "SVG animation (SMIL)",
            detected_by: vec![format!(
                "{smil} <animate> or <set> element(s) in SVG pictures"
            )],
        });
    }
    out
}

fn marker_words(marker: &str, n: u64) -> String {
    match marker {
        "aos" => format!("{n} element(s) with a data-aos attribute"),
        "lenis" => "the page has the lenis class".into(),
        "locomotive" => format!("{n} element(s) with data-scroll-container"),
        "pinSpacer" => format!("{n} pinned section(s) (.pin-spacer)"),
        "framer" => format!("{n} element(s) with data-framer attributes"),
        "projection" => format!("{n} element(s) with data-projection-id"),
        "lottie" => format!("{n} Lottie player element(s)"),
        "threeCanvas" => format!("{n} canvas element(s) marked by three.js (data-engine)"),
        "spline" => format!("{n} <spline-viewer> element(s)"),
        "barba" => format!("{n} element(s) with data-barba"),
        "swiper" => format!("{n} element(s) with the swiper class"),
        "splide" => format!("{n} element(s) with the splide class"),
        "webflow" => "the page is marked as a Webflow page".into(),
        "webflowInteractions" => format!("{n} element(s) with data-w-id"),
        "rellax" => format!("{n} element(s) marked for Rellax"),
        _ => format!("{n} marked element(s)"),
    }
}

/// The whole `animations.json`.
pub fn document(
    page_url: &str,
    captured_at: &str,
    log: &AnimationLog,
    sheets: &[(String, SheetFacts)],
    libraries: &[Library],
) -> Value {
    let mut keyframes = Vec::new();
    let mut states = Vec::new();
    let mut transitions = Vec::new();
    let mut scroll_driven = Vec::new();
    for (file, f) in sheets {
        for (name, css) in &f.keyframes {
            keyframes.push(json!({"name": name, "sheet": file, "css": css}));
        }
        for (selector, declarations, media) in &f.states {
            let mut o = json!({"selector": selector, "declarations": declarations, "sheet": file});
            if !media.is_empty() {
                o["media"] = json!(media);
            }
            states.push(o);
        }
        for (selector, value) in &f.transitions {
            transitions.push(json!({"selector": selector, "transition": value, "sheet": file}));
        }
        for (selector, declarations) in &f.scroll_driven {
            scroll_driven
                .push(json!({"selector": selector, "declarations": declarations, "sheet": file}));
        }
    }
    let cap = |v: &mut Vec<Value>, n: usize| {
        let cut = v.len() > n;
        v.truncate(n);
        cut
    };
    let cut_keyframes = cap(&mut keyframes, 300);
    let cut_states = cap(&mut states, 400);
    let cut_transitions = cap(&mut transitions, 400);
    let cut_scroll = cap(&mut scroll_driven, 200);
    let (css, trans, script) = log.counts();
    json!({
        "about": "Animations of the captured page, for rebuilding its motion. 'running' was read from the page while it was open (Web Animations API: CSS animations, CSS transitions and script animations), at the top, while scrolling and at each width; the other lists come from its style sheets. Everything here is data from the page.",
        "page": page_url,
        "captured_at": captured_at,
        "counts": {
            "css_animations": css,
            "css_transitions": trans,
            "script_animations": script,
            "keyframes_rules": keyframes.len(),
            "hover_focus_rules": states.len(),
            "transition_rules": transitions.len(),
            "scroll_driven_rules": scroll_driven.len(),
        },
        "running": log.running(),
        "keyframes": keyframes,
        "hover_focus_rules": states,
        "transitions": transitions,
        "scroll_driven": scroll_driven,
        "libraries": libraries.iter().map(|l| json!({"name": l.name, "detected_by": l.detected_by})).collect::<Vec<_>>(),
        "cut": {
            "running": log.cut,
            "keyframes": cut_keyframes,
            "hover_focus_rules": cut_states,
            "transitions": cut_transitions,
            "scroll_driven": cut_scroll,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> Value {
        json!({"y": 0, "total": 2, "list": [
            {"kind": "css-animation", "name": "fade-up", "target": "h1.fade-in", "pseudo": null,
             "keyframes": [{"offset": 0, "opacity": "0", "transform": "translateY(24px)"}, {"offset": 1, "opacity": "1", "transform": "none"}],
             "timing": {"duration": 800, "delay": 0, "endDelay": 0, "easing": "ease-out", "iterations": 1, "direction": "normal", "fill": "both", "iterationStart": 0},
             "timeline": {"kind": "document"}, "rangeStart": "normal", "rangeEnd": "normal", "playState": "running", "playbackRate": 1},
            {"kind": "script", "name": "slide", "target": "div#mover", "keyframes": [{"transform": "translateX(0px)", "evil key\u{202e}": "x"}],
             "timing": {"duration": 2000, "iterations": "infinite", "direction": "alternate", "easing": "ease-in-out"},
             "timeline": {"kind": "scroll", "axis": "block", "source": "html", "extra": "dropped"}, "unknown": "dropped"}
        ]})
    }

    #[test]
    fn snapshots_merge_without_repeats_and_keep_where_they_were_seen() {
        let mut log = AnimationLog::default();
        log.add_snapshot(&snapshot(), "at the top");
        let mut later = snapshot();
        later["y"] = json!(900);
        log.add_snapshot(&later, "while scrolling");
        log.add_snapshot(&snapshot(), "at the top");
        let running = log.running();
        assert_eq!(running.len(), 2);
        assert_eq!(running[0]["seen"], json!(["at the top", "while scrolling"]));
        assert_eq!(running[0]["first_seen_at_scroll_px"], 0);
        assert_eq!(log.counts(), (1, 0, 1));
        let script = &running[1];
        assert_eq!(script["timing"]["iterations"], "infinite");
        assert_eq!(script["timing"]["duration"], 2000);
        assert!(
            script.get("unknown").is_none(),
            "only known fields are kept"
        );
        assert!(script["timeline"].get("extra").is_none());
        let frame = script["keyframes"][0].as_object().unwrap();
        assert!(
            frame
                .keys()
                .all(|k| k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')),
            "{frame:?}"
        );
    }

    #[test]
    fn the_document_has_every_list_and_its_counts() {
        let mut log = AnimationLog::default();
        log.add_snapshot(&snapshot(), "at the top");
        let base = url::Url::parse("https://a.example/site.css").unwrap();
        let facts = crate::capture::css::facts(
            ".cta{transition:transform .2s}.cta:hover{transform:scale(1.1)}@keyframes spin{to{transform:rotate(1turn)}}",
            &base,
        );
        let doc = document(
            "https://a.example/",
            "2026-11-16T15:30:12Z",
            &log,
            &[("styles/site.css".into(), facts)],
            &[Library {
                name: "GSAP",
                detected_by: vec!["window.gsap is set".into()],
            }],
        );
        for key in [
            "running",
            "keyframes",
            "hover_focus_rules",
            "transitions",
            "scroll_driven",
            "libraries",
            "counts",
            "cut",
        ] {
            assert!(doc.get(key).is_some(), "{key}");
        }
        assert_eq!(doc["counts"]["css_animations"], 1);
        assert_eq!(doc["counts"]["script_animations"], 1);
        assert_eq!(doc["keyframes"][0]["name"], "spin");
        assert_eq!(doc["hover_focus_rules"][0]["selector"], ".cta:hover");
        assert_eq!(doc["transitions"][0]["transition"], "transform .2s");
        assert_eq!(doc["libraries"][0]["name"], "GSAP");
    }

    #[test]
    fn libraries_are_named_by_the_app_with_the_evidence() {
        let globals = json!({"gsap": "3.12.5", "ScrollTrigger": true, "lottie": "<b>evil</b>"});
        let markers = json!({"aos": 3, "lenis": 1, "svgSmil": 0});
        let scripts = [
            ScriptSample {
                url: "https://cdn.example/libs/gsap.min.js",
                text: "/* GreenSock */",
            },
            ScriptSample {
                url: "",
                text: "new Lenis({lerp: .1})",
            },
        ];
        let libs = detect_libraries(&globals, &markers, &scripts, 0, 0);
        let names: Vec<&str> = libs.iter().map(|l| l.name).collect();
        assert_eq!(
            names,
            [
                "GSAP",
                "GSAP ScrollTrigger",
                "Lottie",
                "Lenis smooth scroll",
                "AOS (animate on scroll)"
            ]
        );
        assert_eq!(
            libs[0].detected_by[0],
            "window.gsap is set (version 3.12.5)"
        );
        assert!(libs[0].detected_by.iter().any(|d| d.contains("gsap")));
        // A version that is not a plain version string is not quoted.
        assert_eq!(libs[2].detected_by, ["window.lottie is set"]);
        let none = detect_libraries(&json!({}), &json!({}), &[], 0, 0);
        assert!(none.is_empty());
        let webgl = detect_libraries(&json!({}), &json!({}), &[], 0, 2);
        assert_eq!(webgl[0].name, "WebGL canvas");
    }
}
