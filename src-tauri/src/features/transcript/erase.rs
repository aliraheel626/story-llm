use std::collections::HashSet;

use super::{
    attachments,
    model::{kind as transcript_kind, TranscriptEntry},
    query, summaries, turns,
};
use crate::shared::error::AppResult;
use chrono::Utc;

pub(crate) struct RemovedTurn {
    pub visible_ids: Vec<String>,
    pub entries: Vec<TranscriptEntry>,
}

pub(super) fn delete_turn_assets(
    conn: &rusqlite::Connection,
    entries: &[TranscriptEntry],
) -> AppResult<()> {
    let mut asset_ids = HashSet::new();
    for entry in entries {
        asset_ids.extend(attachments::image_ids_for_entry(conn, &entry.id)?);
        if entry.kind == transcript_kind::IMAGE_GENERATED {
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
        attachments::delete_asset_by_id(conn, &asset_id)?;
    }
    Ok(())
}

pub(crate) fn remove_turn(
    conn: &rusqlite::Connection,
    story_id: &str,
    turn_id: &str,
) -> AppResult<RemovedTurn> {
    let entries = query::entries_of_turn(conn, turn_id)?;
    let visible_ids = entries
        .iter()
        .filter(|entry| entry.visibility == "visible")
        .map(|entry| entry.id.clone())
        .collect::<Vec<_>>();
    delete_turn_assets(conn, &entries)?;
    let doomed_ids = entries
        .iter()
        .map(|entry| entry.id.clone())
        .collect::<HashSet<_>>();
    summaries::prune_covering(conn, story_id, &doomed_ids)?;
    conn.execute(
        "DELETE FROM turns WHERE id = ?1 AND story_id = ?2",
        rusqlite::params![turn_id, story_id],
    )?;
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE stories SET updated_at = ?1 WHERE id = ?2",
        rusqlite::params![now, story_id],
    )?;

    Ok(RemovedTurn {
        visible_ids,
        entries,
    })
}

pub(crate) fn erase_last_exchange_in_tx(
    tx: &rusqlite::Transaction<'_>,
    story_id: &str,
) -> AppResult<Option<RemovedTurn>> {
    let Some(last_turn) = turns::last_turn(tx, story_id)? else {
        return Ok(None);
    };
    remove_turn(tx, story_id, &last_turn.id).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::entities;
    use crate::features::transcript::repository::{self as transcript_repository, append_entry};
    use crate::shared::db::with_transaction;
    use crate::shared::test_support;
    use serde_json::json;

    #[test]
    fn replay_failure_rolls_back_turn_assets_summaries_and_timestamp() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute("INSERT INTO stories (id, title, created_at, updated_at) VALUES ('s', 'story', 'now', 'before')", []).unwrap();
        let turn_id = turns::create_turn(&conn, "s").unwrap();
        let entry = append_entry(
            &conn,
            "s",
            transcript_kind::NARRATION,
            "visible",
            Some("scene"),
            &json!({}),
            None,
            Some(&turn_id),
        )
        .unwrap();
        conn.execute("INSERT INTO image_assets (id, entry_id, path, prompt, created_at) VALUES ('image', ?1, '', '', 'now')", [&entry.id]).unwrap();
        let summary = append_entry(
            &conn,
            "s",
            transcript_kind::CONTEXT_SUMMARY,
            "hidden",
            Some("summary"),
            &json!({"through_entry_id":entry.id}),
            None,
            None,
        )
        .unwrap();
        let updated_at: String = conn
            .query_row("SELECT updated_at FROM stories WHERE id = 's'", [], |row| {
                row.get(0)
            })
            .unwrap();
        drop(conn);

        assert!(matches!(
            with_transaction(&pool, |tx| {
                let removed = erase_last_exchange_in_tx(tx, "s")?.unwrap();
                assert_eq!(removed.entries.len(), 1);
                assert_eq!(
                    tx.query_row("SELECT COUNT(*) FROM turns", [], |row| row.get::<_, i64>(0))?,
                    0
                );
                Err::<(), _>(crate::shared::error::AppError::Other("replay failed".into()))
            }),
            Err(crate::shared::error::AppError::Other(message)) if message == "replay failed"
        ));
        let conn = pool.get().unwrap();
        assert!(turns::last_turn(&conn, "s").unwrap().is_some());
        assert!(transcript_repository::get_entry(&conn, &entry.id).is_ok());
        assert!(transcript_repository::get_entry(&conn, &summary.id).is_ok());
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM image_assets", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row("SELECT updated_at FROM stories WHERE id = 's'", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap(),
            updated_at
        );
    }

    #[test]
    fn erase_removes_the_action_and_generated_response_for_every_mode() {
        for mode in ["do", "say", "story", "guide", "continue", "see"] {
            let pool = crate::shared::db::test_pool();
            let conn = pool.get().unwrap();
            test_support::story(&conn, "s");
            let (_, action_id, response_id) = test_support::exchange(
                &conn,
                "s",
                mode,
                if matches!(mode, "continue" | "see") {
                    ""
                } else {
                    "action"
                },
                Some("response"),
            );
            let action = transcript_repository::get_entry(&conn, &action_id).unwrap();
            let response = transcript_repository::get_entry(&conn, &response_id.unwrap()).unwrap();
            drop(conn);

            let removed = with_transaction(&pool, |tx| {
                let removed = erase_last_exchange_in_tx(tx, "s")?.unwrap();
                entities::projection::replay_after_erase(tx, "s", &removed.entries)?;
                Ok(removed.visible_ids)
            })
            .unwrap();
            assert_eq!(removed, vec![action.id, response.id], "mode {mode}");
        }
    }

    #[test]
    fn erase_trailing_see_removes_its_image_from_the_prior_narration() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        test_support::story(&conn, "s");
        let narration_turn = turns::create_turn(&conn, "s").unwrap();
        let narration = append_entry(
            &conn,
            "s",
            transcript_kind::NARRATION,
            "visible",
            Some("A moonlit harbor."),
            &json!({"input_mode":"generated"}),
            None,
            Some(&narration_turn),
        )
        .unwrap();
        let see_turn = turns::create_turn(&conn, "s").unwrap();
        let see = append_entry(
            &conn,
            "s",
            transcript_kind::PLAYER_MESSAGE,
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
            transcript_kind::IMAGE_GENERATED,
            "hidden",
            Some("image"),
            &json!({"asset_id":"image"}),
            Some(&see.id),
            Some(&see_turn),
        )
        .unwrap();
        drop(conn);

        let removed = with_transaction(&pool, |tx| {
            let removed = erase_last_exchange_in_tx(tx, "s")?.unwrap();
            entities::projection::replay_after_erase(tx, "s", &removed.entries)?;
            Ok(removed.visible_ids)
        })
        .unwrap();
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
                "SELECT COUNT(*) FROM transcript_entries WHERE id = ?1",
                [&image_event.id],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM transcript_entries WHERE id = ?1",
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
        test_support::story(&conn, "s");
        let baseline_turn = turns::create_turn(&conn, "s").unwrap();
        let baseline = append_entry(
            &conn,
            "s",
            transcript_kind::NARRATION,
            "visible",
            Some("Earlier scene"),
            &json!({"input_mode":"generated"}),
            None,
            Some(&baseline_turn),
        )
        .unwrap();
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
        let unrelated_projection: (String, Option<String>, i64, String) = conn
            .query_row(
                "SELECT e.name, c.appearance_anchor, e.is_present, e.updated_at
                 FROM entities e JOIN characters c ON c.entity_id=e.id
                 WHERE e.story_id = 's' AND e.id = 'unrelated'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                    ))
                },
            )
            .unwrap();
        let older_summary = append_entry(
            &conn,
            "s",
            transcript_kind::CONTEXT_SUMMARY,
            "hidden",
            Some("older summary"),
            &json!({"through_entry_id":baseline.id}),
            None,
            None,
        )
        .unwrap();

        let (turn_id, player_id, narration_id) =
            test_support::exchange(&conn, "s", "do", "act", Some("result"));
        let player = transcript_repository::get_entry(&conn, &player_id).unwrap();
        let narration = transcript_repository::get_entry(&conn, &narration_id.unwrap()).unwrap();
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
            transcript_kind::ENTITY_UPDATED,
            "hidden",
            Some("Looked up Mira"),
            &json!({"entity_ids":["mira"]}),
            Some(&narration.id),
            Some(&turn_id),
        )
        .unwrap();
        for kind in [
            transcript_kind::DICEROLL,
            transcript_kind::CONTENT_EDITED,
            transcript_kind::IMAGE_GENERATED,
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
            transcript_kind::CONTEXT_SUMMARY,
            "hidden",
            Some("summary"),
            &json!({"through_entry_id":query.id}),
            None,
            None,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO image_assets (id, entry_id, path, prompt, created_at)
             VALUES ('image', ?1, 'C:/tmp/image.png', 'prompt', 'now')",
            [&narration.id],
        )
        .unwrap();
        drop(conn);

        let removed = with_transaction(&pool, |tx| {
            let removed = erase_last_exchange_in_tx(tx, "s")?.unwrap();
            entities::projection::replay_after_erase(tx, "s", &removed.entries)?;
            Ok(removed.visible_ids)
        })
        .unwrap();
        assert_eq!(removed, vec![player.id, narration.id]);
        let conn = pool.get().unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM transcript_entries WHERE id = ?1",
                [&doomed_summary.id],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM transcript_entries WHERE id = ?1",
                [&older_summary.id],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        let mira: (String, Option<String>) = conn
            .query_row(
                "SELECT e.name, c.appearance_anchor FROM entities e JOIN characters c ON c.entity_id=e.id
                 WHERE e.story_id = 's' AND e.id = 'mira'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            mira,
            (
                "Mira".into(),
                Some("silver hair".into()),
            )
        );
        let restored_attribute: (f64, String) = conn
            .query_row(
                "SELECT value, source FROM entity_attributes JOIN entities ON entities.id = entity_attributes.entity_id WHERE entities.story_id = 's' AND entity_id = 'mira' AND attribute_id = ?1",
                [&trust_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            restored_attribute,
            (2.0, "inferred".into())
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM entities WHERE story_id = 's' AND id = 'temporary'",
                [],
                |row| row.get::<_, i64>(0)
            )
                .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT e.name, c.appearance_anchor, e.is_present, e.updated_at
                 FROM entities e JOIN characters c ON c.entity_id=e.id
                 WHERE e.story_id = 's' AND e.id = 'unrelated'",
                [],
                |row| Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?
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
        test_support::story(&conn, "s");
        let (turn_id, action_id, narration_id) =
            test_support::exchange(&conn, "s", "do", "act", Some("result"));
        let action = transcript_repository::get_entry(&conn, &action_id).unwrap();
        let narration = transcript_repository::get_entry(&conn, &narration_id.unwrap()).unwrap();
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

        let removed = with_transaction(&pool, |tx| {
            let removed = erase_last_exchange_in_tx(tx, "s")?.unwrap();
            entities::projection::replay_after_erase(tx, "s", &removed.entries)?;
            Ok(removed.visible_ids)
        })
        .unwrap();
        assert_eq!(removed, vec![action.id, narration.id]);

        let conn = pool.get().unwrap();
        let counts: (i64, i64, i64) = conn
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM entities WHERE id = 'temporary'),
                    (SELECT COUNT(*) FROM characters WHERE entity_id = 'temporary'),
                    (SELECT COUNT(*) FROM entity_attributes WHERE entity_id = 'temporary')",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(counts, (0, 0, 0));
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM transcript_entries WHERE kind = ?1",
                [transcript_kind::ENTITY_ATTRIBUTE_CHANGED],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            1
        );
    }

    #[test]
    fn erase_removes_legacy_failed_turn_and_entries() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        test_support::story(&conn, "s");
        let (turn_id, action_id, _) = test_support::exchange(&conn, "s", "do", "act", None);
        let action = transcript_repository::get_entry(&conn, &action_id).unwrap();
        conn.execute(
            "UPDATE turns SET status = 'failed' WHERE id = ?1",
            [&turn_id],
        )
        .unwrap();
        drop(conn);

        let removed = with_transaction(&pool, |tx| {
            let removed = erase_last_exchange_in_tx(tx, "s")?.unwrap();
            entities::projection::replay_after_erase(tx, "s", &removed.entries)?;
            Ok(removed.visible_ids)
        })
        .unwrap();
        assert_eq!(removed, vec![action.id.clone()]);
        assert!(transcript_repository::get_entry(&pool.get().unwrap(), &action.id).is_err());
    }
}
