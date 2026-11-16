//! What the model is told about rebuilding a captured design (ARCHITECTURE.md 9.3,
//! `browser_capture_site`).
//!
//! [`METHOD`] ends every `report.md` (and so every capture result), which is where a model
//! reads it when it needs it. [`GUIDE`] and [`GUIDE_SHORT`] are the same method shortened
//! for the system message's tool guide (`agent::prompt::guide`, owned by the tool-guide
//! work): they are meant to be shown when `browser_capture_site` is on, the way the
//! computer-use lines are shown when `computer_screenshot` is on.

/// One sentence on what the tool is for, used in the tool's description, the report and
/// the card.
pub const PURPOSE: &str = "This is for studying a design and rebuilding it with the user's own content; do not reproduce other people's logos, brand names or sign-in pages.";

/// The method, step by step, as `report.md` ends.
pub const METHOD: &[&str] = &[
    "1. Look: read this report and look at the screenshots (the first screen at each width came with the capture's result; the full pages are in screenshots/). Note the sections from top to bottom.",
    "2. Plan: list the sections (header, hero, features, footer…). For each, note its layout (columns, widths, gaps), type sizes, colours and the motion it uses.",
    "3. Rebuild with clean HTML and CSS (or the user's framework), with the user's own text, pictures and logo. Take exact values from styles/ and the lists above: fonts, colours, sizes, spacing, breakpoints. Do not copy the page's scripts.",
    "4. Recreate the motion from animations.json: CSS @keyframes and transitions where the page used CSS; element.animate() (the Web Animations API) for script animations, with the same keyframes, duration, delay and easing; IntersectionObserver or CSS scroll-driven animations (animation-timeline: view()) for effects that start while scrolling.",
    "5. Preview: create an html artifact to show it beside the chat (its pictures and fonts must be data: URLs there, and the user may have to press Run scripts), or write the files into a folder for the user to open in a browser.",
    "6. Compare with the screenshots at each width, section by section, and fix the differences in spacing, sizes, colours, breakpoints and timing. Ask the user what looks different when you cannot see the preview.",
];

/// The method for the system message's tool guide, when the tool is on.
pub const GUIDE: &[&str] = &[
    "Rebuilding a website's look (browser_capture_site):",
    "- Capture the page, read report.md and look at the screenshots; plan the page as sections.",
    "- Rebuild with clean HTML and CSS and the user's own text, pictures and logo; take fonts, colours, sizes, spacing and breakpoints from styles/ and the report.",
    "- Recreate motion from animations.json: CSS keyframes and transitions, element.animate() for script animations, IntersectionObserver or animation-timeline for scroll effects.",
    "- Preview it (an html artifact, or files the user opens), compare with screenshots/ at each width and fix the differences.",
    "- Never reproduce other people's logos, brand names or sign-in pages.",
];

/// The same in one line, for the guide's short form.
pub const GUIDE_SHORT: &str = "Website capture: capture, read report.md, rebuild with the user's own content, recreate motion from animations.json, compare with screenshots/; never copy logos, brand names or sign-in pages.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_method_names_every_step_and_the_limits() {
        let all = METHOD.join("\n");
        for word in ["report", "screenshots", "sections", "user's own", "animations.json", "artifact", "Compare"] {
            assert!(all.contains(word), "{word}");
        }
        assert!(PURPOSE.contains("logos") && PURPOSE.contains("brand names") && PURPOSE.contains("sign-in pages"));
        assert!(GUIDE.join("\n").contains("logos"));
        assert!(GUIDE_SHORT.len() < 220);
        for line in METHOD.iter().chain(GUIDE) {
            assert!(!line.contains('!'), "no exclamation marks: {line}");
        }
    }
}
