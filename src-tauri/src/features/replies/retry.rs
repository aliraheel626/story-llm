use std::{collections::HashMap, path::PathBuf, sync::Arc};

use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::OptionalExtension;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::features::{
    images,
    ledger::{model::LedgerEntry, repository as ledger_repository},
    narrator,
};
use crate::shared::db::{with_transaction, Pool};
use crate::shared::error::{AppError, AppResult};

use super::{
    erase::remove_reply_in_tx,
    model::{NarrationDonePayload, RetryResult},
    submit::start_action_generation,
};
use narrator::{
    staging::TurnStaging, transcript::load_transcript, Candidate, NarratorInputs, NarratorPurpose,
};

struct RetrySnapshot {
    pool: Pool,
    _path: SnapshotPath,
    /// Exact story ledger at snapshot time, including hidden events. A new
    /// image, edit, tool effect, or action must reject the replacement.
    original_entries: Vec<LedgerEntry>,
    original_updated_at: String,
    original_settings_json: String,
    original_registry_aliases: HashMap<String, String>,
}

struct SnapshotPath(PathBuf);

impl Drop for SnapshotPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn story_revision(
    conn: &rusqlite::Connection,
    story_id: &str,
) -> AppResult<(Vec<LedgerEntry>, String, String)> {
    let (updated_at, settings_json) = conn.query_row(
        "SELECT updated_at, settings_json FROM stories WHERE id = ?1",
        [story_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok((
        ledger_repository::list_logical_entries(conn, story_id)?,
        updated_at,
        settings_json,
    ))
}

fn registry_aliases(conn: &rusqlite::Connection) -> AppResult<HashMap<String, String>> {
    let mut stmt = conn.prepare("SELECT id, aliases_json FROM attribute_registry")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    Ok(rows.collect::<Result<HashMap<_, _>, _>>()?)
}

fn snapshot_for_retry(pool: &Pool, story_id: &str, entry_id: &str) -> AppResult<RetrySnapshot> {
    let path = SnapshotPath(
        std::env::temp_dir().join(format!("story-llm-retry-{}.sqlite3", Uuid::new_v4())),
    );
    {
        let conn = pool.get()?;
        conn.execute("VACUUM INTO ?1", [path.0.to_string_lossy().as_ref()])?;
    }
    let manager = SqliteConnectionManager::file(&path.0).with_init(|conn| {
        conn.execute_batch("PRAGMA foreign_keys = ON")?;
        Ok(())
    });
    let snapshot_pool = r2d2::Pool::new(manager)?;
    let (original_entries, original_updated_at, original_settings_json, original_registry_aliases) = {
        let conn = snapshot_pool.get()?;
        let revision = story_revision(&conn, story_id)?;
        let target = ledger_repository::last_active_entry(&conn, story_id)?
            .ok_or_else(|| AppError::Invalid("no narration to retry".into()))?;
        if target.id != entry_id || target.role() != "narrator" {
            return Err(AppError::Invalid(
                "only the latest narration can be retried".into(),
            ));
        }
        (revision.0, revision.1, revision.2, registry_aliases(&conn)?)
    };
    with_transaction(&snapshot_pool, |tx| {
        remove_reply_in_tx(tx, story_id, entry_id)?;
        Ok(())
    })?;
    Ok(RetrySnapshot {
        pool: snapshot_pool,
        _path: path,
        original_entries,
        original_updated_at,
        original_settings_json,
        original_registry_aliases,
    })
}

fn reconcile_attribute_registry(
    tx: &rusqlite::Transaction<'_>,
    scratch: &rusqlite::Connection,
    original_aliases: &HashMap<String, String>,
) -> AppResult<()> {
    let mut stmt = scratch.prepare(
        "SELECT id, canonical_name, aliases_json, entity_kinds_json, min, max, category,
                is_user_created, created_in_story_id, created_at FROM attribute_registry",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, f64>(4)?,
            row.get::<_, f64>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, i64>(7)?,
            row.get::<_, Option<String>>(8)?,
            row.get::<_, String>(9)?,
        ))
    })?;
    for row in rows {
        let (id, name, aliases, kinds, min, max, category, user_created, story, created_at) = row?;
        if original_aliases
            .get(&id)
            .is_some_and(|original| original == &aliases)
        {
            continue;
        }
        let live_aliases: Option<String> = tx
            .query_row(
                "SELECT aliases_json FROM attribute_registry WHERE id = ?1",
                [&id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(live_aliases) = live_aliases {
            if live_aliases != aliases {
                let mut combined: Vec<String> = serde_json::from_str(&live_aliases)
                    .map_err(|e| AppError::Other(format!("invalid attribute aliases: {e}")))?;
                for alias in serde_json::from_str::<Vec<String>>(&aliases)
                    .map_err(|e| AppError::Other(format!("invalid attribute aliases: {e}")))?
                {
                    if !combined
                        .iter()
                        .any(|existing| existing.eq_ignore_ascii_case(&alias))
                    {
                        combined.push(alias);
                    }
                }
                tx.execute(
                    "UPDATE attribute_registry SET aliases_json = ?1 WHERE id = ?2",
                    rusqlite::params![
                        serde_json::to_string(&combined)
                            .map_err(|e| AppError::Other(e.to_string()))?,
                        id
                    ],
                )?;
            }
        } else {
            tx.execute(
                "INSERT INTO attribute_registry
                (id, canonical_name, aliases_json, entity_kinds_json, min, max, category,
                 is_user_created, created_in_story_id, created_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                rusqlite::params![
                    id,
                    name,
                    aliases,
                    kinds,
                    min,
                    max,
                    category,
                    user_created,
                    story,
                    created_at
                ],
            )?;
        }
    }
    Ok(())
}

async fn replace_narration_entry(
    live: &Pool,
    snapshot: &RetrySnapshot,
    story_id: &str,
    target_id: &str,
    visible: &str,
    thoughts: Option<&str>,
    staging: Option<Arc<Mutex<TurnStaging>>>,
) -> AppResult<(LedgerEntry, Vec<String>)> {
    if visible.trim().is_empty() {
        return Err(AppError::Invalid(
            "retry generated no narration; original was preserved".into(),
        ));
    }
    let guard = match &staging {
        Some(staging) => Some(staging.lock().await),
        None => None,
    };
    let scratch = snapshot.pool.get()?;
    with_transaction(live, |tx| {
        let (entries, updated_at, settings_json) = story_revision(tx, story_id)?;
        if entries != snapshot.original_entries
            || updated_at != snapshot.original_updated_at
            || settings_json != snapshot.original_settings_json
            || ledger_repository::last_active_entry(tx, story_id)?
                .as_ref()
                .map(|entry| entry.id.as_str())
                != Some(target_id)
        {
            return Err(AppError::Invalid(
                "story changed during retry; original narration was preserved".into(),
            ));
        }
        let paths = remove_reply_in_tx(tx, story_id, target_id)?;
        reconcile_attribute_registry(tx, &scratch, &snapshot.original_registry_aliases)?;
        let passage = ledger_repository::append_story_message(
            tx,
            story_id,
            "narrator",
            "generated",
            visible,
            thoughts,
        )?;
        if let Some(staging) = &guard {
            staging.commit(tx, &passage.id)?;
        }
        Ok((ledger_repository::active_entry(tx, &passage.id)?, paths))
    })
}

fn start_replacement_generation(
    app: AppHandle,
    pool: &Pool,
    story_id: String,
    entry_id: String,
) -> AppResult<String> {
    let target = {
        let conn = pool.get()?;
        let last = ledger_repository::last_active_entry(&conn, &story_id)?
            .ok_or_else(|| AppError::Invalid("no narration to retry".into()))?;
        if last.id != entry_id {
            return Err(AppError::Invalid(
                "only the latest narration can be retried".into(),
            ));
        }
        if last.role() != "narrator" {
            return Err(AppError::Invalid(
                "only a narration entry can be retried".into(),
            ));
        }
        last
    };

    let snapshot = snapshot_for_retry(pool, &story_id, &target.id)?;
    let transcript = load_transcript(&snapshot.pool, &story_id, None)?;
    let prepared = narrator::prepare(NarratorInputs {
        app: &app,
        settings_pool: pool,
        world_pool: &snapshot.pool,
        story_id: &story_id,
        transcript,
        before_seq: None,
        purpose: NarratorPurpose::Replacement,
    })?;

    let story_id_bg = story_id.clone();
    let live_pool = pool.clone();
    let stream_id = narrator::spawn(prepared, move |app, sid, candidate| async move {
        let Candidate {
            visible,
            thoughts,
            staging,
            image_requests,
        } = candidate;
        let (entry, image_paths) = replace_narration_entry(
            &live_pool,
            &snapshot,
            &story_id_bg,
            &target.id,
            &visible,
            thoughts.as_deref(),
            staging,
        )
        .await?;
        images::delete_assets(&image_paths);
        let _ = app.emit(
            "narration-done",
            NarrationDonePayload {
                stream_id: sid,
                entry: entry.clone(),
            },
        );
        if !image_requests.is_empty() {
            images::generate_from_narrator_requests(
                &app,
                &live_pool,
                &entry.id,
                &visible,
                image_requests,
                None,
            );
        }
        Ok(())
    });

    Ok(stream_id)
}

pub(super) async fn retry_narration(
    app: AppHandle,
    pool: &Pool,
    story_id: String,
    entry_id: String,
) -> AppResult<RetryResult> {
    let trailing_action = {
        let conn = pool.get()?;
        ledger_repository::last_active_entry(&conn, &story_id)?
            .filter(|entry| entry.id == entry_id && entry.role() == "player")
    };
    let stream_id = if let Some(action) = trailing_action {
        let history = load_transcript(pool, &story_id, Some(action.seq + 1))?;
        let active_action = {
            let conn = pool.get()?;
            ledger_repository::active_entry(&conn, &action.id)?
        };
        let mode = action.input_mode().to_string();
        start_action_generation(
            app,
            pool.clone(),
            story_id,
            active_action,
            mode,
            history,
            Some(action.seq + 1),
        )?
    } else {
        start_replacement_generation(app, pool, story_id, entry_id.clone())?
    };
    Ok(RetryResult {
        entry_id,
        stream_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::ledger::model::kind as ledger_kind;
    use crate::features::ledger::repository::append_entry;
    use crate::features::narrator::tools;
    use crate::features::stories;
    use serde_json::json;
    fn retry_fixture() -> (Pool, String, String) {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO stories (id, title, created_at, updated_at, settings_json)
            VALUES ('s', 'story', 'now', 'now', '{}')",
            [],
        )
        .unwrap();
        crate::features::entities::create_entity_with_id_sync(
            &conn,
            "mira",
            "s",
            "character",
            "Mira",
            Some("old cloak"),
            "test",
            None,
        )
        .unwrap();
        crate::features::entities::create_entity_with_id_sync(
            &conn,
            "you",
            "s",
            "character",
            "You",
            None,
            "test",
            None,
        )
        .unwrap();
        let action = ledger_repository::append_story_message(
            &conn,
            "s",
            "player",
            "do",
            "Open the door",
            None,
        )
        .unwrap();
        let reply = ledger_repository::append_story_message(
            &conn,
            "s",
            "narrator",
            "generated",
            "Original",
            None,
        )
        .unwrap();
        crate::features::entities::update_entity_sync(
            &conn,
            "s",
            "mira",
            "Mira Changed",
            Some("new cloak"),
            "narrator_tool",
            Some(&reply.id),
        )
        .unwrap();
        append_entry(
            &conn,
            "s",
            ledger_kind::DICEROLL,
            "hidden",
            Some("Old roll"),
            &json!({"roll":99,"chance_percent":50,"seed":123,"outcome":"success"}),
            Some(&reply.id),
        )
        .unwrap();
        (pool, action.id, reply.id)
    }

    #[test]
    fn retry_snapshot_reads_pre_turn_state_without_damaging_live_reply() {
        let (pool, action, reply) = retry_fixture();
        let snapshot = snapshot_for_retry(&pool, "s", &reply).unwrap();
        let scratch = snapshot.pool.get().unwrap();
        let entities = crate::features::entities::list_entities_sync(&scratch, "s", None).unwrap();
        assert_eq!(
            entities.iter().find(|e| e.id == "mira").unwrap().name,
            "Mira"
        );
        assert_eq!(
            ledger_repository::last_active_entry(&scratch, "s")
                .unwrap()
                .unwrap()
                .id,
            action
        );
        assert!(!ledger_repository::list_logical_entries(&scratch, "s")
            .unwrap()
            .iter()
            .any(|entry| entry.kind == ledger_kind::DICEROLL));
        let history = load_transcript(&snapshot.pool, "s", None).unwrap();
        assert_eq!(
            history.last().and_then(|turn| turn.entry_id.as_deref()),
            Some(action.as_str())
        );
        assert!(!history
            .iter()
            .any(|turn| turn.content.contains("Original") || turn.content.contains("Old roll")));
        drop(scratch);
        drop(snapshot);
        let conn = pool.get().unwrap();
        assert_eq!(
            ledger_repository::last_active_entry(&conn, "s")
                .unwrap()
                .unwrap()
                .id,
            reply
        );
        assert_eq!(
            crate::features::entities::list_entities_sync(&conn, "s", None)
                .unwrap()
                .iter()
                .find(|e| e.id == "mira")
                .unwrap()
                .name,
            "Mira Changed"
        );
    }

    #[test]
    fn migrated_player_bootstrap_after_reply_does_not_block_retry() {
        let (pool, action, reply) = retry_fixture();
        let conn = pool.get().unwrap();
        append_entry(
            &conn,
            "s",
            ledger_kind::ENTITY_CREATED,
            "hidden",
            Some("You was added as a character."),
            &json!({"entity_id":"you","name":"You","source":"story_bootstrap"}),
            None,
        )
        .unwrap();
        drop(conn);

        let snapshot = snapshot_for_retry(&pool, "s", &reply).unwrap();
        let history = load_transcript(&snapshot.pool, "s", None).unwrap();
        assert_eq!(
            history.last().and_then(|turn| turn.entry_id.as_deref()),
            Some(action.as_str())
        );
    }

    #[test]
    fn abandoned_retry_removes_its_temporary_snapshot() {
        let (pool, _, reply) = retry_fixture();
        let snapshot = snapshot_for_retry(&pool, "s", &reply).unwrap();
        let path = snapshot._path.0.clone();
        assert!(path.exists());
        drop(snapshot);
        assert!(!path.exists());
        assert_eq!(
            ledger_repository::last_active_entry(&pool.get().unwrap(), "s")
                .unwrap()
                .unwrap()
                .id,
            reply
        );
    }

    #[tokio::test]
    async fn retry_replays_you_creation_without_losing_canonical_player() {
        let (pool, _, reply) = retry_fixture();
        let conn = pool.get().unwrap();
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
            "you",
            &trust,
            2.0,
            "original reply",
            &reply,
            false,
        )
        .unwrap();
        drop(conn);
        let snapshot = snapshot_for_retry(&pool, "s", &reply).unwrap();
        let scratch = snapshot.pool.get().unwrap();
        assert!(
            crate::features::entities::list_entities_sync(&scratch, "s", None)
                .unwrap()
                .iter()
                .any(|entity| entity.id == "you" && entity.name == "You")
        );
        assert_eq!(
            scratch
                .query_row(
                    "SELECT COUNT(*) FROM entity_attributes WHERE story_id='s' AND entity_id='you'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        drop(scratch);
        replace_narration_entry(&pool, &snapshot, "s", &reply, "New", None, None)
            .await
            .unwrap();
        let conn = pool.get().unwrap();
        assert!(
            crate::features::entities::list_entities_sync(&conn, "s", None)
                .unwrap()
                .iter()
                .any(|entity| entity.id == "you" && entity.name == "You")
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM entity_attributes WHERE story_id='s' AND entity_id='you'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn replacement_removes_old_roll_and_effects_but_keeps_action() {
        let (pool, action, reply) = retry_fixture();
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO image_assets (id, entry_id, path, prompt, created_at)
            VALUES ('old-image', ?1, 'old-image.png', 'prompt', 'now')",
            [&reply],
        )
        .unwrap();
        drop(conn);
        let snapshot = snapshot_for_retry(&pool, "s", &reply).unwrap();
        let (new_reply, paths) =
            replace_narration_entry(&pool, &snapshot, "s", &reply, "New outcome", None, None)
                .await
                .unwrap();
        assert_eq!(paths, vec!["old-image.png"]);
        assert_ne!(new_reply.id, reply);
        let conn = pool.get().unwrap();
        let entries = ledger_repository::list_logical_entries(&conn, "s").unwrap();
        assert!(entries.iter().any(|entry| entry.id == action));
        assert!(!entries
            .iter()
            .any(|entry| entry.id == reply || entry.kind == ledger_kind::DICEROLL));
        assert_eq!(new_reply.content.as_deref(), Some("New outcome"));
        assert_eq!(
            crate::features::entities::list_entities_sync(&conn, "s", None)
                .unwrap()
                .iter()
                .find(|e| e.id == "mira")
                .unwrap()
                .name,
            "Mira"
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM image_assets", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn retry_can_stage_fresh_roll_and_entity_changes_against_pre_turn_world() {
        let (pool, _, reply) = retry_fixture();
        let snapshot = snapshot_for_retry(&pool, "s", &reply).unwrap();
        let staging = Arc::new(Mutex::new(TurnStaging::new(
            snapshot.pool.clone(),
            "s".into(),
        )));
        let tool_set = tools::narrator_portable_tools_for_settings(
            staging.clone(),
            String::new(),
            &stories::settings::NarratorToolSettings::default(),
        );
        let named = |name: &str| tool_set.iter().find(|tool| tool.name() == name).unwrap();
        let entities = named("get_entities")
            .execute(json!({"name":"Mira"}))
            .await
            .unwrap();
        assert_eq!(
            entities.as_json().unwrap()["entities"][0]["name"],
            json!("Mira")
        );
        let roll = named("roll_check")
            .execute(json!({"chance_percent":100,"reason":"retry"}))
            .await
            .unwrap();
        assert_eq!(roll.as_json().unwrap()["outcome"], json!("success"));
        named("update_entity")
            .execute(json!({"id":"mira", "name":"Mira Retried"}))
            .await
            .unwrap();
        named("create_entity")
            .execute(json!({"kind":"location", "name":"New chamber"}))
            .await
            .unwrap();
        let (entry, _) = replace_narration_entry(
            &pool,
            &snapshot,
            "s",
            &reply,
            "Retried",
            None,
            Some(staging),
        )
        .await
        .unwrap();
        let conn = pool.get().unwrap();
        let entries = ledger_repository::list_logical_entries(&conn, "s").unwrap();
        let rolls = entries
            .iter()
            .filter(|e| e.kind == ledger_kind::DICEROLL)
            .collect::<Vec<_>>();
        assert_eq!(rolls.len(), 1);
        assert_eq!(rolls[0].target_entry_id.as_deref(), Some(entry.id.as_str()));
        assert_eq!(rolls[0].payload["chance_percent"], json!(100));
        let entities = crate::features::entities::list_entities_sync(&conn, "s", None).unwrap();
        assert_eq!(
            entities.iter().find(|e| e.id == "mira").unwrap().name,
            "Mira Retried"
        );
        assert!(entities.iter().any(|e| e.name == "New chamber"));
    }

    #[tokio::test]
    async fn scratch_registry_changes_commit_only_with_a_successful_replacement() {
        let (pool, _, reply) = retry_fixture();
        let snapshot = snapshot_for_retry(&pool, "s", &reply).unwrap();
        let scratch = snapshot.pool.get().unwrap();
        let id: String = scratch
            .query_row(
                "SELECT id FROM attribute_registry WHERE canonical_name = 'Trust'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        crate::features::entities::attributes::add_alias(&scratch, &id, "Confidence").unwrap();
        scratch.execute("INSERT INTO attribute_registry
            (id, canonical_name, aliases_json, entity_kinds_json, min, max, category,
             is_user_created, created_in_story_id, created_at)
            VALUES ('new-attribute', 'Aethercraft', '[]', '[\"character\"]', 0, 10, 'user', 1, 's', 'now')", []).unwrap();
        drop(scratch);
        let live = pool.get().unwrap();
        let before: String = live
            .query_row(
                "SELECT aliases_json FROM attribute_registry WHERE id = ?1",
                [&id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!before.contains("Confidence"));
        assert_eq!(
            live.query_row(
                "SELECT COUNT(*) FROM attribute_registry WHERE canonical_name = 'Aethercraft'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        drop(live);
        replace_narration_entry(&pool, &snapshot, "s", &reply, "New", None, None)
            .await
            .unwrap();
        let live = pool.get().unwrap();
        let after: String = live
            .query_row(
                "SELECT aliases_json FROM attribute_registry WHERE id = ?1",
                [&id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(after.contains("Confidence"));
        assert_eq!(
            live.query_row(
                "SELECT id FROM attribute_registry WHERE canonical_name = 'Aethercraft'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
            "new-attribute"
        );
    }

    #[tokio::test]
    async fn registry_collision_rolls_back_reply_removal_and_entity_replay() {
        let (pool, _, reply) = retry_fixture();
        let snapshot = snapshot_for_retry(&pool, "s", &reply).unwrap();
        let insert_registry = "INSERT INTO attribute_registry
            (id, canonical_name, aliases_json, entity_kinds_json, min, max, category,
             is_user_created, created_in_story_id, created_at)
            VALUES (?1, 'Aethercraft', '[]', '[\"character\"]', 0, 10, 'user', 1, 's', 'now')";
        snapshot
            .pool
            .get()
            .unwrap()
            .execute(insert_registry, ["scratch-id"])
            .unwrap();
        pool.get()
            .unwrap()
            .execute(insert_registry, ["live-id"])
            .unwrap();
        assert!(
            replace_narration_entry(&pool, &snapshot, "s", &reply, "New", None, None)
                .await
                .is_err()
        );
        let conn = pool.get().unwrap();
        assert_eq!(
            ledger_repository::last_active_entry(&conn, "s")
                .unwrap()
                .unwrap()
                .id,
            reply
        );
        assert!(ledger_repository::list_logical_entries(&conn, "s")
            .unwrap()
            .iter()
            .any(|entry| entry.kind == ledger_kind::DICEROLL));
        assert_eq!(
            crate::features::entities::list_entities_sync(&conn, "s", None)
                .unwrap()
                .iter()
                .find(|entity| entity.id == "mira")
                .unwrap()
                .name,
            "Mira Changed"
        );
    }

    #[tokio::test]
    async fn empty_generation_and_concurrent_edit_preserve_original_and_roll() {
        let (pool, action, reply) = retry_fixture();
        let snapshot = snapshot_for_retry(&pool, "s", &reply).unwrap();
        assert!(
            replace_narration_entry(&pool, &snapshot, "s", &reply, " ", None, None)
                .await
                .is_err()
        );
        with_transaction(&pool, |tx| {
            ledger_repository::append_entry(
                tx,
                "s",
                ledger_kind::CONTENT_EDITED,
                "hidden",
                Some("Player edited while retry ran"),
                &json!({"reason":"user_edit"}),
                Some(&action),
            )?;
            Ok(())
        })
        .unwrap();
        assert!(
            replace_narration_entry(&pool, &snapshot, "s", &reply, "New", None, None)
                .await
                .is_err()
        );
        let conn = pool.get().unwrap();
        let entries = ledger_repository::list_logical_entries(&conn, "s").unwrap();
        assert!(entries
            .iter()
            .any(|entry| entry.id == reply && entry.content.as_deref() == Some("Original")));
        assert!(entries
            .iter()
            .any(|entry| entry.kind == ledger_kind::DICEROLL));
        assert_eq!(
            crate::features::entities::list_entities_sync(&conn, "s", None)
                .unwrap()
                .iter()
                .find(|e| e.id == "mira")
                .unwrap()
                .name,
            "Mira Changed"
        );
    }

    #[tokio::test]
    async fn changed_story_settings_reject_retry_even_when_timestamp_is_unchanged() {
        let (pool, _, reply) = retry_fixture();
        let snapshot = snapshot_for_retry(&pool, "s", &reply).unwrap();
        pool.get().unwrap().execute(
            "UPDATE stories SET settings_json = '{\"author_note\":\"new guidance\"}' WHERE id = 's'", [],
        ).unwrap();
        assert!(
            replace_narration_entry(&pool, &snapshot, "s", &reply, "New", None, None)
                .await
                .is_err()
        );
        let conn = pool.get().unwrap();
        assert_eq!(
            ledger_repository::last_active_entry(&conn, "s")
                .unwrap()
                .unwrap()
                .id,
            reply
        );
        assert!(ledger_repository::list_logical_entries(&conn, "s")
            .unwrap()
            .iter()
            .any(|entry| entry.kind == ledger_kind::DICEROLL));
    }
}
