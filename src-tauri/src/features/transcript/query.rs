use std::collections::{HashMap, HashSet};

use rusqlite::OptionalExtension;

use crate::shared::error::{AppError, AppResult};

use super::{
    model::{kind, TranscriptEntry},
    reducer, repository,
};

/// Which transcript content to return. `None` means no filter on that field.
#[derive(Debug, Clone, Default)]
pub struct TranscriptQuery<'a> {
    pub since_seq: Option<i64>,
    pub kinds: Option<&'a [&'a str]>,
    pub input_modes: Option<&'a [&'a str]>,
    pub entry_ids: Option<&'a [String]>,
}

/// Entries in sequence order, with edits folded into player actions and narration.
/// `content_edited` rows are never returned separately.
pub fn select(
    conn: &rusqlite::Connection,
    story_id: &str,
    query: &TranscriptQuery<'_>,
) -> AppResult<Vec<TranscriptEntry>> {
    let raw = match query.since_seq {
        Some(seq) => repository::list_logical_entries_since(conn, story_id, seq)?,
        None => repository::list_logical_entries(conn, story_id)?,
    };
    let active = reducer::active_visible_entries(&raw)
        .into_iter()
        .map(|entry| (entry.id.clone(), entry))
        .collect::<HashMap<_, _>>();
    let ids = query
        .entry_ids
        .map(|ids| ids.iter().collect::<HashSet<_>>());
    Ok(raw
        .into_iter()
        .filter(|entry| entry.kind != kind::CONTENT_EDITED)
        .map(|entry| active.get(&entry.id).cloned().unwrap_or(entry))
        .filter(|entry| {
            query
                .kinds
                .is_none_or(|kinds| kinds.contains(&entry.kind.as_str()))
                && (entry.kind != kind::PLAYER_MESSAGE
                    || query.input_modes.is_none_or(|modes| {
                        entry
                            .payload
                            .get("input_mode")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|mode| modes.contains(&mode))
                    }))
                && ids.as_ref().is_none_or(|ids| ids.contains(&entry.id))
        })
        .collect())
}

/// Entity ids named by entity events with `seq >= since_seq`.
pub fn entities_touched_since(
    conn: &rusqlite::Connection,
    story_id: &str,
    since_seq: i64,
) -> AppResult<HashSet<String>> {
    let mut stmt = conn.prepare(
        "SELECT payload_json FROM transcript_entries
         WHERE story_id = ?1 AND seq >= ?2
            AND kind IN (?3, ?4, ?5, ?6, ?7)",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![
            story_id,
            since_seq,
            kind::ENTITY_CREATED,
            kind::ENTITY_UPDATED,
            kind::ENTITY_DELETED,
            kind::ENTITY_ATTRIBUTE_CHANGED,
            kind::ENTITY_ATTRIBUTE_REMOVED,
        ],
        |row| row.get::<_, String>(0),
    )?;
    let mut touched = HashSet::new();
    for row in rows {
        let payload = serde_json::from_str::<serde_json::Value>(&row?)
            .map_err(|error| AppError::Other(format!("invalid transcript payload JSON: {error}")))?;
        if let Some(id) = payload.get("entity_id").and_then(serde_json::Value::as_str) {
            touched.insert(id.to_string());
        }
        if let Some(ids) = payload
            .get("entity_ids")
            .and_then(serde_json::Value::as_array)
        {
            touched.extend(ids.iter().filter_map(|id| id.as_str().map(str::to_string)));
        }
    }
    Ok(touched)
}

/// The latest active narration entry id, if any.
pub fn latest_narration_id(
    conn: &rusqlite::Connection,
    story_id: &str,
) -> AppResult<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT id FROM transcript_entries WHERE story_id = ?1 AND kind = ?2
             ORDER BY seq DESC LIMIT 1",
            rusqlite::params![story_id, kind::NARRATION],
            |row| row.get(0),
        )
        .optional()?)
}

/// The entries of one turn, in sequence order.
pub fn entries_of_turn(conn: &rusqlite::Connection, turn_id: &str) -> AppResult<Vec<TranscriptEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, story_id, seq, kind, visibility, content, payload_json,
                target_entry_id, turn_id, created_at
         FROM transcript_entries WHERE turn_id = ?1 ORDER BY seq ASC",
    )?;
    let rows = stmt.query_map([turn_id], repository::row_to_entry)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::test_support;
    use serde_json::json;

    fn story() -> crate::shared::db::Pool {
        let pool = crate::shared::db::test_pool();
        test_support::story(&pool.get().unwrap(), "story");
        pool
    }

    fn append(
        conn: &rusqlite::Connection,
        kind: &str,
        content: Option<&str>,
        payload: serde_json::Value,
        target: Option<&str>,
    ) -> TranscriptEntry {
        let id = test_support::record(conn, "story", kind, content, payload, target, None);
        repository::get_entry(conn, &id).unwrap()
    }

    #[test]
    fn edits_fold_into_actions_and_narration_without_returning_edit_rows() {
        let pool = story();
        let conn = pool.get().unwrap();
        let action = append(
            &conn,
            kind::PLAYER_MESSAGE,
            Some("original"),
            json!({"input_mode":"do"}),
            None,
        );
        let narration = append(&conn, kind::NARRATION, Some("first"), json!({}), None);
        append(
            &conn,
            kind::CONTENT_EDITED,
            Some("edited action"),
            json!({}),
            Some(&action.id),
        );
        append(
            &conn,
            kind::CONTENT_EDITED,
            Some("edited narration"),
            json!({}),
            Some(&narration.id),
        );
        let rows = select(&conn, "story", &TranscriptQuery::default()).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].content.as_deref(), Some("edited action"));
        assert_eq!(rows[1].content.as_deref(), Some("edited narration"));
    }

    #[test]
    fn kinds_filter_keeps_only_requested_records() {
        let pool = story();
        let conn = pool.get().unwrap();
        append(
            &conn,
            kind::PLAYER_MESSAGE,
            Some("go"),
            json!({"input_mode":"do"}),
            None,
        );
        let roll = append(&conn, kind::DICEROLL, None, json!({"roll":5}), None);
        let rows = select(
            &conn,
            "story",
            &TranscriptQuery {
                kinds: Some(&[kind::DICEROLL]),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            vec![roll.id.as_str()]
        );
    }

    #[test]
    fn input_modes_filter_applies_only_to_player_messages() {
        let pool = story();
        let conn = pool.get().unwrap();
        append(
            &conn,
            kind::PLAYER_MESSAGE,
            Some("hello"),
            json!({"input_mode":"say"}),
            None,
        );
        let action = append(
            &conn,
            kind::PLAYER_MESSAGE,
            Some("go"),
            json!({"input_mode":"do"}),
            None,
        );
        let narration = append(&conn, kind::NARRATION, Some("scene"), json!({}), None);
        let rows = select(
            &conn,
            "story",
            &TranscriptQuery {
                input_modes: Some(&["do"]),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            vec![action.id.as_str(), narration.id.as_str()]
        );
    }

    #[test]
    fn since_seq_includes_the_boundary() {
        let pool = story();
        let conn = pool.get().unwrap();
        append(&conn, kind::DICEROLL, None, json!({}), None);
        let boundary = append(
            &conn,
            kind::PLAYER_MESSAGE,
            Some("go"),
            json!({"input_mode":"do"}),
            None,
        );
        let later = append(&conn, kind::NARRATION, Some("scene"), json!({}), None);
        let rows = select(
            &conn,
            "story",
            &TranscriptQuery {
                since_seq: Some(boundary.seq),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            vec![boundary.id.as_str(), later.id.as_str()]
        );
    }

    #[test]
    fn entry_ids_filter_keeps_requested_entries_in_sequence_order() {
        let pool = story();
        let conn = pool.get().unwrap();
        let first = append(&conn, kind::DICEROLL, None, json!({}), None);
        append(&conn, kind::DICEROLL, None, json!({}), None);
        let last = append(&conn, kind::DICEROLL, None, json!({}), None);
        let ids = vec![last.id.clone(), first.id.clone()];
        let rows = select(
            &conn,
            "story",
            &TranscriptQuery {
                entry_ids: Some(&ids),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            vec![first.id.as_str(), last.id.as_str()]
        );
    }

    #[test]
    fn touched_entities_reads_single_and_multiple_ids_from_the_raw_tail() {
        let pool = story();
        let conn = pool.get().unwrap();
        append(
            &conn,
            kind::ENTITY_CREATED,
            None,
            json!({"entity_id":"before"}),
            None,
        );
        let boundary = append(
            &conn,
            kind::ENTITY_UPDATED,
            None,
            json!({"entity_id":"one"}),
            None,
        );
        append(
            &conn,
            kind::ENTITY_UPDATED,
            None,
            json!({"entity_ids":["two","three"]}),
            None,
        );
        let touched = entities_touched_since(&conn, "story", boundary.seq).unwrap();
        assert_eq!(
            touched,
            ["one", "two", "three"]
                .into_iter()
                .map(str::to_string)
                .collect()
        );
    }
}
