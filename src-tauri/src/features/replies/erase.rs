use std::collections::HashSet;

use crate::features::{
    compaction, images,
    ledger::{model::kind as ledger_kind, repository as ledger_repository},
};
use crate::shared::db::{with_transaction, Pool};
use crate::shared::error::{AppError, AppResult};
use chrono::Utc;

fn collect_cascade_effects(
    conn: &rusqlite::Connection,
    root_id: &str,
    doomed_ids: &mut HashSet<String>,
    affected_entities: &mut HashSet<String>,
    image_paths: &mut Vec<String>,
) -> AppResult<()> {
    for (id, kind, payload_json) in
        crate::features::ledger::cascade::cascade_entries(conn, root_id)?
    {
        doomed_ids.insert(id);
        if kind == ledger_kind::IMAGE_GENERATED {
            if let Some(asset_id) = serde_json::from_str::<serde_json::Value>(&payload_json)
                .ok()
                .and_then(|payload| {
                    payload
                        .get("asset_id")
                        .and_then(|value| value.as_str())
                        .map(str::to_string)
                })
            {
                if let Some(path) = images::delete_asset_by_id(conn, &asset_id)? {
                    if !image_paths.contains(&path) {
                        image_paths.push(path);
                    }
                }
            }
        } else if matches!(
            kind.as_str(),
            ledger_kind::ENTITY_CREATED
                | ledger_kind::ENTITY_UPDATED
                | ledger_kind::ENTITY_DELETED
                | ledger_kind::ENTITY_ATTRIBUTE_CHANGED
                | ledger_kind::ENTITY_ATTRIBUTE_REMOVED
        ) {
            if let Some(entity_id) = serde_json::from_str::<serde_json::Value>(&payload_json)
                .ok()
                .and_then(|payload| {
                    payload
                        .get("entity_id")
                        .and_then(|value| value.as_str())
                        .map(str::to_string)
                })
            {
                affected_entities.insert(entity_id);
            }
        }
    }
    Ok(())
}

pub(super) fn remove_reply_in_tx(
    tx: &rusqlite::Transaction<'_>,
    story_id: &str,
    reply_id: &str,
) -> AppResult<Vec<String>> {
    let mut image_paths = images::image_paths_for_entry(tx, reply_id)?;
    let mut doomed_ids = HashSet::new();
    let mut affected_entities = HashSet::new();
    collect_cascade_effects(
        tx,
        reply_id,
        &mut doomed_ids,
        &mut affected_entities,
        &mut image_paths,
    )?;
    let deleted = tx.execute(
        "DELETE FROM ledger_entries WHERE id = ?1 AND story_id = ?2 AND kind = ?3",
        rusqlite::params![reply_id, story_id, ledger_kind::NARRATION],
    )?;
    if deleted != 1 {
        return Err(AppError::Invalid(
            "narration to replace no longer exists".into(),
        ));
    }
    compaction::prune_summaries_covering(tx, story_id, &doomed_ids)?;
    crate::features::ledger::projections::replay_entities(tx, story_id, &affected_entities)?;
    Ok(image_paths)
}

fn erase_last_exchange_in_tx(
    tx: &rusqlite::Transaction<'_>,
    story_id: &str,
) -> AppResult<(Vec<String>, Vec<String>)> {
    let Some(last) = ledger_repository::last_active_entry(tx, story_id)? else {
        return Ok((vec![], vec![]));
    };
    let mut removed = vec![last.id.clone()];
    let mut image_paths = Vec::new();
    let mut doomed_ids = HashSet::new();
    let mut affected_entities = HashSet::new();
    if last.role() == "narrator" {
        image_paths = remove_reply_in_tx(tx, story_id, &last.id)?;
    } else {
        image_paths.extend(images::image_paths_for_entry(tx, &last.id)?);
        collect_cascade_effects(
            tx,
            &last.id,
            &mut doomed_ids,
            &mut affected_entities,
            &mut image_paths,
        )?;
        tx.execute("DELETE FROM ledger_entries WHERE id = ?1", [&last.id])?;
    }

    let paired = last.role() == "narrator" && last.input_mode() == "generated";
    if paired {
        if let Some(prev) = ledger_repository::last_active_entry(tx, story_id)? {
            if prev.role() == "player" {
                image_paths.extend(images::image_paths_for_entry(tx, &prev.id)?);
                collect_cascade_effects(
                    tx,
                    &prev.id,
                    &mut doomed_ids,
                    &mut affected_entities,
                    &mut image_paths,
                )?;
                removed.push(prev.id.clone());
                tx.execute("DELETE FROM ledger_entries WHERE id = ?1", [&prev.id])?;
            }
        }
    }

    compaction::prune_summaries_covering(tx, story_id, &doomed_ids)?;

    let now = Utc::now().to_rfc3339();
    crate::features::ledger::projections::replay_entities(tx, story_id, &affected_entities)?;
    tx.execute(
        "UPDATE stories SET updated_at = ?1 WHERE id = ?2",
        rusqlite::params![now, story_id],
    )?;

    Ok((removed, image_paths))
}

/// "Erase": removes the most recent exchange — the latest narration plus the
/// player message (or story draft) that triggered it. Returns the IDs removed
/// so the frontend can splice locally.
pub(super) fn erase_last_exchange(pool: &Pool, story_id: String) -> AppResult<Vec<String>> {
    let (removed, image_paths) =
        with_transaction(pool, |tx| erase_last_exchange_in_tx(tx, &story_id))?;

    images::delete_assets(&image_paths);

    Ok(removed)
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
            )
            .unwrap();
            drop(conn);

            let (removed, _) =
                with_transaction(&pool, |tx| erase_last_exchange_in_tx(tx, "s")).unwrap();
            assert_eq!(removed, vec![response.id, action.id], "mode {mode}");
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
        let narration = append_entry(
            &conn,
            "s",
            ledger_kind::NARRATION,
            "visible",
            Some("A moonlit harbor."),
            &json!({"input_mode":"generated"}),
            None,
        )
        .unwrap();
        let see = append_entry(
            &conn,
            "s",
            ledger_kind::PLAYER_MESSAGE,
            "visible",
            Some(""),
            &json!({"input_mode":"see"}),
            None,
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
        )
        .unwrap();
        drop(conn);

        let (removed, paths) =
            with_transaction(&pool, |tx| erase_last_exchange_in_tx(tx, "s")).unwrap();
        assert_eq!(removed, vec![see.id]);
        assert_eq!(paths, vec!["C:/tmp/see.png"]);
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
        let baseline = append_entry(
            &conn,
            "s",
            ledger_kind::NARRATION,
            "visible",
            Some("Earlier scene"),
            &json!({"input_mode":"generated"}),
            None,
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
        )
        .unwrap();

        let player = append_entry(
            &conn,
            "s",
            ledger_kind::PLAYER_MESSAGE,
            "visible",
            Some("act"),
            &json!({"input_mode":"do"}),
            None,
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
        )
        .unwrap();
        conn.execute(
            "INSERT INTO image_assets (id, entry_id, path, prompt, created_at)
             VALUES ('image', ?1, 'C:/tmp/image.png', 'prompt', 'now')",
            [&narration.id],
        )
        .unwrap();
        drop(conn);

        let (removed, paths) =
            with_transaction(&pool, |tx| erase_last_exchange_in_tx(tx, "s")).unwrap();
        assert_eq!(removed, vec![narration.id, player.id]);
        assert_eq!(paths, vec!["C:/tmp/image.png"]);
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
            1
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM image_assets", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}
