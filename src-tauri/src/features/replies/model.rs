use serde::Serialize;

use crate::features::ledger::model::LedgerEntry;

#[derive(Debug, Clone, Serialize)]
pub struct SubmitTurnResult {
    pub entry: LedgerEntry,
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
    pub(super) entry: LedgerEntry,
}
