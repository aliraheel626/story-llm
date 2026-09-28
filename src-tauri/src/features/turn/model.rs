use serde::Serialize;

use crate::features::transcript::model::TranscriptEntry;

#[derive(Debug, Clone, Serialize)]
pub struct SubmitTurnResult {
    pub entry: TranscriptEntry,
    pub stream_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RetryResult {
    pub entry_id: String,
    pub stream_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct NarrationDonePayload {
    pub(super) stream_id: String,
    pub(super) entry: TranscriptEntry,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct NarrationTextCompletePayload {
    pub(super) stream_id: String,
}
