//! Reading the Studio engine's progress from its output (docs/dev/0.1.0/studio-local-design.md,
//! "Progress, Stop, failures").
//!
//! `sd-cli` prints log lines with a level prefix (`[I] `, `[W] `, `[E] `) and the source
//! file and line at the end (` --- image.cpp:954`), and redraws one progress bar in place:
//! `\r  |=====>     | 3/4 -   2.10s/it - ETA 00:02\x1b[K`, with `\n` only after the last
//! step. It draws `#` bars in MB/s or GB/s while it reads a part's weights and `=` bars in
//! `s/it` or `it/s` while it samples and, with `--vae-tiling`, while it encodes or decodes in
//! tiles. The engine runner reads both pipes with a reader that ends a line at `\r` too.
//!
//! The wording is the pinned build's (`master-1106-532d2c1`; `tests/fixtures/sd_progress_*.txt`
//! follow its source). It has no interface guarantee: a bump of the pin re-checks it.
//!
//! The stages move forward only. This build reads each part's weights when it is first used,
//! so "loading" lines and `#` bars come again while the prompt is read, while it draws and
//! while it decodes; they never move the stage back. `sampling using …` is printed before the
//! prompt is read (and before a picture added to the prompt is encoded), so it starts reading
//! the prompt, not the drawing; `get_learned_condition completed` starts the drawing. A video
//! prints `generate_video` only after its prompt was read, so `sampling using …` is what
//! shows it reading the prompt.

use std::borrow::Cow;

use serde::Serialize;

/// What the engine is doing, in the order it does it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LocalStage {
    /// Starting and reading the model's weights.
    Loading,
    /// Turning the prompt into what the model reads, from `generate_image` or `sampling
    /// using …`, whichever comes first (a video prints `generate_video` only after the prompt
    /// was read, when the drawing has already begun).
    ReadingPrompt,
    /// Drawing the picture or video, step by step.
    Drawing,
    /// Turning what was drawn into pixels.
    Decoding,
    /// Writing the files.
    Saving,
}

impl LocalStage {
    /// The stage's place in the run, for moving forward only.
    fn order(self) -> u8 {
        match self {
            Self::Loading => 0,
            Self::ReadingPrompt => 1,
            Self::Drawing => 2,
            Self::Decoding => 3,
            Self::Saving => 4,
        }
    }
}

/// Where a local run is, as the card shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalProgress {
    /// The current stage.
    pub stage: LocalStage,
    /// The drawing pass, from 1 (Wan 2.2 A14B draws in two).
    pub pass: u32,
    /// How many passes the model draws in.
    pub passes: u32,
    /// Steps done in this pass of this picture (0 until the first step ends).
    pub step: u32,
    /// Steps in this pass, from the engine's bar (0 until the first step ends).
    pub steps: u32,
    /// The picture being drawn, from 1.
    pub picture: u32,
    /// How many pictures the run makes.
    pub pictures: u32,
    /// Time since the engine started, as the caller measured it for this update.
    pub elapsed_ms: u64,
    /// Seconds the last step took, from the bar (`s/it`, or the inverse of `it/s`).
    pub sec_per_step: Option<f64>,
}

/// Turns the engine's output lines into [`LocalProgress`] updates.
#[derive(Debug, Clone)]
pub struct ProgressParser {
    last: LocalProgress,
}

impl ProgressParser {
    /// A parser for a run of `pictures` pictures (the batch count; 1 for a video) drawn in
    /// `passes` passes (2 for a model with a high-noise part). Zero counts as one.
    pub fn new(pictures: u32, passes: u32) -> Self {
        Self {
            last: LocalProgress {
                stage: LocalStage::Loading,
                pass: 1,
                passes: passes.max(1),
                step: 0,
                steps: 0,
                picture: 1,
                pictures: pictures.max(1),
                elapsed_ms: 0,
                sec_per_step: None,
            },
        }
    }

    /// Reads one line (from stdout or stderr, without its line end). Returns the progress
    /// when the line moved it or is a progress bar (every bar shows the engine at work, but
    /// only a step bar while drawing changes the step); `None` for any other line,
    /// including a marker of a stage already passed. `elapsed_ms` is stamped on the update.
    pub fn feed(&mut self, line: &str, elapsed_ms: u64) -> Option<LocalProgress> {
        let text = without_escapes(line);
        let message = without_level(&text);
        let moved = match parse_bar(message) {
            Some(bar) => {
                self.on_bar(&bar);
                true
            }
            None => self.on_log(message),
        };
        if !moved {
            return None;
        }
        self.last.elapsed_ms = elapsed_ms;
        Some(self.last.clone())
    }

    /// Only an `=` bar timed per step, while drawing, counts steps: `#` bars read weights,
    /// and the decoder's tile bars come after the drawing.
    fn on_bar(&mut self, bar: &Bar) {
        let sec_per_step = match bar.unit {
            Unit::SecondsPerStep => bar.speed,
            Unit::StepsPerSecond => 1.0 / bar.speed,
            Unit::Bytes => return,
        };
        if bar.kind != BarKind::Steps || self.last.stage != LocalStage::Drawing {
            return;
        }
        self.last.step = bar.step;
        self.last.steps = bar.steps;
        self.last.sec_per_step =
            Some(sec_per_step).filter(|seconds| seconds.is_finite() && *seconds > 0.0);
    }

    fn on_log(&mut self, message: &str) -> bool {
        if message.starts_with("loading ") {
            // A part or its weights (`loading diffusion model from …`, `loading tensors
            // completed`): news only while the run is still loading.
            return self.last.stage == LocalStage::Loading;
        }
        if message.starts_with("generate_image")
            || message.starts_with("generate_video")
            || message.starts_with("sampling using ")
        {
            // `generate_video` comes after `get_learned_condition completed` in this build
            // (video.cpp), so for a video it is a stage already passed. The sampling plan
            // prints `sampling using …` (request.cpp) before the prompt is read, after
            // `generate_image` for a picture: for a video it is the first sign of the prompt.
            return self.advance(LocalStage::ReadingPrompt);
        }
        if message.starts_with("get_learned_condition completed") {
            // Not `sampling using …`: it comes before the prompt is read and before an added
            // picture is encoded, whose tile bars would then count as steps.
            return self.advance(LocalStage::Drawing);
        }
        if let Some(rest) = message.strip_prefix("generating image: ") {
            return self.next_picture(rest);
        }
        if message.starts_with("sampling(high noise) completed") {
            // The first pass (`sampling(high noise) using …`) is announced before the
            // prompt is read; the second starts when the first completes.
            return self.next_pass();
        }
        if message.starts_with("decoding ")
            || (message.starts_with("generating ")
                && (message.contains(" latent images completed")
                    || message.contains(" latent video completed")))
        {
            return self.advance(LocalStage::Decoding);
        }
        if message.starts_with("save result") {
            return self.advance(LocalStage::Saving);
        }
        false
    }

    fn advance(&mut self, stage: LocalStage) -> bool {
        if stage.order() <= self.last.stage.order() {
            return false;
        }
        self.last.stage = stage;
        true
    }

    /// `generating image: i/n - seed …`: picture `i` starts at its first pass.
    fn next_picture(&mut self, rest: &str) -> bool {
        let count = rest.split(" - ").next().unwrap_or_default();
        let Some(picture) = count
            .split_once('/')
            .and_then(|(i, _)| i.trim().parse::<u32>().ok())
        else {
            return false;
        };
        let picture = picture.clamp(1, self.last.pictures);
        if picture <= self.last.picture {
            return false;
        }
        self.last.picture = picture;
        self.last.pass = 1;
        self.last.step = 0;
        true
    }

    fn next_pass(&mut self) -> bool {
        if self.last.pass >= self.last.passes {
            return false;
        }
        self.last.pass += 1;
        self.last.step = 0;
        true
    }
}

/// How far the whole run is, from 0 to 0.99: loading 0–0.10, reading the prompt 0.10–0.15,
/// drawing 0.15–0.90 (by step, pass and picture), decoding 0.90–0.99, saving 0.99. Only the
/// drawing has steps to count, so every other stage is at the start of its share.
pub fn overall(p: &LocalProgress) -> f32 {
    match p.stage {
        LocalStage::Loading => 0.0,
        LocalStage::ReadingPrompt => 0.10,
        LocalStage::Drawing => (0.15 + 0.75 * drawn(p)) as f32,
        LocalStage::Decoding => 0.90,
        LocalStage::Saving => 0.99,
    }
}

/// The share of the drawing done, from 0 to 1.
fn drawn(p: &LocalProgress) -> f64 {
    let pictures = p.pictures.max(1);
    let passes = p.passes.max(1);
    let in_pass = if p.steps > 0 {
        f64::from(p.step.min(p.steps)) / f64::from(p.steps)
    } else {
        0.0
    };
    let passes_done = f64::from(p.picture.clamp(1, pictures) - 1) * f64::from(passes)
        + f64::from(p.pass.clamp(1, passes) - 1);
    ((passes_done + in_pass) / (f64::from(pictures) * f64::from(passes))).clamp(0.0, 1.0)
}

/// What a bar draws with: `=` (steps or tiles) or `#` (weights read).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BarKind {
    Steps,
    Bytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unit {
    SecondsPerStep,
    StepsPerSecond,
    Bytes,
}

/// One redraw of the engine's bar: `  |====>   | 3/4 -   2.10s/it`.
#[derive(Debug, Clone, PartialEq)]
struct Bar {
    kind: BarKind,
    step: u32,
    steps: u32,
    speed: f64,
    unit: Unit,
}

fn parse_bar(text: &str) -> Option<Bar> {
    let rest = text.trim_start().strip_prefix('|')?;
    let (drawing, rest) = rest.split_once('|')?;
    let kind = if drawing.contains(['=', '>']) && drawing.chars().all(|c| "=> ".contains(c)) {
        BarKind::Steps
    } else if drawing.contains('#') && drawing.chars().all(|c| "# ".contains(c)) {
        BarKind::Bytes
    } else {
        return None;
    };
    let rest = rest.split(" - ETA ").next().unwrap_or_default();
    let (count, speed) = rest.trim().split_once(" - ")?;
    let (step, steps) = count.split_once('/')?;
    let step: u32 = step.trim().parse().ok()?;
    let steps: u32 = steps.trim().parse().ok()?;
    if steps == 0 || step > steps {
        return None;
    }
    let speed = speed.trim();
    let (number, unit) = [
        ("s/it", Unit::SecondsPerStep),
        ("it/s", Unit::StepsPerSecond),
        ("MB/s", Unit::Bytes),
        ("GB/s", Unit::Bytes),
    ]
    .into_iter()
    .find_map(|(suffix, unit)| Some((speed.strip_suffix(suffix)?, unit)))?;
    Some(Bar {
        kind,
        step,
        steps,
        speed: number.trim().parse().ok()?,
        unit,
    })
}

/// The line without terminal escape sequences (`\x1b[K` after each redraw, colours with
/// `--color`).
fn without_escapes(line: &str) -> Cow<'_, str> {
    if !line.contains('\x1b') {
        return Cow::Borrowed(line);
    }
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        if chars.next_if_eq(&'[').is_some() {
            // A control sequence ends at its first byte in `@`..=`~`.
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        }
    }
    Cow::Owned(out)
}

/// The message without the level prefix (`[I] `, `[W] `, `[E] `, …).
fn without_level(text: &str) -> &str {
    match text.as_bytes() {
        [b'[', level, b']', b' ', ..] if level.is_ascii_graphic() => &text[4..],
        _ => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llama::lines::LineReader;

    const FLUX2_KLEIN: &str = include_str!("../../tests/fixtures/sd_progress_flux2_klein.txt");
    const WAN22_TI2V: &str = include_str!("../../tests/fixtures/sd_progress_wan22_ti2v.txt");
    const BATCH_OF_TWO: &str = include_str!("../../tests/fixtures/sd_progress_batch_of_two.txt");
    const TWO_PASSES: &str = include_str!("../../tests/fixtures/sd_progress_two_passes.txt");
    const VAE_TILING: &str = include_str!("../../tests/fixtures/sd_progress_vae_tiling.txt");

    /// The fixture's lines as the engine runner reads them: `\r` ends a line too.
    fn engine_lines(text: &str) -> Vec<String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a runtime");
        runtime.block_on(async {
            let mut reader = LineReader::splitting_cr(text.as_bytes());
            let mut lines = Vec::new();
            while let Some(line) = reader.next_line().await {
                lines.push(line);
            }
            lines
        })
    }

    /// Every update the parser gives for the fixture, with the line that gave it.
    fn updates(parser: &mut ProgressParser, text: &str) -> Vec<(String, LocalProgress)> {
        engine_lines(text)
            .into_iter()
            .enumerate()
            .filter_map(|(i, line)| {
                let update = parser.feed(&line, i as u64 * 100)?;
                Some((line, update))
            })
            .collect()
    }

    fn drawing_parser(pictures: u32, passes: u32) -> ProgressParser {
        let mut parser = ProgressParser::new(pictures, passes);
        let update = parser
            .feed("[I] get_learned_condition completed, taking 0.81s", 8_000)
            .expect("an update");
        assert_eq!(update.stage, LocalStage::Drawing);
        parser
    }

    fn stages(updates: &[(String, LocalProgress)]) -> Vec<LocalStage> {
        let mut stages: Vec<LocalStage> = updates.iter().map(|(_, u)| u.stage).collect();
        stages.dedup();
        stages
    }

    #[test]
    fn steps_come_from_the_bar_in_either_unit() {
        let mut p = drawing_parser(1, 1);
        let x = p
            .feed(
                "  |=====>        | 3/4 -      2.10s/it - ETA 00:02\x1b[K",
                9_000,
            )
            .unwrap();
        assert_eq!(
            (x.stage, x.step, x.steps, x.sec_per_step),
            (LocalStage::Drawing, 3, 4, Some(2.10))
        );
        assert_eq!(x.elapsed_ms, 9_000);
        assert!(
            (p.feed("  |======>       | 4/4 -      1.60it/s\x1b[K", 9_600)
                .unwrap()
                .sec_per_step
                .unwrap()
                - 0.625)
                .abs()
                < 1e-9
        );
    }

    #[test]
    fn escape_sequences_and_the_level_are_stripped() {
        let mut p = ProgressParser::new(1, 1);
        let x = p
            .feed(
                "\x1b[34;1m[I]\x1b[0m generate_image 1024x1024 --- image.cpp:954",
                10,
            )
            .unwrap();
        assert_eq!(x.stage, LocalStage::ReadingPrompt);
        assert_eq!(p.feed("", 20), None);
        assert_eq!(
            p.feed(
                "[I] Version: Flux.2 klein  --- diffusion_engine.cpp:982",
                30
            ),
            None
        );
    }

    #[test]
    fn the_flux2_fixture_walks_every_stage_in_order() {
        let updates = updates(&mut ProgressParser::new(1, 1), FLUX2_KLEIN);
        assert_eq!(
            stages(&updates),
            [
                LocalStage::Loading,
                LocalStage::ReadingPrompt,
                LocalStage::Drawing,
                LocalStage::Decoding,
                LocalStage::Saving
            ]
        );
        let (_, last) = updates.last().unwrap();
        assert_eq!(
            (
                last.step,
                last.steps,
                last.picture,
                last.pictures,
                last.pass,
                last.passes
            ),
            (4, 4, 1, 1, 1, 1)
        );
        assert_eq!(last.sec_per_step, Some(4.43));
    }

    #[test]
    fn loading_bars_and_tile_bars_are_not_steps() {
        // Weights read before the prompt (`--eager-load`) draw "#" bars while loading. Fed by
        // hand: the fixtures use this build's default, lazy loading, which reads the weights
        // only after `generate_image`.
        let mut p = ProgressParser::new(1, 1);
        for line in [
            "  |################                                  | 120/398 - 410.20MB/s\x1b[K",
            "  |##################################################| 398/398 -   1.08GB/s\x1b[K",
        ] {
            let x = p.feed(line, 500).expect("a loading update");
            assert_eq!((x.stage, x.step, x.steps), (LocalStage::Loading, 0, 0));
        }

        // This build reads each part's weights when it is first used, so "#" bars also come
        // while the prompt is read and before the first step: none of them is a step.
        let flux2 = updates(&mut ProgressParser::new(1, 1), FLUX2_KLEIN);
        let first_step = flux2.iter().position(|(_, u)| u.step > 0).expect("a step");
        assert!(
            flux2[..first_step]
                .iter()
                .any(|(line, u)| line.contains('#') && u.stage == LocalStage::Drawing)
        );
        assert!(flux2[..first_step].iter().all(|(_, u)| u.step == 0));

        // With `--vae-tiling` the decoder's "=" bars count tiles, not steps.
        let tiling = updates(&mut ProgressParser::new(1, 1), VAE_TILING);
        let decoding = tiling
            .iter()
            .position(|(_, u)| u.stage == LocalStage::Decoding)
            .expect("the decoding");
        let saving = tiling
            .iter()
            .position(|(_, u)| u.stage == LocalStage::Saving)
            .expect("the saving line");
        let tile_bars = &tiling[decoding + 1..saving];
        assert_eq!(
            tile_bars
                .iter()
                .filter(|(line, _)| line.contains("/16 -"))
                .count(),
            16
        );
        for (line, u) in tile_bars {
            assert_eq!(
                (u.stage, u.step, u.steps),
                (LocalStage::Decoding, 4, 4),
                "{line}"
            );
        }
    }

    #[test]
    fn bars_before_the_prompt_is_read_are_not_steps() {
        // `sampling using …` comes before the prompt is read and before a picture added to
        // the prompt is encoded, which `--vae-tiling` also draws as "=" bars.
        let tile_bar =
            "  |===>                                              |  1/16 -   1.61it/s\x1b[K";
        let mut p = ProgressParser::new(1, 1);
        p.feed("[I] generate_image 1024x1024 --- image.cpp:954", 100);
        assert_eq!(
            p.feed("[I] sampling using euler method --- request.cpp:478", 110),
            None
        );
        let x = p.feed(tile_bar, 400).expect("an update");
        assert_eq!(
            (x.stage, x.step, x.steps),
            (LocalStage::ReadingPrompt, 0, 0)
        );

        // A video prints no `generate_video` before its prompt: `sampling using …` starts
        // reading the prompt (ruling L3-video-prompt-stage), and its bars are still not steps.
        let mut p = ProgressParser::new(1, 1);
        let x = p
            .feed("[I] sampling using euler method --- request.cpp:478", 110)
            .expect("an update");
        assert_eq!(x.stage, LocalStage::ReadingPrompt);
        let x = p.feed(tile_bar, 400).expect("an update");
        assert_eq!(
            (x.stage, x.step, x.steps),
            (LocalStage::ReadingPrompt, 0, 0)
        );
    }

    #[test]
    fn a_late_loading_line_never_moves_the_stage_back() {
        let mut p = drawing_parser(1, 1);
        p.feed(
            "  |============>      | 1/4 -   4.62s/it - ETA 00:13\x1b[K",
            9_000,
        );
        p.feed(
            "[I] loading tensors completed, taking 3.10s (read: 2.80s, memcpy: 0.00s, convert: 0.00s, copy_to_backend: 0.28s) --- model_loader.cpp:1437",
            9_100,
        );
        p.feed(
            "[I] loading diffusion model from '/m/x.gguf' --- diffusion_engine.cpp:697",
            9_150,
        );
        let x = p
            .feed(
                "  |=========================>   | 2/4 -   4.41s/it - ETA 00:09\x1b[K",
                9_200,
            )
            .unwrap();
        assert_eq!((x.stage, x.step), (LocalStage::Drawing, 2));
        // An earlier stage's marker is not an update at all.
        assert_eq!(
            p.feed("[I] generate_image 1024x1024 --- image.cpp:954", 9_300),
            None
        );
    }

    #[test]
    fn a_batch_of_two_counts_pictures() {
        let updates = updates(&mut ProgressParser::new(2, 1), BATCH_OF_TWO);
        let second = updates
            .iter()
            .position(|(line, _)| line.contains("generating image: 2/2"))
            .expect("the second picture");
        let (_, before) = &updates[second - 1];
        assert_eq!((before.picture, before.step, before.steps), (1, 4, 4));
        let (_, x) = &updates[second];
        assert_eq!((x.picture, x.pictures, x.step), (2, 2, 0));
        let (_, last) = updates
            .iter()
            .rfind(|(_, u)| u.stage == LocalStage::Drawing)
            .unwrap();
        assert_eq!(
            (last.picture, last.pictures, last.step, last.steps),
            (2, 2, 4, 4)
        );
    }

    #[test]
    fn two_pass_models_report_pass_2_of_2() {
        let updates = updates(&mut ProgressParser::new(1, 2), TWO_PASSES);
        let switch = updates
            .iter()
            .position(|(line, _)| line.contains("sampling(high noise) completed"))
            .expect("the end of the first pass");
        assert!(
            updates[..switch]
                .iter()
                .all(|(_, u)| (u.pass, u.passes) == (1, 2))
        );
        assert!(updates[..switch].iter().any(|(_, u)| u.step == 4));
        let (_, x) = &updates[switch];
        assert_eq!((x.pass, x.passes, x.step), (2, 2, 0));
        assert!((overall(x) - 0.525).abs() < 1e-6);
        let (_, last_drawing) = updates
            .iter()
            .rfind(|(_, u)| u.stage == LocalStage::Drawing)
            .unwrap();
        assert_eq!(
            (last_drawing.pass, last_drawing.step, last_drawing.steps),
            (2, 4, 4)
        );
    }

    #[test]
    fn the_overall_fraction_never_goes_back() {
        for (name, text, pictures, passes) in [
            ("wan22_ti2v", WAN22_TI2V, 1, 1),
            ("flux2_klein", FLUX2_KLEIN, 1, 1),
            ("batch_of_two", BATCH_OF_TWO, 2, 1),
            ("two_passes", TWO_PASSES, 1, 2),
            ("vae_tiling", VAE_TILING, 1, 1),
        ] {
            let updates = updates(&mut ProgressParser::new(pictures, passes), text);
            let fractions: Vec<f32> = updates.iter().map(|(_, u)| overall(u)).collect();
            assert!(
                fractions.windows(2).all(|pair| pair[0] <= pair[1]),
                "{name}: {fractions:?}"
            );
            assert_eq!(fractions.first(), Some(&0.0), "{name}");
            assert_eq!(fractions.last(), Some(&0.99), "{name}");
        }
        // A video's prompt is read after `sampling using …`; `generate_video` is printed only
        // once the drawing has begun.
        let wan = updates(&mut ProgressParser::new(1, 1), WAN22_TI2V);
        assert_eq!(
            stages(&wan),
            [
                LocalStage::Loading,
                LocalStage::ReadingPrompt,
                LocalStage::Drawing,
                LocalStage::Decoding,
                LocalStage::Saving
            ]
        );
    }

    #[test]
    fn each_stage_has_its_share_of_the_card() {
        let at = |stage, step, steps| LocalProgress {
            stage,
            pass: 1,
            passes: 1,
            step,
            steps,
            picture: 1,
            pictures: 1,
            elapsed_ms: 0,
            sec_per_step: None,
        };
        assert_eq!(overall(&at(LocalStage::Loading, 0, 0)), 0.0);
        assert_eq!(overall(&at(LocalStage::ReadingPrompt, 0, 0)), 0.10);
        assert_eq!(overall(&at(LocalStage::Drawing, 0, 0)), 0.15);
        assert!((overall(&at(LocalStage::Drawing, 2, 4)) - 0.525).abs() < 1e-6);
        assert_eq!(overall(&at(LocalStage::Drawing, 4, 4)), 0.90);
        assert_eq!(overall(&at(LocalStage::Decoding, 4, 4)), 0.90);
        assert_eq!(overall(&at(LocalStage::Saving, 4, 4)), 0.99);
    }

    #[test]
    fn progress_reaches_the_webview_in_camel_case() {
        let x = drawing_parser(1, 1)
            .feed("  |=====>        | 3/4 -      2.10s/it\x1b[K", 9_000)
            .unwrap();
        assert_eq!(
            serde_json::to_value(&x).unwrap(),
            serde_json::json!({
                "stage": "drawing", "pass": 1, "passes": 1, "step": 3, "steps": 4,
                "picture": 1, "pictures": 1, "elapsedMs": 9000, "secPerStep": 2.1
            })
        );
        assert_eq!(
            serde_json::to_value(LocalStage::ReadingPrompt).unwrap(),
            "readingPrompt"
        );
    }

    #[test]
    fn the_fixtures_were_recorded_from_the_pinned_build() {
        assert_eq!(
            include_str!("../../tests/fixtures/sd_fixtures_build.txt").trim(),
            crate::sd::lock::embedded().sd_cpp.tag
        );
    }
}
