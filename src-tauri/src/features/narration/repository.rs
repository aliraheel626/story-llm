use crate::features::timeline::{
    model::{kind, TimelineEntry},
    reducer, repository as timeline,
};
use crate::shared::error::AppResult;

use super::model::ActiveStoryEntry;
use crate::features::timeline::model::NarrationVariant;

fn to_story_entry(entry: TimelineEntry) -> ActiveStoryEntry {
    let role = if entry.kind == kind::PLAYER_MESSAGE {
        "player"
    } else {
        "narrator"
    };
    ActiveStoryEntry {
        id: entry.id,
        story_id: entry.story_id,
        seq: entry.seq,
        role: role.to_string(),
        input_mode: entry
            .payload
            .get("input_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("generated")
            .to_string(),
        content: entry.content.unwrap_or_default(),
        thoughts: None,
        created_at: entry.created_at,
        edited_at: None,
    }
}

pub(super) fn get_last_story_entry(
    conn: &rusqlite::Connection,
    story_id: &str,
) -> AppResult<Option<ActiveStoryEntry>> {
    let raw = timeline::list_logical_entries(conn, story_id)?;
    Ok(reducer::active_visible_entries(&raw)
        .pop()
        .map(to_story_entry))
}

pub(super) fn insert_story_entry(
    conn: &rusqlite::Connection,
    story_id: &str,
    role: &str,
    input_mode: &str,
    content: &str,
    thoughts: Option<&str>,
) -> AppResult<ActiveStoryEntry> {
    let event_kind = if role == "player" {
        kind::PLAYER_MESSAGE
    } else {
        kind::NARRATION
    };
    // Reasoning is stored for display only — it is deliberately left out of
    // `load_history`, so it never reaches the model as context.
    let thoughts = thoughts.map(str::trim).filter(|t| !t.is_empty());
    let payload = match thoughts {
        Some(thoughts) => serde_json::json!({ "input_mode": input_mode, "thoughts": thoughts }),
        None => serde_json::json!({ "input_mode": input_mode }),
    };
    let entry = timeline::append_entry(
        conn,
        story_id,
        event_kind,
        "visible",
        Some(content),
        &payload,
        None,
    )?;
    Ok(to_story_entry(entry))
}

pub(super) fn image_paths_for_entry(
    conn: &rusqlite::Connection,
    entry_id: &str,
) -> AppResult<Vec<String>> {
    timeline::image_paths_for_entry(conn, entry_id)
}

pub(super) fn list_variants(
    conn: &rusqlite::Connection,
    entry_id: &str,
) -> AppResult<Vec<NarrationVariant>> {
    let entry = timeline::get_entry(conn, entry_id)?;
    let raw = timeline::list_logical_entries(conn, &entry.story_id)?;
    Ok(reducer::variants_for_entry(&raw, entry_id))
}
