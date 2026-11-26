use runtime::sd::prompt_view::parse_prompt_attention;
use std::io::Write;
use std::process::{Command, Stdio};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 { self.0 ^= self.0 << 13; self.0 ^= self.0 >> 7; self.0 ^= self.0 << 17; self.0 }
}

fn segs(bytes: &[u8]) -> Vec<(String, u32)> {
    bytes.split(|b| *b == 2).filter(|s| !s.is_empty()).map(|s| {
        let at = s.iter().position(|b| *b == 1).unwrap();
        (String::from_utf8(s[..at].to_vec()).unwrap(), std::str::from_utf8(&s[at + 1..]).unwrap().parse().unwrap())
    }).collect()
}

fn main() {
    // Bracket-heavy pieces: weights that underflow, overflow, zero, negate and stay near 1.
    let opens = ["(", "[", "((", "[[", "([", "[("];
    let closes = [")", "]", ":0.5)", ":1.1)", ":0.0001)", ":3)", ":0)", ":-1)", ":1)", ":0.9999)", ":1.0001)",
                  ":340282340000000000000000000000000000000)", ":0.000000000000000000000000000000000000012)",
                  ":0.00000000000000000000000000000000000000000001)", ":7)", ":.25)"];
    let texts = ["B", "x", "se", "xy", " ", "BREAK", ":", "\\(", "\\)", "é", "child", "ren"];
    let n: usize = std::env::args().nth(1).map(|s| s.parse().unwrap()).unwrap_or(20_000);
    let mut rng = Rng(std::env::args().nth(2).map(|s| s.parse().unwrap()).unwrap_or(0x9E37_79B9_7F4A_7C15));
    let mut prompts = Vec::with_capacity(n);
    for _ in 0..n {
        let len = 20 + (rng.next() % 1_500) as usize;
        let bias = rng.next() % 100; // how often a piece opens a bracket
        let mut p = String::new();
        for _ in 0..len {
            let r = rng.next() % 100;
            if r < bias / 2 + 10 { p.push_str(opens[(rng.next() % opens.len() as u64) as usize]); }
            else if r < bias / 2 + 10 + (100 - bias) / 3 { p.push_str(closes[(rng.next() % closes.len() as u64) as usize]); }
            else { p.push_str(texts[(rng.next() % texts.len() as u64) as usize]); }
        }
        prompts.push(p);
    }
    let mut input = Vec::new();
    for p in &prompts {
        input.extend_from_slice(p.as_bytes()); input.push(3);
        input.extend_from_slice(b"x"); input.push(0);
    }
    // A developer tool, outside the app: it starts the oracle directly.
    let mut child = Command::new(concat!(env!("D"), "/oracle")).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || stdin.write_all(&input).unwrap());
    let out = child.wait_with_output().unwrap().stdout;
    writer.join().unwrap();
    let records: Vec<&[u8]> = out.split(|b| *b == 0).collect();
    let (mut bad, mut subnormal, mut infinite, mut nan, mut zero, mut segments) = (0, 0, 0, 0, 0, 0usize);
    for (i, p) in prompts.iter().enumerate() {
        let rec = records[i];
        let sep = rec.iter().position(|b| *b == 3).unwrap();
        let want = segs(&rec[..sep]);
        let have: Vec<(String, u32)> = parse_prompt_attention(p).into_iter().map(|(t, w)| (t, w.to_bits())).collect();
        segments += want.len();
        for (_, w) in &want {
            let w = f32::from_bits(*w);
            if w.is_subnormal() { subnormal += 1 } else if w.is_infinite() { infinite += 1 } else if w.is_nan() { nan += 1 } else if w == 0.0 { zero += 1 }
        }
        if want != have { bad += 1; if bad <= 3 { println!("MISMATCH {p:?}\n  engine {want:?}\n  port   {have:?}"); } }
    }
    println!("cases={n} segments={segments} parse_prompt_attention mismatches={bad}; engine weights: subnormal={subnormal} infinite={infinite} nan={nan} zero={zero}");
}
