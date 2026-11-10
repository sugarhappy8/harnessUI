//! A short, fixed generation used to compare backends on the user's hardware.

use agent::{ChatMessage, ChatRequest, StreamEvent};
use futures_util::StreamExt;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Prompt long enough that prompt processing shows up in the timings.
fn bench_prompt() -> String {
    let mut s = String::from("Summarise the following notes in two sentences.\n\n");
    for i in 0..40 {
        s.push_str(&format!(
            "Note {i}: the quarterly report shows steady growth in the northern region, \
             with costs flat and two new hires in support.\n"
        ));
    }
    s
}

pub async fn time_generation(state: &AppState) -> AppResult<String> {
    let Some((base_url, key, _model, _n_ctx)) = state.runtime.endpoint().await else {
        return Err(AppError::internal("no model loaded"));
    };
    let client = crate::runtime_manager::local_client(base_url, key)?;
    let mut req = ChatRequest::new("local", vec![ChatMessage::user(bench_prompt())]);
    req.stream = true;
    req.max_tokens = Some(64);
    req.extra.insert(
        "chat_template_kwargs".into(),
        json!({ "enable_thinking": false }),
    );
    req.extra.insert("cache_prompt".into(), json!(false));
    let stream = client.stream_chat(&req, CancellationToken::new()).await?;
    futures_util::pin_mut!(stream);
    let mut line = String::from("no timings reported");
    while let Some(ev) = stream.next().await {
        match ev? {
            StreamEvent::Timings(t) => {
                line = format!(
                    "prompt {:.0} tokens/s ({} tokens), generation {:.1} tokens/s ({} tokens)",
                    t.prompt_per_second.unwrap_or(0.0),
                    t.prompt_n.unwrap_or(0),
                    t.predicted_per_second.unwrap_or(0.0),
                    t.predicted_n.unwrap_or(0)
                );
            }
            StreamEvent::Done => break,
            _ => {}
        }
    }
    Ok(line)
}
