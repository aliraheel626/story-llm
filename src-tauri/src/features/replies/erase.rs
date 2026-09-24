use std::collections::HashSet;

use crate::features::{
    compaction, entities, images,
    ledger::{
        model::{kind as ledger_kind, LedgerEntry},
        repository as ledger_repository, turns,
    },
};
use crate::shared::db::{with_transaction, Pool};
use crate::shared::error::{AppError, AppResult};
use chrono::Utc;

pub(super) fn entries_for_turn(
    conn: &rusqlite::Connection,
    turn_id: &str,
) -> AppResult<Vec<LedgerEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, story_id, seq, kind, visibility, content, payload_json,
                target_entry_id, turn_id, created_at
         FROM ledger_entries WHERE turn_id = ?1 ORDER BY seq ASC",
    )?;
    let rows = stmt.query_map([turn_id], ledger_repository::row_to_entry)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub(super) fn touched_entities(entries: &[LedgerEntry]) -> HashSet<String> {
    entries
        .iter()
        .filter_map(entities::events::EntityEvent::from_entry)
        .map(|event| event.entity_id().to_string())
        .collect()
}

pub(super) fn delete_turn_assets(
    conn: &rusqlite::Connection,
    entries: &[LedgerEntry],
) -> AppResult<()> {
    let mut asset_ids = HashSet::new();
    for entry in entries {
        let mut stmt = conn.prepare("SELECT id FROM image_assets WHERE entry_id = ?1")?;
        let rows = stmt.query_map([&entry.id], |row| row.get::<_, String>(0))?;
        asset_ids.extend(rows.collect::<Result<Vec<_>, _>>()?);
        if entry.kind == ledger_kind::IMAGE_GENERATED {
            if let Some(asset_id) = entry
                .payload
                .get("asset_id")
                .and_then(|value| value.as_str())
            {
                asset_ids.insert(asset_id.to_string());
            }
        }
    }
    for asset_id in asset_ids {
        images::delete_asset_by_id(conn, &asset_id)?;
    }
    Ok(())
}

pub(super) fn remove_turn(
    conn: &rusqlite::Connection,
    story_id: &str,
    turn_id: &str,
) -> AppResult<Vec<String>> {
    let entries = entries_for_turn(conn, turn_id)?;
    let removed = entries
        .iter()
        .filter(|entry| entry.visibility == "visible")
        .map(|entry| entry.id.clone())
        .collect::<Vec<_>>();
    delete_turn_assets(conn, &entries)?;
    let doomed_ids = entries
        .iter()
        .map(|entry| entry.id.clone())
        .collect::<HashSet<_>>();
    let affected_entities = touched_entities(&entries);
    compaction::prune_summaries_covering(conn, story_id, &doomed_ids)?;
    conn.execute(
        "DELETE FROM turns WHERE id = ?1 AND story_id = ?2",
        rusqlite::params![turn_id, story_id],
    )?;
    let now = Utc::now().to_rfc3339();
    entities::projection::replay(conn, story_id, &affected_entities, None)?;
    conn.execute(
        "UPDATE stories SET updated_at = ?1 WHERE id = ?2",
        rusqlite::params![now, story_id],
    )?;

    Ok(removed)
}

fn erase_last_exchange_in_tx(
    tx: &rusqlite::Transaction<'_>,
    story_id: &str,
) -> AppResult<Vec<String>> {
    let Some(last_turn) = turns::last_turn(tx, story_id)? else {
        return Ok(vec![]);
    };
    if last_turn.status == turns::PENDING {
        return Err(AppError::Invalid(
            "cannot erase a turn while it is generating".into(),
        ));
    }
    remove_turn(tx, story_id, &last_turn.id)
}

/// "Erase": removes the most recent exchange — the latest narration plus the
/// player message (or story draft) that triggered it. Returns the IDs removed
/// so the frontend can splice locally.
pub(super) fn erase_last_exchange(pool: &Pool, story_id: String) -> AppResult<Vec<String>> {
    with_transaction(pool, |tx| erase_last_exchange_in_tx(tx, &story_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::ledger::repository::append_entry;
    use serde_json::json;
    #[test]
    fn erase_removes_the_action_and_generated_response_for_every_mode() {
        for mode in ["do", "say", "story", "guide", "continue", "see"] {
            let pool = crate::shared::db::test_pool();
            let conn = pool.get().unwrap();
            conn.execute(
                "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
                 VALUES ('s', 'story', 'now', 'now', '{}')",
                [],
            )
            .unwrap();
            let turn_id = turns::create_turn(&conn, "s").unwrap();
            let action = append_entry(
                &conn,
                "s",
                ledger_kind::PLAYER_MESSAGE,
                "visible",
                Some(if matches!(mode, "continue" | "see") {
                    ""
                } else {
                    "action"
                }),
                &json!({"input_mode":mode}),
                None,
                Some(&turn_id),
            )
            .unwrap();
            let response = append_entry(
                &conn,
                "s",
                ledger_kind::NARRATION,
                "visible",
                Some("response"),
                &json!({"input_mode":"generated"}),
                None,
                Some(&turn_id),
            )
            .unwrap();
            turns::set_status(&conn, &turn_id, turns::COMPLETE).unwrap();
            drop(conn);

            let removed =
                with_transaction(&pool, |tx| erase_last_exchange_in_tx(tx, "s")).unwrap();
            assert_eq!(removed, vec![action.id, response.id], "mode {mode}");
        }
    }

    #[test]
    fn erase_trailing_see_removes_its_image_from_the_prior_narration() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
             VALUES ('s', 'story', 'now', 'now', '{}')",
            [],
        )
        .unwrap();
        let narration_turn = turns::create_turn(&conn, "s").unwrap();
        let narration = append_entry(
            &conn,
            "s",
            ledger_kind::NARRATION,
            "visible",
            Some("A moonlit harbor."),
            &json!({"input_mode":"generated"}),
            None,
            Some(&narration_turn),
        )
        .unwrap();
        turns::set_status(&conn, &narration_turn, turns::COMPLETE).unwrap();
        let see_turn = turns::create_turn(&conn, "s").unwrap();
        let see = append_entry(
            &conn,
            "s",
            ledger_kind::PLAYER_MESSAGE,
            "visible",
            Some(""),
            &json!({"input_mode":"see"}),
            None,
            Some(&see_turn),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO image_assets (id, entry_id, path, prompt, created_at)
             VALUES ('image', ?1, 'C:/tmp/see.png', 'prompt', 'now')",
            [&narration.id],
        )
        .unwrap();
        let image_event = append_entry(
            &conn,
            "s",
            ledger_kind::IMAGE_GENERATED,
            "hidden",
            Some("image"),
            &json!({"asset_id":"image"}),
            Some(&see.id),
            Some(&see_turn),
        )
        .unwrap();
        turns::set_status(&conn, &see_turn, turns::COMPLETE).unwrap();
        drop(conn);

        let removed =
            with_transaction(&pool, |tx| erase_last_exchange_in_tx(tx, "s")).unwrap();
        assert_eq!(removed, vec![see.id]);
        let conn = pool.get().unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM image_assets", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM ledger_entries WHERE id = ?1",
                [&image_event.id],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM ledger_entries WHERE id = ?1",
                [&narration.id],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
    }

    #[test]
    fn hard_erase_removes_exchange_derivatives_summaries_and_projections() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
             VALUES ('s', 'story', 'now', 'now', '{}')",
            [],
        )
        .unwrap();
        let baseline_turn = turns::create_turn(&conn, "s").unwrap();
        let baseline = append_entry(
            &conn,
            "s",
            ledger_kind::NARRATION,
            "visible",
            Some("Earlier scene"),
            &json!({"input_mode":"generated"}),
            None,
            Some(&baseline_turn),
        )
        .unwrap();
        turns::set_status(&conn, &baseline_turn, turns::COMPLETE).unwrap();
        crate::features::entities::create_entity_with_id_sync(
            &conn,
            "mira",
            "s",
            "character",
            "Mira",
            Some("silver hair"),
            "test",
            None,
            None,
        )
        .unwrap();
        crate::features::entities::create_entity_with_id_sync(
            &conn,
            "unrelated",
            "s",
            "character",
            "Tomas",
            None,
            "test",
            None,
            None,
        )
        .unwrap();
        let trust_id: String = conn
            .query_row(
                "SELECT id FROM attribute_registry WHERE canonical_name = 'Trust'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let trust =
            crate::features::entities::attributes::find_attribute_by_id(&conn, &trust_id).unwrap();
        crate::features::entities::attributes::apply_attribute_delta(
            &conn,
            "s",
            "mira",
            &trust,
            2.0,
            "earlier event",
            &baseline.id,
            false,
            None,
        )
        .unwrap();
        let mira_last_event_id: String = conn
            .query_row(
                "SELECT last_event_id FROM story_entity_state WHERE story_id = 's' AND entity_id = 'mira'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let attribute_last_event_id: String = conn
            .query_row(
                "SELECT last_event_id FROM entity_attributes WHERE story_id = 's' AND entity_id = 'mira' AND attribute_id = ?1",
                [&trust_id],
                |row| row.get(0),
            )
            .unwrap();
        let unrelated_projection: (String, Option<String>, i64, String, String) = conn
            .query_row(
                "SELECT name, appearance_anchor, is_present, updated_at, last_event_id
                 FROM story_entity_state WHERE story_id = 's' AND entity_id = 'unrelated'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        let older_summary = append_entry(
            &conn,
            "s",
            ledger_kind::CONTEXT_SUMMARY,
            "hidden",
            Some("older summary"),
            &json!({"through_entry_id":baseline.id}),
            None,
            None,
        )
        .unwrap();

        let turn_id = turns::create_turn(&conn, "s").unwrap();
        let player = append_entry(
            &conn,
            "s",
            ledger_kind::PLAYER_MESSAGE,
            "visible",
            Some("act"),
            &json!({"input_mode":"do"}),
            None,
            Some(&turn_id),
        )
        .unwrap();
        let narration = append_entry(
            &conn,
            "s",
            ledger_kind::NARRATION,
            "visible",
            Some("result"),
            &json!({"input_mode":"generated"}),
            None,
            Some(&turn_id),
        )
        .unwrap();
        crate::features::entities::update_entity_sync(
            &conn,
            "s",
            "mira",
            "Mira Changed",
            Some("black armor"),
            "narrator_tool",
            Some(&narration.id),
            Some(&turn_id),
        )
        .unwrap();
        crate::features::entities::attributes::apply_attribute_delta(
            &conn,
            "s",
            "mira",
            &trust,
            3.0,
            "latest event",
            &narration.id,
            false,
            Some(&turn_id),
        )
        .unwrap();
        crate::features::entities::create_entity_with_id_sync(
            &conn,
            "temporary",
            "s",
            "character",
            "Temporary",
            None,
            "narrator_tool",
            Some(&narration.id),
            Some(&turn_id),
        )
        .unwrap();
        let query = append_entry(
            &conn,
            "s",
            ledger_kind::ENTITY_QUERIED,
            "hidden",
            Some("Looked up Mira"),
            &json!({"entity_ids":["mira"]}),
            Some(&narration.id),
            Some(&turn_id),
        )
        .unwrap();
        for kind in [
            ledger_kind::DICEROLL,
            ledger_kind::CONTENT_EDITED,
            ledger_kind::IMAGE_GENERATED,
        ] {
            append_entry(
                &conn,
                "s",
                kind,
                "hidden",
                Some("derivative"),
                &json!({}),
                Some(&narration.id),
                Some(&turn_id),
            )
            .unwrap();
        }
        let doomed_summary = append_entry(
            &conn,
            "s",
            ledger_kind::CONTEXT_SUMMARY,
            "hidden",
            Some("summary"),
            &json!({"through_entry_id":query.id}),
            None,
            None,
        )
        .unwrap();
        turns::set_status(&conn, &turn_id, turns::COMPLETE).unwrap();
        conn.execute(
            "INSERT INTO image_assets (id, entry_id, path, prompt, created_at)
             VALUES ('image', ?1, 'C:/tmp/image.png', 'prompt', 'now')",
            [&narration.id],
        )
        .unwrap();
        drop(conn);

        let removed =
            with_transaction(&pool, |tx| erase_last_exchange_in_tx(tx, "s")).unwrap();
        assert_eq!(removed, vec![player.id, narration.id]);
        let conn = pool.get().unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM ledger_entries WHERE id = ?1",
                [&doomed_summary.id],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM ledger_entries WHERE id = ?1",
                [&older_summary.id],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        let mira: (String, Option<String>, String) = conn
            .query_row(
                "SELECT name, appearance_anchor, last_event_id FROM story_entity_state WHERE story_id = 's' AND entity_id = 'mira'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            mira,
            (
                "Mira".into(),
                Some("silver hair".into()),
                mira_last_event_id
            )
        );
        let restored_attribute: (f64, String, String) = conn
            .query_row(
                "SELECT value, source, last_event_id FROM entity_attributes WHERE story_id = 's' AND entity_id = 'mira' AND attribute_id = ?1",
                [&trust_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            restored_attribute,
            (2.0, "inferred".into(), attribute_last_event_id)
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM story_entity_state WHERE story_id = 's' AND entity_id = 'temporary'",
                [],
                |row| row.get::<_, i64>(0)
            )
                .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT name, appearance_anchor, is_present, updated_at, last_event_id
                 FROM story_entity_state WHERE story_id = 's' AND entity_id = 'unrelated'",
                [],
                |row| Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?
                ))
            )
            .unwrap(),
            unrelated_projection
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM entities WHERE id = 'temporary'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM image_assets", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn erasing_entity_creation_skips_a_later_player_attribute_event() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
             VALUES ('s', 'story', 'now', 'now', '{}')",
            [],
        )
        .unwrap();
        let turn_id = turns::create_turn(&conn, "s").unwrap();
        let action = append_entry(
            &conn,
            "s",
            ledger_kind::PLAYER_MESSAGE,
            "visible",
            Some("act"),
            &json!({"input_mode":"do"}),
            None,
            Some(&turn_id),
        )
        .unwrap();
        let narration = append_entry(
            &conn,
            "s",
            ledger_kind::NARRATION,
            "visible",
            Some("result"),
            &json!({"input_mode":"generated"}),
            None,
            Some(&turn_id),
        )
        .unwrap();
        entities::create_entity_with_id_sync(
            &conn,
            "temporary",
            "s",
            "character",
            "Temporary",
            None,
            "narrator_tool",
            Some(&narration.id),
            Some(&turn_id),
        )
        .unwrap();
        turns::set_status(&conn, &turn_id, turns::COMPLETE).unwrap();
        let health_id: String = conn
            .query_row(
                "SELECT id FROM attribute_registry WHERE canonical_name = 'Health'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        entities::attributes::set_entity_attribute_sync(&conn, "s", "temporary", &health_id, 7.0)
            .unwrap();
        drop(conn);

        let removed =
            with_transaction(&pool, |tx| erase_last_exchange_in_tx(tx, "s")).unwrap();
        assert_eq!(removed, vec![action.id, narration.id]);

        let conn = pool.get().unwrap();
        let counts: (i64, i64, i64) = conn
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM entities WHERE id = 'temporary'),
                    (SELECT COUNT(*) FROM story_entity_state WHERE entity_id = 'temporary'),
                    (SELECT COUNT(*) FROM entity_attributes WHERE entity_id = 'temporary')",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(counts, (0, 0, 0));
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM ledger_entries WHERE kind = ?1",
                [ledger_kind::ENTITY_ATTRIBUTE_CHANGED],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            1
        );
    }

    #[test]
    fn erase_refuses_a_pending_turn_without_deleting_entries() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
             VALUES ('s', 'story', 'now', 'now', '{}')",
            [],
        )
        .unwrap();
        let turn_id = turns::create_turn(&conn, "s").unwrap();
        let action = append_entry(
            &conn,
            "s",
            ledger_kind::PLAYER_MESSAGE,
            "visible",
            Some("act"),
            &json!({"input_mode":"do"}),
            None,
            Some(&turn_id),
        )
        .unwrap();
        drop(conn);

        let error = with_transaction(&pool, |tx| erase_last_exchange_in_tx(tx, "s")).unwrap_err();
        assert!(matches!(
            error,
            AppError::Invalid(message)
                if message == "cannot erase a turn while it is generating"
        ));
        assert!(ledger_repository::get_entry(&pool.get().unwrap(), &action.id).is_ok());
    }
}
