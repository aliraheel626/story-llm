use super::{
    attachments,
    model::{kind as transcript_kind, TranscriptEntry},
    repository as transcript_repository,
};
use crate::shared::error::AppResult;

/// "Edit": append a content override for any visible transcript entry.
pub(super) fn edit_transcript_entry(
    conn: &rusqlite::Transaction<'_>,
    entry_id: &str,
    content: &str,
) -> AppResult<TranscriptEntry> {
    let target = transcript_repository::get_entry(conn, entry_id)?;
    attachments::detach_from_entry(conn, entry_id)?;
    transcript_repository::append_entry(
        conn,
        &target.story_id,
        transcript_kind::CONTENT_EDITED,
        "hidden",
        Some(content),
        &serde_json::json!({"reason":"user_edit"}),
        Some(entry_id),
        target.turn_id.as_deref(),
    )?;
    transcript_repository::active_entry(conn, entry_id)
}
