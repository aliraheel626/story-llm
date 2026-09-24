use crate::features::ledger::model::kind as ledger_kind;
use crate::shared::error::AppResult;

use super::model::StoryImage;

fn row_to_image(row: &rusqlite::Row) -> rusqlite::Result<StoryImage> {
    Ok(StoryImage {
        id: row.get(0)?,
        entry_id: row.get(1)?,
        prompt: row.get(2)?,
        created_at: row.get(3)?,
    })
}

pub(super) fn list_for_story(
    conn: &rusqlite::Connection,
    story_id: &str,
) -> AppResult<Vec<StoryImage>> {
    let mut stmt = conn.prepare(
        "SELECT image_assets.id, image_assets.entry_id, image_assets.prompt, image_assets.created_at
         FROM image_assets JOIN ledger_entries ON ledger_entries.id = image_assets.entry_id
         WHERE ledger_entries.story_id = ?1 ORDER BY image_assets.created_at ASC",
    )?;
    let rows = stmt.query_map([story_id], row_to_image)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub(super) fn insert_asset(
    conn: &rusqlite::Connection,
    image: &StoryImage,
    media_type: &str,
    bytes: &[u8],
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO image_assets (id, entry_id, path, prompt, created_at)
         VALUES (?1, ?2, '', ?3, ?4)",
        rusqlite::params![image.id, image.entry_id, image.prompt, image.created_at],
    )?;
    conn.execute(
        "INSERT INTO image_blobs (asset_id, media_type, bytes) VALUES (?1, ?2, ?3)",
        rusqlite::params![image.id, media_type, bytes],
    )?;
    Ok(())
}

fn delete_for_entry(tx: &rusqlite::Transaction<'_>, entry_id: &str) -> AppResult<()> {
    tx.execute("DELETE FROM image_assets WHERE entry_id = ?1", [entry_id])?;
    Ok(())
}

pub fn detach_from_entry(tx: &rusqlite::Transaction<'_>, entry_id: &str) -> AppResult<()> {
    let asset_ids = {
        let mut stmt = tx.prepare("SELECT id FROM image_assets WHERE entry_id = ?1")?;
        let rows = stmt.query_map([entry_id], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    if !asset_ids.is_empty() {
        let events = {
            let mut stmt =
                tx.prepare("SELECT id, payload_json FROM ledger_entries WHERE kind = ?1")?;
            let rows = stmt.query_map([ledger_kind::IMAGE_GENERATED], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        for (event_id, payload) in events {
            let asset_id = serde_json::from_str::<serde_json::Value>(&payload)
                .ok()
                .and_then(|value| value.get("asset_id")?.as_str().map(str::to_string));
            if asset_id.is_some_and(|asset_id| asset_ids.contains(&asset_id)) {
                tx.execute("DELETE FROM ledger_entries WHERE id = ?1", [event_id])?;
            }
        }
    }
    delete_for_entry(tx, entry_id)?;
    Ok(())
}

pub fn delete_asset_by_id(conn: &rusqlite::Connection, asset_id: &str) -> AppResult<()> {
    conn.execute("DELETE FROM image_assets WHERE id = ?1", [asset_id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detaching_an_entry_removes_only_its_images_and_events() {
        let pool = crate::shared::db::test_pool();
        let mut conn = pool.get().unwrap();
        conn.execute_batch(
            r#"INSERT INTO stories (id, title, created_at, updated_at) VALUES ('s', 'Story', 'now', 'now');
             INSERT INTO ledger_entries (id, story_id, seq, kind, visibility, payload_json, created_at)
               VALUES ('entry-1', 's', 1, 'narration', 'visible', '{}', 'now'),
                      ('entry-2', 's', 2, 'narration', 'visible', '{}', 'now');
             INSERT INTO image_assets VALUES ('asset-1', 'entry-1', '', 'first', 'now');
             INSERT INTO image_assets VALUES ('asset-2', 'entry-2', '', 'second', 'now');
             INSERT INTO image_blobs VALUES ('asset-1', 'image/png', X'0102');
             INSERT INTO image_blobs VALUES ('asset-2', 'image/png', X'0304');
             INSERT INTO ledger_entries (id, story_id, seq, kind, visibility, payload_json, created_at)
               VALUES ('event-1', 's', 3, 'image_generated', 'hidden', '{"asset_id":"asset-1"}', 'now'),
                      ('event-2', 's', 4, 'image_generated', 'hidden', '{"asset_id":"asset-2"}', 'now'),
                      ('event-3', 's', 5, 'content_edited', 'hidden', '{}', 'now'),
                      ('event-4', 's', 6, 'image_generated', 'hidden', '{"asset_id":"asset-1"}', 'now'),
                      ('event-5', 's', 7, 'image_generated', 'hidden', '{"asset_id":"asset-2"}', 'now');"#,
        )
        .unwrap();

        let tx = conn.transaction().unwrap();
        detach_from_entry(&tx, "entry-1").unwrap();
        assert_eq!(tx.query_row("SELECT COUNT(*) FROM image_blobs", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
        let event_ids: Vec<String> = tx
            .prepare("SELECT id FROM ledger_entries ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(event_ids, vec!["entry-1", "entry-2", "event-2", "event-3", "event-5"]);
        delete_asset_by_id(&tx, "asset-2").unwrap();
        delete_asset_by_id(&tx, "asset-2").unwrap();
        tx.commit().unwrap();
    }

    #[test]
    fn deleting_entry_cascades_to_blob() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        conn.execute_batch(
            "INSERT INTO stories (id, title, created_at, updated_at) VALUES ('s', 'Story', 'now', 'now');
             INSERT INTO ledger_entries (id, story_id, seq, kind, visibility, payload_json, created_at)
               VALUES ('entry', 's', 1, 'narration', 'visible', '{}', 'now');
             INSERT INTO image_assets VALUES ('asset', 'entry', '', 'prompt', 'now');
             INSERT INTO image_blobs VALUES ('asset', 'image/png', X'0102');
             DELETE FROM ledger_entries WHERE id = 'entry';",
        ).unwrap();
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM image_blobs", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
    }
}
