use crate::shared::error::AppResult;
use rusqlite::OptionalExtension;

use super::model::{kind as transcript_kind, StoryImage};

fn row_to_image(row: &rusqlite::Row) -> rusqlite::Result<StoryImage> {
    Ok(StoryImage {
        id: row.get(0)?,
        entry_id: row.get(1)?,
        prompt: row.get(2)?,
        created_at: row.get(3)?,
        caption: row.get(4)?,
    })
}

pub(crate) fn images_for_story(
    conn: &rusqlite::Connection,
    story_id: &str,
) -> AppResult<Vec<StoryImage>> {
    let mut stmt = conn.prepare(
        "SELECT image_assets.id, image_assets.entry_id, image_assets.prompt, image_assets.created_at,
           (SELECT json_extract(caption.payload_json, '$.caption') FROM transcript_entries AS caption
            WHERE caption.story_id = ?1 AND caption.kind = ?2
              AND json_extract(caption.payload_json, '$.asset_id') = image_assets.id
            ORDER BY caption.seq DESC LIMIT 1)
         FROM image_assets JOIN transcript_entries ON transcript_entries.id = image_assets.entry_id
         WHERE transcript_entries.story_id = ?1 ORDER BY image_assets.created_at ASC",
    )?;
    let rows = stmt.query_map([story_id, transcript_kind::IMAGE_CAPTIONED], row_to_image)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// Fetch only the newest eligible image blobs for narration entries in this transcript.
pub(crate) fn images_for_entries(
    conn: &rusqlite::Connection,
    story_id: &str,
    after_seq: Option<i64>,
    limit: usize,
) -> AppResult<Vec<(String, String, Vec<u8>)>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare(
        "SELECT image_assets.entry_id, image_blobs.media_type, image_blobs.bytes
         FROM image_assets
         JOIN image_blobs ON image_blobs.asset_id = image_assets.id
         JOIN transcript_entries ON transcript_entries.id = image_assets.entry_id
         WHERE transcript_entries.story_id = ?1 AND transcript_entries.kind = ?2
           AND (?3 IS NULL OR transcript_entries.seq > ?3)
           AND image_blobs.media_type IN ('image/png', 'image/jpeg', 'image/webp', 'image/gif')
         ORDER BY transcript_entries.seq DESC, image_assets.created_at DESC, image_assets.id DESC
         LIMIT ?4",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![story_id, transcript_kind::NARRATION, after_seq, limit],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub(crate) fn insert_image(
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

pub(crate) fn image_ids_for_entry(
    conn: &rusqlite::Connection,
    entry_id: &str,
) -> AppResult<Vec<String>> {
    let mut stmt = conn.prepare("SELECT id FROM image_assets WHERE entry_id = ?1")?;
    let rows = stmt.query_map([entry_id], |row| row.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub(crate) fn image_blob(
    conn: &rusqlite::Connection,
    id: &str,
) -> AppResult<Option<(String, Vec<u8>)>> {
    Ok(conn
        .query_row(
            "SELECT media_type, bytes FROM image_blobs WHERE asset_id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?)
}

pub fn detach_from_entry(tx: &rusqlite::Transaction<'_>, entry_id: &str) -> AppResult<()> {
    let asset_ids = image_ids_for_entry(tx, entry_id)?;
    if !asset_ids.is_empty() {
        let events = {
            let mut stmt = tx.prepare(
                "SELECT id, payload_json FROM transcript_entries WHERE kind IN (?1, ?2)",
            )?;
            let rows = stmt.query_map(
                [
                    transcript_kind::IMAGE_GENERATED,
                    transcript_kind::IMAGE_CAPTIONED,
                ],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        for (event_id, payload) in events {
            let asset_id = serde_json::from_str::<serde_json::Value>(&payload)
                .ok()
                .and_then(|value| value.get("asset_id")?.as_str().map(str::to_string));
            if asset_id.is_some_and(|asset_id| asset_ids.contains(&asset_id)) {
                tx.execute("DELETE FROM transcript_entries WHERE id = ?1", [event_id])?;
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
    use crate::shared::test_support;
    use serde_json::json;

    #[test]
    fn story_images_read_latest_caption_by_asset_and_leave_uncaptioned_images_none() {
        let pool = crate::shared::db::test_pool();
        let conn = pool.get().unwrap();
        test_support::story(&conn, "s");
        test_support::story(&conn, "other");
        let entry_id = test_support::record(
            &conn, "s", transcript_kind::NARRATION, Some("scene"), json!({}), None, None,
        );
        for id in ["asset-1", "asset-2", "asset-3"] {
            insert_image(
                &conn,
                &StoryImage {
                    id: id.into(),
                    entry_id: entry_id.clone(),
                    prompt: format!("prompt {id}"),
                    caption: None,
                    created_at: id.into(),
                },
                "image/png",
                b"image",
            )
            .unwrap();
        }
        for (story, kind, asset, content) in [
            ("s", transcript_kind::IMAGE_CAPTIONED, "asset-1", "old caption"),
            ("s", transcript_kind::IMAGE_CAPTIONED, "asset-2", "second caption"),
            ("s", transcript_kind::IMAGE_CAPTIONED, "asset-1", "latest caption"),
            ("s", transcript_kind::IMAGE_GENERATED, "asset-3", "not a caption"),
            ("other", transcript_kind::IMAGE_CAPTIONED, "asset-1", "wrong story"),
        ] {
            test_support::record(
                &conn, story, kind, Some(&format!("The generated scene image shows: {content}")), json!({"asset_id":asset,"caption":content}), None, None,
            );
        }

        let images = images_for_story(&conn, "s").unwrap();
        assert_eq!(images.len(), 3);
        for (image, id, caption) in [
            (&images[0], "asset-1", Some("latest caption")),
            (&images[1], "asset-2", Some("second caption")),
            (&images[2], "asset-3", None),
        ] {
            assert_eq!(image.id, id);
            assert_eq!(image.entry_id, entry_id);
            assert_eq!(image.prompt, format!("prompt {id}"));
            assert_eq!(image.created_at, id);
            assert_eq!(image.caption.as_deref(), caption);
        }
        assert!(images_for_story(&conn, "other").unwrap().is_empty());
    }

    #[test]
    fn detaching_an_entry_removes_only_its_images_and_events() {
        let pool = crate::shared::db::test_pool();
        let mut conn = pool.get().unwrap();
        conn.execute_batch(
            r#"INSERT INTO stories (id, title, created_at, updated_at) VALUES ('s', 'Story', 'now', 'now');
             INSERT INTO transcript_entries (id, story_id, seq, kind, visibility, payload_json, created_at)
               VALUES ('entry-1', 's', 1, 'narration', 'visible', '{}', 'now'),
                      ('entry-2', 's', 2, 'narration', 'visible', '{}', 'now');
             INSERT INTO image_assets VALUES ('asset-1', 'entry-1', '', 'first', 'now');
             INSERT INTO image_assets VALUES ('asset-2', 'entry-2', '', 'second', 'now');
             INSERT INTO image_blobs VALUES ('asset-1', 'image/png', X'0102');
             INSERT INTO image_blobs VALUES ('asset-2', 'image/png', X'0304');
             INSERT INTO transcript_entries (id, story_id, seq, kind, visibility, payload_json, created_at)
               VALUES ('event-1', 's', 3, 'image_generated', 'hidden', '{"asset_id":"asset-1"}', 'now'),
                      ('event-2', 's', 4, 'image_generated', 'hidden', '{"asset_id":"asset-2"}', 'now'),
                      ('event-3', 's', 5, 'content_edited', 'hidden', '{"asset_id":"asset-1"}', 'now'),
                      ('event-4', 's', 6, 'image_generated', 'hidden', '{"asset_id":"asset-1"}', 'now'),
                      ('event-5', 's', 7, 'image_generated', 'hidden', '{"asset_id":"asset-2"}', 'now');
             INSERT INTO transcript_entries (id, story_id, seq, kind, visibility, content, payload_json, created_at)
               VALUES ('caption-1', 's', 8, 'image_captioned', 'hidden', 'first caption', '{"asset_id":"asset-1","caption":"first caption"}', 'now'),
                      ('caption-2', 's', 9, 'image_captioned', 'hidden', 'second caption', '{"asset_id":"asset-2","caption":"second caption"}', 'now'),
                      ('caption-3', 's', 10, 'image_captioned', 'hidden', 'revised first caption', '{"asset_id":"asset-1","caption":"revised first caption"}', 'now');"#,
        )
        .unwrap();

        assert_eq!(
            image_ids_for_entry(&conn, "entry-1").unwrap(),
            vec!["asset-1"]
        );
        assert_eq!(
            image_blob(&conn, "asset-1").unwrap(),
            Some(("image/png".into(), vec![1, 2]))
        );
        assert_eq!(image_blob(&conn, "missing").unwrap(), None);

        let tx = conn.transaction().unwrap();
        detach_from_entry(&tx, "entry-1").unwrap();
        assert_eq!(tx.query_row("SELECT COUNT(*) FROM image_blobs", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
        let event_ids: Vec<String> = tx
            .prepare("SELECT id FROM transcript_entries ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(event_ids, vec!["caption-2", "entry-1", "entry-2", "event-2", "event-3", "event-5"]);
        let images = images_for_story(&tx, "s").unwrap();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].id, "asset-2");
        assert_eq!(images[0].caption.as_deref(), Some("second caption"));
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
             INSERT INTO transcript_entries (id, story_id, seq, kind, visibility, payload_json, created_at)
               VALUES ('entry', 's', 1, 'narration', 'visible', '{}', 'now');
             INSERT INTO image_assets VALUES ('asset', 'entry', '', 'prompt', 'now');
             INSERT INTO image_blobs VALUES ('asset', 'image/png', X'0102');
             DELETE FROM transcript_entries WHERE id = 'entry';",
        ).unwrap();
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM image_blobs", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
    }
}
