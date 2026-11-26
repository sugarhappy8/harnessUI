// Crate `runtime` as the harnesses see it: `sd` (sd.rs) holds the real lora_tags.rs and
// prompt_view.rs, read from the repository, and the one constant of args.rs they use.
pub mod sd;
