//! Thin façade over Rig. The rest of the app depends on the types in this
//! module, never on rig-core/rig-agent types directly — keeps Rig's pre-1.0
//! API churn contained to one place.

mod reasoning_strip;

use std::sync::{Arc, Mutex};

use futures::StreamExt;
use rig_agent::agent::{ToolCall, ToolResultEvent};
use rig_agent::prelude::*;
use rig_agent::tool::DynamicTool;
use rig_core::providers::openrouter;
use rig_core::streaming::StreamedAssistantContent;

use crate::shared::error::{AppError, AppResult};
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
    pub context_window: usize,
}

/// One turn of prior conversation, already resolved from the persisted timeline.
#[derive(Debug, Clone)]
pub struct HistoryTurn {
    pub entry_id: Option<String>,
    pub is_player: bool,
    pub content: String,
}

pub struct NarrateRequest {
    pub config: TextModelConfig,
    pub preamble: String,
    pub history: Vec<HistoryTurn>,
    pub prompt: String,
    /// Tools the narrator may call mid-generation (see `narration::tools`).
    /// Empty for every call site except `submit_turn`'s tool-calling path —
    /// `ai::mod` deliberately never sees the concrete tool types, only Rig's
    /// own runtime-defined `DynamicTool`.
    /// OpenRouter reasoning effort for this request (`none`/`low`/`high`/…).
    /// `None` leaves the model at its own default. Sent as an OpenRouter
    /// request-body extension; changing it mid-story changes the request
    /// prefix, which invalidates the provider's prompt cache.
    pub reasoning_effort: Option<String>,
    pub tools: Vec<DynamicTool>,
}

#[derive(Debug, Clone)]
pub enum ToolActivityPhase {
    Started,
    Finished { ok: bool },
}

#[derive(Debug, Clone)]
pub enum NarratorChunk {
    Text(String),
    Reasoning(String),
    ToolActivity {
        call_id: String,
        tool_name: String,
        args: String,
        phase: ToolActivityPhase,
    },
}

/// Observes tool calls as they happen and queues a `NarratorChunk` for each,
/// so `stream_narration` can forward live activity through the same
/// `on_chunk` callback used for text/reasoning deltas. `on_tool_call` fires
/// before execution — the stream's own `ToolExecutionCommitted` item is
/// deferred until the whole tool batch settles, so it can't drive a live
/// "started" indicator.
struct ActivityHook {
    buffer: Arc<Mutex<Vec<NarratorChunk>>>,
}

impl AgentHook for ActivityHook {
    async fn on_tool_call(
        &self,
        _ctx: &HookContext,
        event: ToolCall<'_>,
    ) -> rig_agent::agent::ToolCallAction {
        if let Ok(mut buf) = self.buffer.lock() {
            buf.push(NarratorChunk::ToolActivity {
                call_id: event.internal_call_id.to_string(),
                tool_name: event.tool_name.to_string(),
                args: event.args.to_string(),
                phase: ToolActivityPhase::Started,
            });
        }
        rig_agent::agent::ToolCallAction::Run
    }

    async fn on_tool_result(
        &self,
        _ctx: &HookContext,
        event: ToolResultEvent<'_>,
    ) -> rig_agent::agent::ToolResultAction {
        if let Ok(mut buf) = self.buffer.lock() {
            buf.push(NarratorChunk::ToolActivity {
                call_id: event.internal_call_id.to_string(),
                tool_name: event.tool_name.to_string(),
                args: event.args.to_string(),
                phase: ToolActivityPhase::Finished {
                    ok: event.raw_result.is_success(),
                },
            });
        }
        rig_agent::agent::ToolResultAction::Keep
    }

    fn observes(&self, kind: rig_agent::agent::StepEventKind) -> bool {
        matches!(
            kind,
            rig_agent::agent::StepEventKind::ToolCall | rig_agent::agent::StepEventKind::ToolResult
        )
    }
}

fn drain_activity_buffer<F>(activity_buffer: &Mutex<Vec<NarratorChunk>>, on_chunk: &mut F)
where
    F: FnMut(NarratorChunk),
{
    let queued = {
        let mut buffer = activity_buffer.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *buffer)
    };
    for chunk in queued {
        on_chunk(chunk);
    }
}

/// Streams a narration turn, invoking `on_chunk` for every visible-text or
/// reasoning delta as it arrives. Returns the full visible text and full
/// reasoning text once the stream ends.
pub async fn stream_narration<F>(
    req: NarrateRequest,
    mut on_chunk: F,
) -> AppResult<(String, String)>
where
    F: FnMut(NarratorChunk),
{
    let has_tools = !req.tools.is_empty();
    let agent = build_agent(
        &req.config,
        &req.preamble,
        req.tools,
        req.reasoning_effort.as_deref(),
    )?;

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

    let activity_buffer: Arc<Mutex<Vec<NarratorChunk>>> = Arc::new(Mutex::new(Vec::new()));
    let mut runner = agent.runner(req.prompt).history(history);
    if has_tools {
        runner = runner.add_hook(ActivityHook {
            buffer: activity_buffer.clone(),
        });
    }
    let mut stream = runner.stream().await;

    let mut text_stripper = ReasoningStripper::new();
    let mut visible = String::new();
    let mut thoughts = String::new();

    while let Some(item) = stream.next().await {
        if has_tools {
            drain_activity_buffer(&activity_buffer, &mut on_chunk);
        }
        match item {
            Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(t))) => {
                let delta = text_stripper.push(&t.text);
                if !delta.is_empty() {
                    visible.push_str(&delta);
                    on_chunk(NarratorChunk::Text(delta));
                }
            }
            Ok(MultiTurnStreamItem::StreamAssistantItem(
                StreamedAssistantContent::ReasoningDelta { reasoning, .. },
            )) => {
                thoughts.push_str(&reasoning);
                on_chunk(NarratorChunk::Reasoning(reasoning));
            }
            Ok(_) => {}
            Err(e) => {
                return Err(AppError::Other(format!("narrator stream error: {e}")));
            }
        }
    }

    if has_tools {
        drain_activity_buffer(&activity_buffer, &mut on_chunk);
    }

    let tail = text_stripper.finalize();
    if !tail.is_empty() {
        visible.push_str(&tail);
        on_chunk(NarratorChunk::Text(tail));
    }

    Ok((visible, thoughts))
}

/// Structured, schema-validated completion using Rig's native structured-output
/// mode rather than prose parsing. `T` must round-trip through
/// `agent.prompt_typed::<T>()`.
pub async fn prompt_typed<T>(
    config: &TextModelConfig,
    preamble: &str,
    prompt: String,
) -> AppResult<T>
where
    T: schemars::JsonSchema + serde::de::DeserializeOwned + Send + 'static,
{
    let agent = build_agent(config, preamble, Vec::new(), None)?;
    agent
        .prompt_typed::<T>(prompt)
        .await
        .map_err(|e| AppError::Other(format!("structured prompt failed: {e}")))
}

/// Model round-trips a tool-calling turn may take. Rig's builder defaults to
/// `max_turns: 1`, which aborts the whole turn the moment the model calls any
/// tool (`MaxTurnsError`) — it never gets to see the tool result and narrate.
/// This allows a realistic sequence (look up entities, roll, adjust state,
/// then narrate) while still bounding a confused model's loop.
const MAX_TOOL_TURNS: usize = 8;

fn build_agent(
    config: &TextModelConfig,
    preamble: &str,
    tools: Vec<DynamicTool>,
    reasoning_effort: Option<&str>,
) -> AppResult<rig_agent::Agent> {
    match config.provider {
        TextProviderKind::OpenRouter => {
            let client = openrouter::Client::builder()
                .api_key(config.api_key.clone())
                .with_app_identity("Dungeon", "https://github.com/dungeon-app/dungeon")
                .build()
                .map_err(|e| AppError::Other(format!("failed to build OpenRouter client: {e}")))?;
            let builder = client.agent(config.model.clone()).preamble(preamble);
            // OpenRouter accepts provider-specific request fields it doesn't
            // model natively; `reasoning.effort` is how the effort selector
            // reaches the upstream model.
            let builder = match reasoning_effort {
                Some(effort) => builder.additional_params(serde_json::json!({
                    "reasoning": { "effort": effort },
                })),
                None => builder,
            };
            let agent = if tools.is_empty() {
                builder.build()
            } else {
                builder
                    .dynamic_tools(tools)
                    .default_max_turns(MAX_TOOL_TURNS)
                    .build()
            };
            Ok(agent)
        }
    }
}
