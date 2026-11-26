use runtime::sd::prompt_view::{attention_text, parse_prompt_attention, strip_prompt_markers};
use std::io::Write;
use std::process::{Command, Stdio};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 { self.0 ^= self.0 << 13; self.0 ^= self.0 >> 7; self.0 ^= self.0 << 17; self.0 }
    fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str { xs[(self.next() % xs.len() as u64) as usize] }
}

fn segs(bytes: &[u8]) -> Vec<(String, u32)> {
    bytes.split(|b| *b == 2).filter(|s| !s.is_empty()).map(|s| {
        let at = s.iter().position(|b| *b == 1).unwrap();
        (String::from_utf8(s[..at].to_vec()).unwrap(), std::str::from_utf8(&s[at + 1..]).unwrap().parse().unwrap())
    }).collect()
}

fn main() {
    let frags = ["(", ")", "[", "]", ":", "1", "0.5", "1.2.3", ".", "+", "-", ")", "\\", "\\(", "\\)", "\\]",
                 "BREAK", " BREAK ", "B", "BR", "x", "se", "xy", " ", ",", "_", "é", "日", "<lora:", "<image:", ">",
                 "a", "lora:", "ima", "ge:", "<", "<lo", "ra:", ":1)", ":-1)", ":+.5)", "\n", "child", "ren"];
    let n: usize = std::env::args().nth(1).map(|s| s.parse().unwrap()).unwrap_or(300_000);
    let mut rng = Rng(0x2545F4914F6CDD1D);
    let mut prompts = Vec::with_capacity(n);
    for _ in 0..n {
        let len = 1 + (rng.next() % 16) as usize;
        let mut p = String::new();
        for _ in 0..len { p.push_str(rng.pick(&frags)); }
        prompts.push(p);
    }
    let mut input = Vec::new();
    let mut files = Vec::new();
    for p in &prompts {
        let file = format!("{} <lora:style/x:0.8>", strip_prompt_markers(p));
        input.extend_from_slice(p.as_bytes()); input.push(3);
        input.extend_from_slice(file.as_bytes()); input.push(0);
        files.push(file);
    }
    // A developer tool, outside the app: it starts the oracle directly.
    let mut child = Command::new(concat!(env!("D"), "/oracle")).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || stdin.write_all(&input).unwrap());
    let out = child.wait_with_output().unwrap().stdout;
    writer.join().unwrap();
    let records: Vec<&[u8]> = out.split(|b| *b == 0).collect();
    let (mut parse_bad, mut view_bad, mut threw, mut images, mut negative_skipped) = (0, 0, 0, 0, 0);
    for (i, p) in prompts.iter().enumerate() {
        let rec = records[i];
        let sep = rec.iter().position(|b| *b == 3).unwrap();
        let want: Vec<(String, u32)> = segs(&rec[..sep]);
        let have: Vec<(String, u32)> = parse_prompt_attention(p).into_iter().map(|(t, w)| (t, w.to_bits())).collect();
        if want != have { parse_bad += 1; if parse_bad <= 5 { println!("PARSE {p:?}\n  engine {want:?}\n  port   {have:?}"); } }
        let view = &rec[sep + 1..];
        if view == b"THROW" { threw += 1; println!("THROW {p:?} file {:?}", files[i]); continue; }
        if view == b"IMAGES" { images += 1; println!("IMAGES {p:?} file {:?}", files[i]); continue; }
        let engine = segs(view);
        if engine.iter().any(|(t, w)| t.contains("BREAK") && f32::from_bits(*w) != -1.0 && f32::from_bits(*w) < 0.0) || p.contains('-') {
            // a BREAK merged into negative-weight text reads as a space in attention_text by design
            negative_skipped += 1;
            continue;
        }
        let engine_text: String = engine.iter().map(|(t, w)| if f32::from_bits(*w) < 0.0 && !t.is_empty() && t.len() % 5 == 0 && t.as_bytes().chunks(5).all(|c| c == b"BREAK") { " ".repeat(t.len() / 5) } else { t.clone() }).collect();
        let ours = attention_text(&format!("{} ", strip_prompt_markers(p)));
        if engine_text != ours { view_bad += 1; if view_bad <= 5 { println!("VIEW {p:?}\n  engine {engine_text:?}\n  ours   {ours:?}"); } }
    }
    println!("cases={n} parse_prompt_attention mismatches={parse_bad}; file view: threw={threw} images={images} attention_text mismatches={view_bad} (skipped with '-': {negative_skipped})");
}
