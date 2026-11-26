# The prompt-attention oracle

Studio's child-safety prompt check reads every local prompt four ways
(`src-tauri/src/studio/prompt_check.rs`). The fourth way is the words the model is given after
the engine's weighting syntax: `(word)`, `[word]`, `(word:1.2)` and `BREAK` drop away and
segments of equal weight join, so `se()xy` reaches the model as "sexy". That reading is a Rust
port of stable-diffusion.cpp's own `parse_prompt_attention`
(`crates/runtime/src/sd/prompt_view.rs`), and the check is only as good as the port is exact.

These harnesses check the port against the engine's code itself, and time the check at its
bound. They are developer tools, not part of the app or of CI. Run them after any change to
`prompt_view.rs` or `lora_tags.rs`, and whenever the engine pin moves.

## The engine pin

`oracle.cpp` copies, verbatim, from stable-diffusion.cpp at commit
`532d2c15c430a785a712b2d94281f04e733008a4` (`sd.lock.json`, tag `master-1106-532d2c1`):

- `parse_prompt_attention`, `src/core/util.cpp:909-1019`;
- `parse_prompt_input` and `prompt_input_text`, `src/conditioning/prompt_input.cpp:69-105` and
  `113-121`.

That code is under stable-diffusion.cpp's MIT licence, whose notice is kept at the top of
`oracle.cpp`. Its `extract_and_remove_lora` is not the engine's code: it emulates how
`examples/common/common.cpp:2356-2426` removes LoRA tags from the prompt file. When the pin
moves, copy those functions again from the new commit, with their new line numbers, and run
everything.

## Files

| File | What it does |
| --- | --- |
| `oracle.cpp` | The engine's functions, reading prompts on stdin and writing each one's segments, as bit-exact weights, and the prompt file's view. |
| `shim.rs`, `sd.rs` | Crate `runtime` as the harnesses see it: the repository's own `lora_tags.rs` and `prompt_view.rs`, by relative path, and `LOCAL_PROMPT_MAX_CHARS` (`run.sh` checks it against `args.rs`). |
| `fuzz.rs` | 300,000 prompts of random fragments (brackets, weights, `BREAK`, escapes, LoRA and image markers, Unicode): `parse_prompt_attention` against the engine, and the prompt file's view against `attention_text`. |
| `deepfuzz.rs` | Bracket-heavy prompts of up to 1,500 pieces, with weights that underflow, overflow, reach zero or turn negative: `parse_prompt_attention` against the engine. |
| `checkbench.rs` | The whole prompt check (`prompt_check::refuses`) on the worst bracket nestings at the 16,384-character local bound, best of three, release build. |
| `bench.rs` | `parse_prompt_attention` and `attention_text` alone, at the bound and at two and four times it. |
| `run.sh` | Builds everything into a temporary folder (`OUT` to choose one) and runs it. |

## Run it

```bash
bash tools/prompt-attention-oracle/run.sh
```

It needs a C++17 compiler and `rustc`, writes nothing in the repository, and takes about ten
minutes, most of it the oracle's `std::regex`.

## What they proved

Recorded for 0.2.0 (tasks S2 and G3, and again when this folder was added):

- **0 mismatches** between the port and the engine over 300,000 random-fragment prompts
  (`fuzz.rs`) and 220,000 bracket-heavy prompts (`deepfuzz.rs`: 100,000 + 100,000 + 20,000),
  every weight compared bit for bit, subnormal, infinite, NaN and zero weights included; and
  no difference between the prompt file the engine reads and what `attention_text` gives.
- **0.17 s worst case** for the whole prompt check at 16,384 characters, in a release build
  (`[` 8,192 times then `B` 8,192 times; other nestings are faster, and plain words take
  about 13 ms). The engine's weighting grows with the square of the brackets, which is why
  the local bound is 16,384 characters (ARCHITECTURE.md 7.9). Timings depend on the computer.

If any run prints a mismatch, the port no longer reads prompts as the engine does: fix
`prompt_view.rs` before anything ships.
