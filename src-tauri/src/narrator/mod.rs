//! Thin façade over Rig. The rest of the app depends on the types in this
//! module, never on rig-core/rig-agent types directly — keeps Rig's pre-1.0
//! API churn contained to one place.

mod reasoning_strip;

use futures::StreamExt;
use rig_agent::prelude::*;
use rig_core::providers::openrouter;
use rig_core::streaming::StreamedAssistantContent;

use crate::error::{AppError, AppResult};
use reasoning_strip::ReasoningStripper;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextProviderKind {
    OpenRouter,
}

#[derive(Debug, Clone)]
pub struct TextModelConfig {
    pub provider: TextProviderKind,
    pub model: String,
    pub api_key: String,
}

/// One turn of prior conversation, already resolved from persisted passages.
#[derive(Debug, Clone)]
pub struct HistoryTurn {
    pub is_player: bool,
    pub content: String,
}

pub struct NarrateRequest {
    pub config: TextModelConfig,
    pub preamble: String,
    pub history: Vec<HistoryTurn>,
    pub prompt: String,
}

#[derive(Debug, Clone)]
pub enum NarratorChunk {
    Text(String),
    Reasoning(String),
}

/// Streams a narration turn, invoking `on_chunk` for every visible-text or
/// reasoning delta as it arrives. Returns the full visible text and full
/// reasoning text once the stream ends.
pub async fn stream_narration<F>(req: NarrateRequest, mut on_chunk: F) -> AppResult<(String, String)>
where
    F: FnMut(NarratorChunk),
{
    let agent = build_agent(&req.config, &req.preamble)?;

    let history: Vec<rig_core::completion::Message> = req
        .history
        .iter()
        .map(|t| {
            if t.is_player {
                rig_core::completion::Message::user(t.content.clone())
            } else {
                rig_core::completion::Message::assistant(t.content.clone())
            }
        })
        .collect();

    let runner = agent.runner(req.prompt).history(history);
    let mut stream = runner.stream().await;

    let mut text_stripper = ReasoningStripper::new();
    let mut visible = String::new();
    let mut thoughts = String::new();

    while let Some(item) = stream.next().await {
        match item {
            Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(t))) => {
                let delta = text_stripper.push(&t.text);
                if !delta.is_empty() {
                    visible.push_str(&delta);
                    on_chunk(NarratorChunk::Text(delta));
                }
            }
            Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::ReasoningDelta {
                reasoning,
                ..
            })) => {
                thoughts.push_str(&reasoning);
                on_chunk(NarratorChunk::Reasoning(reasoning));
            }
            Ok(_) => {}
            Err(e) => {
                return Err(AppError::Other(format!("narrator stream error: {e}")));
            }
        }
    }

    let tail = text_stripper.finalize();
    if !tail.is_empty() {
        visible.push_str(&tail);
        on_chunk(NarratorChunk::Text(tail));
    }

    Ok((visible, thoughts))
}

/// Structured, schema-validated completion (used by the mechanics engine's
/// classify/update stages) — Rig's native structured-output mode, not prose
/// parsing. `T` must round-trip through `agent.prompt_typed::<T>()`.
pub async fn prompt_typed<T>(config: &TextModelConfig, preamble: &str, prompt: String) -> AppResult<T>
where
    T: schemars::JsonSchema + serde::de::DeserializeOwned + Send + 'static,
{
    let agent = build_agent(config, preamble)?;
    agent
        .prompt_typed::<T>(prompt)
        .await
        .map_err(|e| AppError::Other(format!("structured prompt failed: {e}")))
}

fn build_agent(config: &TextModelConfig, preamble: &str) -> AppResult<rig_agent::Agent> {
    match config.provider {
        TextProviderKind::OpenRouter => {
            let client = openrouter::Client::builder()
                .api_key(config.api_key.clone())
                .with_app_identity("Dungeon", "https://github.com/dungeon-app/dungeon")
                .build()
                .map_err(|e| AppError::Other(format!("failed to build OpenRouter client: {e}")))?;
            let agent = client.agent(config.model.clone()).preamble(preamble).build();
            Ok(agent)
        }
    }
}
