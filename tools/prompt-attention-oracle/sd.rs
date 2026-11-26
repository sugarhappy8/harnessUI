// The real lora_tags.rs and prompt_view.rs, as crate `runtime`'s `sd` module holds them (paths
// relative to this file), and the one constant of args.rs the harnesses use. run.sh checks the
// constant against crates/runtime/src/sd/args.rs.
#[allow(dead_code)]
#[path = "../../crates/runtime/src/sd/lora_tags.rs"]
mod lora_tags;
#[path = "../../crates/runtime/src/sd/prompt_view.rs"]
pub mod prompt_view;
pub mod args {
    pub use super::prompt_view::{attention_text, parse_prompt_attention, strip_prompt_markers};
    pub const LOCAL_PROMPT_MAX_CHARS: usize = 16_384;
}
