//! Thin façade over Rig. The rest of the app depends on the types in this
//! module, never on rig-core/rig-agent types directly — keeps Rig's pre-1.0
//! API churn contained to one place.

mod reasoning_strip;

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use futures::StreamExt;
use rig_agent::agent::{ToolCall, ToolResultEvent};
use rig_agent::prelude::*;
use rig_agent::tool::{DynamicTool, ToolOutput, ToolResult};
use rig_core::providers::{openai, openrouter};
use rig_core::streaming::StreamedAssistantContent;

use crate::shared::error::{AppError, AppResult};
use reasoning_strip::ReasoningStripper;

/// Nous Portal's OpenAI-compatible inference gateway. Standard Chat
/// Completions shape (confirmed against live docs), not OpenRouter's — no
/// OpenRouter-specific request extensions are sent to this host.
pub const NOUS_PORTAL_BASE_URL: &str = "https://inference-api.nousresearch.com/v1";

/// Cloudflare (fronting Nous Portal) blocks headerless requests as bot
/// traffic — sent on every request to Nous Portal, including the narration
/// client below and the model-metadata fetch in `features::settings`.
pub const NOUS_PORTAL_USER_AGENT: &str = "Dungeon/0.1 (+https://github.com/dungeon-app/dungeon)";

#[derive(Debug, Clone)]
pub struct TextModelConfig {
    /// `"openrouter"` or `"nous_portal"` — see `build_agent`. Anything else
    /// falls back to OpenRouter, matching `settings::read_text_model_settings`.
    pub provider: String,
    pub model: String,
    pub api_key: String,
    pub context_window: usize,
}

/// One turn of prior conversation, already resolved from the persisted ledger.
#[derive(Debug, Clone)]
pub struct HistoryTurn {
    pub entry_id: Option<String>,
    pub is_player: bool,
    pub content: String,
    pub marker: HistoryTurnMarker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryTurnMarker {
    Ledger,
    Summary,
}

pub struct NarrateRequest {
    pub config: TextModelConfig,
    pub preamble: String,
    pub history: Vec<HistoryTurn>,
    pub prompt: String,
    /// End a tool-bearing run after its first result instead of asking the
    /// model for a follow-up completion.
    pub stop_after_tool_result: bool,
    /// OpenRouter reasoning effort for this request (`none`/`low`/`high`/…).
    /// `None` leaves the model at its own default. Sent as an OpenRouter
    /// request-body extension; changing it mid-story changes the request
    /// prefix, which invalidates the provider's prompt cache.
    pub reasoning_effort: Option<String>,
    /// Tools the agent may call mid-generation. `ai::mod` deliberately never
    /// sees the concrete tool types, only Rig's runtime-defined `DynamicTool`.
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

#[derive(Debug, Clone, PartialEq)]
pub struct CompletedToolCall {
    pub tool: String,
    pub args: serde_json::Value,
    pub result: serde_json::Value,
    pub ok: bool,
}

#[derive(Default)]
struct ToolCallCapture {
    next_order: usize,
    started: HashMap<String, usize>,
    completed: Vec<(usize, CompletedToolCall)>,
}

impl ToolCallCapture {
    fn start(&mut self, call_id: &str) {
        if self.started.contains_key(call_id) {
            return;
        }
        let order = self.next_order;
        self.next_order += 1;
        self.started.insert(call_id.to_string(), order);
    }

    fn finish(&mut self, call_id: &str, call: CompletedToolCall) {
        let order = self.started.get(call_id).copied().unwrap_or_else(|| {
            let order = self.next_order;
            self.next_order += 1;
            order
        });
        self.completed.push((order, call));
    }

    fn take_completed(&mut self) -> Vec<CompletedToolCall> {
        self.completed.sort_by_key(|(order, _)| *order);
        std::mem::take(&mut self.completed)
            .into_iter()
            .map(|(_, call)| call)
            .collect()
    }
}

fn json_or_string(value: &str) -> serde_json::Value {
    serde_json::from_str(value).unwrap_or_else(|_| serde_json::Value::String(value.to_string()))
}

fn canonical_tool_result(output: &ToolOutput) -> serde_json::Value {
    output
        .as_json()
        .cloned()
        .unwrap_or_else(|| serde_json::Value::String(output.render()))
}

fn completed_tool_call(tool: &str, args: &str, result: &ToolResult) -> CompletedToolCall {
    CompletedToolCall {
        tool: tool.to_string(),
        args: json_or_string(args),
        result: canonical_tool_result(result.output()),
        ok: result.is_success(),
    }
}

/// Observes tool calls as they happen and queues a `NarratorChunk` for each,
/// so `stream_narration` can forward live activity through the same
/// `on_chunk` callback used for text/reasoning deltas. `on_tool_call` fires
/// before execution — the stream's own `ToolExecutionCommitted` item is
/// deferred until the whole tool batch settles, so it can't drive a live
/// "started" indicator.
struct ActivityHook {
    buffer: Arc<Mutex<Vec<NarratorChunk>>>,
    completed: Arc<Mutex<ToolCallCapture>>,
    stop_reason: Option<String>,
}

impl ActivityHook {
    fn tool_result_action(&self, tool_name: &str) -> rig_agent::agent::ToolResultAction {
        if tool_name == crate::prompts::ILLUSTRATE_SCENE_TOOL_NAME {
            if let Some(reason) = &self.stop_reason {
                return rig_agent::agent::ToolResultAction::stop(reason.clone());
            }
        }
        rig_agent::agent::ToolResultAction::Keep
    }
}

impl AgentHook for ActivityHook {
    async fn on_tool_call(
        &self,
        _ctx: &HookContext,
        event: ToolCall<'_>,
    ) -> rig_agent::agent::ToolCallAction {
        log::debug!(
            "tool call started: {} call_id={} args={}",
            event.tool_name,
            event.internal_call_id,
            event.args
        );
        self.completed
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .start(event.internal_call_id);
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
        if event.raw_result.is_success() {
            log::debug!(
                "tool call finished: {} call_id={} ok",
                event.tool_name,
                event.internal_call_id
            );
        } else {
            log::warn!(
                "tool call finished: {} call_id={} result={:?}",
                event.tool_name,
                event.internal_call_id,
                event.raw_result
            );
        }
        self.completed
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .finish(
                event.internal_call_id,
                completed_tool_call(event.tool_name, event.args, event.raw_result),
            );
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
        self.tool_result_action(event.tool_name)
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

fn is_expected_tool_stop(
    stop_reason: Option<&str>,
    error: &rig_agent::agent::StreamingError,
) -> bool {
    stop_reason.is_some_and(|stop_reason| {
        matches!(
            error,
            rig_agent::agent::StreamingError::Prompt(error)
                if matches!(
                    error.as_ref(),
                    PromptError::PromptCancelled { reason, .. } if reason == stop_reason
                )
        )
    })
}

/// Streams a narration turn, invoking `on_chunk` for every visible-text or
/// reasoning delta as it arrives. Returns the full visible text and full
/// reasoning text once the stream ends.
pub async fn stream_narration<F>(
    req: NarrateRequest,
    on_chunk: F,
) -> AppResult<(String, String, Vec<CompletedToolCall>)>
where
    F: FnMut(NarratorChunk),
{
    let has_tools = !req.tools.is_empty();
    let stop_reason = req
        .stop_after_tool_result
        .then(|| format!("decision captured/{}", uuid::Uuid::new_v4()));
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
    let completed = Arc::new(Mutex::new(ToolCallCapture::default()));
    let mut runner = agent.runner(req.prompt).history(history);
    if has_tools {
        runner = runner.add_hook(ActivityHook {
            buffer: activity_buffer.clone(),
            completed: completed.clone(),
            stop_reason: stop_reason.clone(),
        });
    }
    let stream = runner.stream().await;
    let (visible, thoughts) = consume_narration_stream(
        stream,
        has_tools,
        stop_reason.as_deref(),
        &activity_buffer,
        on_chunk,
    )
    .await?;
    let tool_calls = completed
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take_completed();
    Ok((visible, thoughts, tool_calls))
}

async fn consume_narration_stream<F>(
    mut stream: rig_agent::agent::StreamingResult,
    has_tools: bool,
    stop_reason: Option<&str>,
    activity_buffer: &Mutex<Vec<NarratorChunk>>,
    mut on_chunk: F,
) -> AppResult<(String, String)>
where
    F: FnMut(NarratorChunk),
{
    let mut text_stripper = ReasoningStripper::new();
    let mut visible = String::new();
    let mut thoughts = String::new();

    while let Some(item) = stream.next().await {
        if has_tools {
            drain_activity_buffer(activity_buffer, &mut on_chunk);
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
            Err(e) if is_expected_tool_stop(stop_reason, &e) => break,
            Err(e) => {
                return Err(AppError::Other(format!("narrator stream error: {e}")));
            }
        }
    }

    if has_tools {
        drain_activity_buffer(activity_buffer, &mut on_chunk);
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
    if config.provider == "nous_portal" {
        // Nous Portal is plain OpenAI Chat Completions — no OpenRouter wire
        // extensions (e.g. `reasoning.effort`) are sent here; that field is
        // an OpenRouter-specific extension with no confirmed Nous Portal
        // contract, so `reasoning_effort` is silently ignored for this
        // provider rather than risk an unrecognized-field rejection.
        // rig's HTTP client sends no User-Agent by default, which Cloudflare
        // (fronting Nous Portal) treats as bot traffic and blocks with a 403
        // before the request ever reaches the API — confirmed live. A
        // normal-looking app UA is enough to pass that check.
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::USER_AGENT,
            http::HeaderValue::from_static(NOUS_PORTAL_USER_AGENT),
        );
        let client = openai::Client::builder()
            .api_key(config.api_key.clone())
            .base_url(NOUS_PORTAL_BASE_URL)
            .http_headers(headers)
            .build()
            .map_err(|e| AppError::Other(format!("failed to build Nous Portal client: {e}")))?
            .completions_api();
        let builder = client.agent(config.model.clone()).preamble(preamble);
        let agent = if tools.is_empty() {
            builder.build()
        } else {
            builder
                .dynamic_tools(tools)
                .default_max_turns(MAX_TOOL_TURNS)
                .build()
        };
        return Ok(agent);
    }

    let client = openrouter::Client::builder()
        .api_key(config.api_key.clone())
        .with_app_identity("Dungeon", "https://github.com/dungeon-app/dungeon")
        .build()
        .map_err(|e| AppError::Other(format!("failed to build OpenRouter client: {e}")))?;
    let builder = client.agent(config.model.clone()).preamble(preamble);
    // OpenRouter accepts provider-specific request fields it doesn't model
    // natively; `reasoning.effort` is how the effort selector reaches the
    // upstream model.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn decision_tool_result_stops_and_ends_the_stream_cleanly() {
        let stop_reason = "decision captured/test-call";
        let hook = ActivityHook {
            buffer: Arc::new(Mutex::new(Vec::new())),
            completed: Arc::new(Mutex::new(ToolCallCapture::default())),
            stop_reason: Some(stop_reason.to_string()),
        };
        assert_eq!(
            hook.tool_result_action(crate::prompts::ILLUSTRATE_SCENE_TOOL_NAME),
            rig_agent::agent::ToolResultAction::stop(stop_reason)
        );
        assert_eq!(
            hook.tool_result_action("get_entities"),
            rig_agent::agent::ToolResultAction::Keep
        );

        let error =
            rig_agent::agent::StreamingError::Prompt(Box::new(PromptError::PromptCancelled {
                chat_history: Vec::new(),
                reason: stop_reason.to_string(),
            }));
        assert!(is_expected_tool_stop(Some(stop_reason), &error));
        assert!(!is_expected_tool_stop(None, &error));
        assert!(!is_expected_tool_stop(
            Some("decision captured/other-call"),
            &error
        ));

        let stream: rig_agent::agent::StreamingResult =
            Box::pin(futures::stream::iter(vec![Err(error)]));
        let activity_buffer = Mutex::new(vec![NarratorChunk::ToolActivity {
            call_id: "call-1".into(),
            tool_name: "illustrate_scene".into(),
            args: "{}".into(),
            phase: ToolActivityPhase::Finished { ok: true },
        }]);
        let mut chunks = Vec::new();
        let output =
            consume_narration_stream(stream, true, Some(stop_reason), &activity_buffer, |chunk| {
                chunks.push(chunk)
            })
            .await
            .unwrap();

        assert_eq!(output, (String::new(), String::new()));
        assert!(matches!(
            chunks.as_slice(),
            [NarratorChunk::ToolActivity {
                phase: ToolActivityPhase::Finished { ok: true },
                ..
            }]
        ));
    }

    #[test]
    fn completed_calls_preserve_canonical_results_and_rig_success_semantics() {
        let no_op = ToolResult::success(ToolOutput::json(serde_json::json!({
            "applied": false
        })));
        let captured = completed_tool_call("adjust_entity_attribute", r#"{"delta":1}"#, &no_op);
        assert_eq!(captured.args, serde_json::json!({"delta": 1}));
        assert_eq!(captured.result, serde_json::json!({"applied": false}));
        assert!(captured.ok);

        let failure = ToolResult::failed(rig_agent::tool::ToolExecutionError::invalid_args(
            "bad arguments",
        ));
        let captured = completed_tool_call("roll_check", "not-json", &failure);
        assert_eq!(captured.args, serde_json::json!("not-json"));
        assert_eq!(captured.result, serde_json::json!("bad arguments"));
        assert!(!captured.ok);
    }

    #[test]
    fn completed_calls_follow_call_start_order() {
        let mut capture = ToolCallCapture::default();
        capture.start("first");
        capture.start("second");
        let result = ToolResult::success(ToolOutput::json(serde_json::json!({"ok": true})));
        capture.finish("second", completed_tool_call("second", "{}", &result));
        capture.finish("first", completed_tool_call("first", "{}", &result));

        assert_eq!(
            capture
                .take_completed()
                .into_iter()
                .map(|call| call.tool)
                .collect::<Vec<_>>(),
            ["first", "second"]
        );
    }
}
