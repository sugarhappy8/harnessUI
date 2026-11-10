# Research notes

These notes are what `ARCHITECTURE.md` was built from. They were gathered on 2026-11-10 against primary sources: crates.io, npm, upstream source at pinned tags, official docs, and the live Hugging Face API. Read them before writing code in the matching area. Several of these projects changed a great deal in 2026, and API names recalled from older versions will not compile.

Each subsystem has two files:

- `<topic>.md` is the first research pass.
- `<topic>.recheck.md` is an independent second pass that tried to refute the first. **Where the two disagree, the re-check is right.** Its "Things the research missed" section often matters more than its corrections.

Where a note and `ARCHITECTURE.md` disagree, the architecture document is the decision. The notes are the evidence.

| Topic | Read before |
|---|---|
| `llama-cpp` | M1. Release model, asset names, flags, endpoints, load progress, tool-call parsing, failure signatures |
| `rocm-*` | M1. Why upstream's Windows ROCm build does not work alone, the lemonade build, installer size limits |
| `tauri` | M0. Versions, capabilities, CSP, resources vs sidecars, signing, updater, tauri-action |
| `gguf-and-hardware` | M2. GGUF header format, memory estimate, hardware and backend detection |
| `huggingface` | M2. Search and download API, redirects, resume, gated repos, seed models with hashes |
| `rmcp-mcp` | M3. rmcp 3.9 API names, protocol revisions, handshake pitfalls, spawning servers |
| `agent-loop-and-skills` | M3 and M5. Stream shapes, fallback formats, compaction numbers, Agent Skills rules |
| `platform-crates` | M0 onward. Crate choices, versions and known traps for storage, secrets, processes, terminal, git |
| `browser-and-computer-use` | M6 and M7. CDP transport, form gating, capture and input crates, coordinate conventions |

## Limits of this research

- Nothing was run on Windows, on macOS or on a GPU. Behaviour on those platforms comes from source code, documentation and issue reports. Each note marks what was executed and what was only read.
- Paths written as `<temp workspace>` pointed at a scratch folder that no longer exists.
- Version numbers are a snapshot. Re-check with `cargo info <crate>` and `npm view <pkg> version` before pinning. `cargo search tauri` shows a 3.0 alpha; this project uses 2.x.
