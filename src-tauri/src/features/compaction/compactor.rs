use rig_core::{
    completion::Message,
    memory::{Compactor, MemoryError},
    wasm_compat::WasmBoxedFuture,
};

use crate::ai::{self, TextModelConfig};
use crate::prompts;

use super::summary::{format_summary, ContextSummary, SummaryArtifact};

#[derive(Clone)]
pub struct NarratorCompactor {
    config: TextModelConfig,
}

impl NarratorCompactor {
    pub fn new(config: TextModelConfig) -> Self {
        Self { config }
    }
}

impl Compactor for NarratorCompactor {
    type Artifact = SummaryArtifact;

    fn compact<'a>(
        &'a self,
        _conversation_id: &'a str,
        evicted: &'a [Message],
        carry_over: Option<&'a Self::Artifact>,
    ) -> WasmBoxedFuture<'a, Result<Self::Artifact, MemoryError>> {
        Box::pin(async move {
            let prior = carry_over.map(|a| format_summary(&a.0)).unwrap_or_default();
            let transcript =
                serde_json::to_string(evicted).map_err(|e| MemoryError::Internal(e.to_string()))?;
            let prompt = format!(
                "Previous summary:\n{prior}\n\nOlder timeline messages to compact:\n{transcript}"
            );
            ai::prompt_typed::<ContextSummary>(
                &self.config,
                prompts::COMPACTION_SUMMARY_SYSTEM_PROMPT,
                prompt,
            )
            .await
            .map(SummaryArtifact)
            .map_err(|e| MemoryError::Policy(e.to_string()))
        })
    }
}
