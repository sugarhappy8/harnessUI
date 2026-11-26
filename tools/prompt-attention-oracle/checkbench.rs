#[allow(dead_code)]
#[path = "../../src-tauri/src/studio/prompt_check.rs"]
mod prompt_check;
use std::time::{Duration, Instant};
fn main() {
    let l = runtime::sd::args::LOCAL_PROMPT_MAX_CHARS;
    let (half, quarter) = (l / 2, l / 4);
    let worst = [
        ("( x8192 then B x8192", format!("{}{}", "(".repeat(half), "B".repeat(half))),
        ("[ x8192 then B x8192", format!("{}{}", "[".repeat(half), "B".repeat(half))),
        ("([ x4096 then B x8192", format!("{}{}", "([".repeat(quarter), "B".repeat(half))),
        ("[B x8192", "[B".repeat(half)),
        ("(B x8192", "(B".repeat(half)),
        ("[ x4096 B x8192 ] x4096", format!("{}{}{}", "[".repeat(quarter), "B".repeat(half), "]".repeat(quarter))),
        ("plain words", "a red fox ".repeat(l / 10)),
    ];
    for (name, p) in &worst {
        let mut best = Duration::MAX;
        for _ in 0..3 {
            let s = Instant::now();
            assert!(!prompt_check::refuses(p));
            best = best.min(s.elapsed());
        }
        println!("{name:28} {:6} chars  {:?}", p.chars().count(), best);
    }
}
