use chrono::Utc;
use rusqlite::{params, Connection};
use std::time::Duration;
use uuid::Uuid;

use crate::shared::db::{self, Pool};
use crate::shared::error::AppResult;

use super::model::{StoryUsage, UsageRecord};

pub const LATE_REPLY_LIMIT: Duration = Duration::from_secs(600);

/// Writes usage that arrived after its turn ended (a request that outlived
/// its timeout). The turn's transaction is gone, so this uses the pool.
pub async fn record_late(pool: Pool, story_id: String, record: UsageRecord) {
    let result = db::blocking(move || {
        let conn = pool.get()?;
        insert(&conn, &story_id, &record, false)
    })
    .await;
    if let Err(error) = result {
        log::warn!("could not record late usage: {error}");
    }
}

pub fn insert(conn: &Connection, story_id: &str, record: &UsageRecord, earlier_attempt: bool) -> AppResult<()> {
    let usage = &record.usage;
    let tokens = |count| i64::try_from(count).unwrap_or(i64::MAX);
    conn.execute(
        "INSERT INTO usage_records
         (id, story_id, kind, provider, model, response_id, input_tokens,
           output_tokens, cached_input_tokens, cache_write_tokens, cost_usd, created_at,
           turn_id, image_asset_id, duration_ms, earlier_attempt)
          VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        params![
            Uuid::new_v4().to_string(),
            story_id,
            record.kind.as_str(),
            record.provider,
            record.model,
            usage.response_id,
            tokens(usage.input_tokens),
            tokens(usage.output_tokens),
            tokens(usage.cached_input_tokens),
            tokens(usage.cache_write_tokens),
            usage.cost_usd,
            Utc::now().to_rfc3339(),
            record.turn_id,
            record.image_asset_id,
            record.duration_ms.map(tokens),
            earlier_attempt
        ],
    )?;
    Ok(())
}

pub fn move_to_turn(conn: &Connection, story_id: &str, old: &str, new: &str) -> AppResult<()> {
    conn.execute(
        "UPDATE usage_records SET turn_id = ?1, earlier_attempt = 1 WHERE story_id = ?2 AND turn_id = ?3",
        params![new, story_id, old],
    )?;
    Ok(())
}

pub fn story_usage(conn: &Connection, story_id: &str) -> AppResult<StoryUsage> {
    let mut usage = conn.query_row(
        "SELECT
           COALESCE(SUM(CASE WHEN kind <> 'image' THEN cost_usd END), 0),
           COALESCE(SUM(CASE WHEN kind = 'image' THEN cost_usd END), 0),
           COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0),
           COALESCE(SUM(cached_input_tokens), 0), COALESCE(SUM(cache_write_tokens), 0),
           COUNT(CASE WHEN kind = 'image' AND cost_usd IS NOT NULL THEN 1 END),
           COUNT(CASE WHEN cost_usd IS NULL THEN 1 END), MIN(created_at)
         FROM usage_records WHERE story_id = ?1",
        [story_id],
        |row| {
            Ok(StoryUsage {
                text_cost_usd: row.get(0)?,
                image_cost_usd: row.get(1)?,
                total_cost_usd: 0.0,
                input_tokens: row.get(2)?,
                output_tokens: row.get(3)?,
                cached_input_tokens: row.get(4)?,
                cache_write_tokens: row.get(5)?,
                image_count: row.get(6)?,
                unpriced_calls: row.get(7)?,
                since: row.get(8)?,
            })
        },
    )?;
    usage.total_cost_usd = usage.text_cost_usd + usage.image_cost_usd;
    Ok(usage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::{CallUsage, TextModelConfig};
    use crate::features::usage::model::UsageKind;
    use crate::features::turn::{TurnGate, TurnTx};
    use crate::shared::{db, test_support};

    fn text(
        kind: UsageKind,
        cost: Option<f64>,
        input: u64,
        cached: u64,
        write: u64,
        output: u64,
    ) -> UsageRecord {
        UsageRecord::text(
            kind,
            &TextModelConfig {
                provider: "openrouter".into(),
                model: "test".into(),
                api_key: String::new(),
                context_window: 1024,
                supports_images: false,
            },
            CallUsage {
                cost_usd: cost,
                input_tokens: input,
                cached_input_tokens: cached,
                cache_write_tokens: write,
                output_tokens: output,
                ..CallUsage::default()
            },
        )
    }

    fn near(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
    }

    #[test]
    fn insert_round_trips_turn_image_and_duration() {
        let pool = db::test_pool();
        let conn = pool.get().unwrap();
        test_support::story(&conn, "s");
        let mut record = UsageRecord::image("m", Some(0.039), Some("asset".into()), Some(14_200));
        record.turn_id = Some("t1".into());
        insert(&conn, "s", &record, false).unwrap();
        let row: (Option<String>, Option<String>, Option<i64>, i64) = conn.query_row(
            "SELECT turn_id, image_asset_id, duration_ms, earlier_attempt FROM usage_records WHERE story_id = 's'",
            [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        ).unwrap();
        assert_eq!(row, (Some("t1".into()), Some("asset".into()), Some(14_200), 0));
    }

    #[test]
    fn move_to_turn_touches_only_that_story_and_turn() {
        let pool = db::test_pool();
        let conn = pool.get().unwrap();
        test_support::story(&conn, "s");
        test_support::story(&conn, "other");
        for (story, turn, cost) in [("s", "t1", 0.01), ("s", "t9", 0.02), ("other", "t1", 0.03)] {
            let mut record = UsageRecord::image("m", Some(cost), None, None);
            record.turn_id = Some(turn.into());
            insert(&conn, story, &record, false).unwrap();
        }
        move_to_turn(&conn, "s", "t1", "t2").unwrap();
        let rows: Vec<(String, String, i64)> = conn.prepare(
            "SELECT story_id, turn_id, earlier_attempt FROM usage_records ORDER BY cost_usd",
        ).unwrap().query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap().collect::<Result<_, _>>().unwrap();
        assert_eq!(rows, [("s".into(), "t2".into(), 1), ("s".into(), "t9".into(), 0), ("other".into(), "t1".into(), 0)]);
    }

    #[tokio::test]
    async fn record_late_writes_through_the_pool() {
        let pool = db::test_pool();
        test_support::story(&pool.get().unwrap(), "s");
        record_late(
            pool.clone(),
            "s".into(),
            UsageRecord::image("m", Some(0.02), None, None),
        )
        .await;
        let conn = pool.get().unwrap();
        let usage = story_usage(&conn, "s").unwrap();
        near(usage.image_cost_usd, 0.02);
        assert_eq!(usage.image_count, 1);
    }

    #[tokio::test]
    async fn record_late_waits_for_an_open_turn_without_failing() {
        let pool = db::test_pool();
        let conn = pool.get().unwrap();
        test_support::story(&conn, "s");
        test_support::story(&conn, "other");
        drop(conn);
        let gate = TurnGate::default();
        let turn = TurnTx::begin(&pool, &gate, "other").unwrap();
        let mut late = tokio::spawn(record_late(
            pool.clone(),
            "s".into(),
            UsageRecord::image("m", Some(0.02), None, None),
        ));
        tokio::task::yield_now().await;
        assert!(tokio::time::timeout(Duration::from_millis(75), &mut late)
            .await
            .is_err());
        turn.commit().await.unwrap();
        late.await.unwrap();
        let conn = pool.get().unwrap();
        near(story_usage(&conn, "s").unwrap().image_cost_usd, 0.02);
    }

    #[test]
    fn empty_story_has_default_usage() {
        let pool = db::test_pool();
        let conn = pool.get().unwrap();
        test_support::story(&conn, "s");
        assert_eq!(story_usage(&conn, "s").unwrap(), StoryUsage::default());
    }

    #[test]
    fn totals_and_first_record_time() {
        let pool = db::test_pool();
        let conn = pool.get().unwrap();
        test_support::story(&conn, "s");
        insert(
            &conn,
            "s",
            &text(UsageKind::Narration, Some(0.010), 1000, 600, 100, 200),
            false,
        )
        .unwrap();
        let earliest: String = conn
            .query_row(
                "SELECT created_at FROM usage_records WHERE story_id = 's'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        insert(
            &conn,
            "s",
            &text(UsageKind::Summary, Some(0.002), 0, 0, 0, 0),
            false,
        )
        .unwrap();
        insert(&conn, "s", &text(UsageKind::Title, None, 50, 0, 0, 0), false).unwrap();
        insert(&conn, "s", &UsageRecord::image("image", Some(0.040), None, None), false).unwrap();
        insert(&conn, "s", &UsageRecord::image("image", None, None, None), false).unwrap();

        let usage = story_usage(&conn, "s").unwrap();
        near(usage.text_cost_usd, 0.012);
        near(usage.image_cost_usd, 0.040);
        near(usage.total_cost_usd, 0.052);
        assert_eq!(
            (
                usage.input_tokens,
                usage.cached_input_tokens,
                usage.cache_write_tokens,
                usage.output_tokens
            ),
            (1050, 600, 100, 200)
        );
        assert_eq!((usage.image_count, usage.unpriced_calls), (1, 2));
        assert_eq!(usage.since.as_deref(), Some(earliest.as_str()));
    }

    #[test]
    fn stories_are_isolated() {
        let pool = db::test_pool();
        let conn = pool.get().unwrap();
        test_support::story(&conn, "a");
        test_support::story(&conn, "b");
        insert(&conn, "b", &UsageRecord::image("image", Some(0.5), None, None), false).unwrap();
        assert_eq!(story_usage(&conn, "a").unwrap(), StoryUsage::default());
    }

    #[test]
    fn deleting_story_cascades_to_usage() {
        let pool = db::test_pool();
        let conn = pool.get().unwrap();
        test_support::story(&conn, "s");
        insert(&conn, "s", &UsageRecord::image("image", Some(0.5), None, None), false).unwrap();
        conn.execute("DELETE FROM stories WHERE id = ?1", ["s"])
            .unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM usage_records WHERE story_id = ?1",
                ["s"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }
}
