//! The story's Author's Note. It has no table of its own: the note is the
//! latest `CONTEXT_NOTE_UPDATED` timeline event, so reading it "as of" a point
//! in the timeline is just picking the newest such event at or before it.

use rusqlite::OptionalExtension;
use serde_json::json;
use tauri::State;

use crate::features::timeline::{model::kind as timeline_kind, repository};
use crate::shared::db::Pool;
use crate::shared::error::{AppError, AppResult};

/// The note text a `CONTEXT_NOTE_UPDATED` event carries. The one place that
/// knows the payload shape; history replay reads events through it too.
pub(super) fn event_note(payload: &serde_json::Value) -> &str {
    payload
        .get("author_note")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
}

fn note_from_latest_event(
    conn: &rusqlite::Connection,
    story_id: &str,
    through_seq: Option<i64>,
    before_seq: Option<i64>,
) -> AppResult<Option<String>> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM stories WHERE id = ?1)",
        [story_id],
        |row| row.get(0),
    )?;
    if !exists {
        return Err(AppError::NotFound(format!("story {story_id} not found")));
    }

    let payload: Option<String> = conn
        .query_row(
            "SELECT payload_json FROM timeline_entries
             WHERE story_id = ?1 AND kind = ?2
               AND (?3 IS NULL OR seq <= ?3)
               AND (?4 IS NULL OR seq < ?4)
             ORDER BY seq DESC LIMIT 1",
            rusqlite::params![
                story_id,
                timeline_kind::CONTEXT_NOTE_UPDATED,
                through_seq,
                before_seq
            ],
            |row| row.get(0),
        )
        .optional()?;
    let note = payload
        .and_then(|payload| serde_json::from_str::<serde_json::Value>(&payload).ok())
        .map(|payload| event_note(&payload).trim().to_string())
        .unwrap_or_default();
    Ok((!note.is_empty()).then_some(note))
}

#[tauri::command]
pub fn get_author_note(pool: State<Pool>, story_id: String) -> AppResult<String> {
    let conn = pool.get()?;
    Ok(note_from_latest_event(&conn, &story_id, None, None)?.unwrap_or_default())
}

#[tauri::command]
pub fn save_author_note(pool: State<Pool>, story_id: String, note: String) -> AppResult<()> {
    let conn = pool.get()?;
    let note = note.trim();
    repository::append_entry(
        &conn,
        &story_id,
        timeline_kind::CONTEXT_NOTE_UPDATED,
        "hidden",
        Some(&format!("Author's note was updated: {note}")),
        &json!({"author_note": note}),
        None,
    )?;
    Ok(())
}

/// The note in force just before `before_seq` (the latest one when `None`).
/// Used to size the prompt before compaction has decided anything.
pub(crate) fn current_for_cut(
    pool: &Pool,
    story_id: &str,
    before_seq: Option<i64>,
) -> AppResult<Option<String>> {
    let conn = pool.get()?;
    note_from_latest_event(&conn, story_id, None, before_seq)
}

/// The note as of the compaction boundary `through_seq` — the one baked into
/// the system prompt. Edits after the boundary reach the model as events in
/// the replayed history instead. With no boundary yet, nothing is baked.
pub(crate) fn at_boundary(
    pool: &Pool,
    story_id: &str,
    through_seq: Option<i64>,
) -> AppResult<Option<String>> {
    let Some(through_seq) = through_seq else {
        return Ok(None);
    };
    let conn = pool.get()?;
    note_from_latest_event(&conn, story_id, Some(through_seq), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baked_note_follows_the_compaction_boundary() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
             VALUES ('s', 'story', 'now', 'now', '{}')",
            [],
        )
        .unwrap();
        let first = repository::append_entry(
            &conn,
            "s",
            timeline_kind::CONTEXT_NOTE_UPDATED,
            "hidden",
            Some("first"),
            &json!({"author_note":"first"}),
            None,
        )
        .unwrap();
        repository::append_entry(
            &conn,
            "s",
            timeline_kind::PLAYER_MESSAGE,
            "visible",
            Some("act"),
            &json!({"input_mode":"do"}),
            None,
        )
        .unwrap();
        let second = repository::append_entry(
            &conn,
            "s",
            timeline_kind::CONTEXT_NOTE_UPDATED,
            "hidden",
            Some("second"),
            &json!({"author_note":"second"}),
            None,
        )
        .unwrap();
        drop(conn);

        assert_eq!(at_boundary(&pool, "s", None).unwrap(), None);
        assert_eq!(
            at_boundary(&pool, "s", Some(first.seq)).unwrap().as_deref(),
            Some("first")
        );
        assert_eq!(
            at_boundary(&pool, "s", Some(second.seq))
                .unwrap()
                .as_deref(),
            Some("second")
        );
        assert_eq!(
            current_for_cut(&pool, "s", Some(second.seq))
                .unwrap()
                .as_deref(),
            Some("first")
        );
    }
}
