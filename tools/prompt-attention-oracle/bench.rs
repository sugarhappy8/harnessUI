use runtime::sd::prompt_view::{parse_prompt_attention, strip_prompt_markers, attention_text};
use std::time::Instant;
fn t(name: &str, p: &str) {
    let s = Instant::now();
    let r = strip_prompt_markers(p);
    let segs = parse_prompt_attention(&r);
    let a = attention_text(&r);
    println!("{name:40} chars={:7} segs={:7} attn={} {:?}", p.chars().count(), segs.len(), a.len(), s.elapsed());
}
fn main() {
    for l in [16_384usize, 32_768, 65_536] {
        t(&format!("P1 (^b B^s unclosed L={l}"), &format!("{}{}", "(".repeat(l/2), "B".repeat(l/2)));
        t(&format!("P1sq [^b B^s unclosed L={l}"), &format!("{}{}", "[".repeat(l/2), "B".repeat(l/2)));
        t(&format!("P2 (B unclosed L={l}"), &"(B".repeat(l/2));
        t(&format!("P3 (^b B^s )^b L={l}"), &format!("{}{}{}", "(".repeat(l/4), "B".repeat(l/2), ")".repeat(l/4)));
        t(&format!("P4 (B^k )^k L={l}"), &format!("{}{}", "(B".repeat(l/3), ")".repeat(l/3)));
    }
}
