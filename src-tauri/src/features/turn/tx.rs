use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use tokio::sync::Mutex;

use super::gate::{GateGuard, TurnGate};
use crate::features::usage::{model::UsageRecord, repository as usage_repository};
use crate::shared::db::{open_connection, Pool};
use crate::shared::error::{AppError, AppResult};

/// All writes to a generating story, including its images and auto-title, use
/// this transaction. Keeping them together permits BEGIN IMMEDIATE to become
/// BEGIN CONCURRENT if storage with concurrent writes is adopted later.
pub struct TurnTx {
    story_id: String,
    conn: Mutex<rusqlite::Connection>,
    done: AtomicBool,
    gate: StdMutex<Option<GateGuard>>,
    usage: StdMutex<Vec<UsageRecord>>,
}

fn flush_usage(story_id: &str, usage: &StdMutex<Vec<UsageRecord>>, conn: &rusqlite::Connection) {
    let records = std::mem::take(&mut *usage.lock().unwrap_or_else(|e| e.into_inner()));
    for record in records {
        if let Err(error) = usage_repository::insert(conn, story_id, &record) {
            log::warn!("could not record usage for story {story_id}: {error}");
        }
    }
}

impl TurnTx {
    pub fn begin(pool: &Pool, gate: &TurnGate, story_id: &str) -> AppResult<Arc<TurnTx>> {
        let guard = gate.acquire(story_id)?;
        let conn = open_connection(pool)?;
        conn.execute_batch("BEGIN IMMEDIATE")?;
        Ok(Arc::new(TurnTx {
            story_id: story_id.to_string(),
            conn: Mutex::new(conn),
            done: AtomicBool::new(false),
            gate: StdMutex::new(Some(guard)),
            usage: StdMutex::new(Vec::new()),
        }))
    }

    fn release_gate(&self) {
        self.gate
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
    }

    pub fn record_usage(&self, record: UsageRecord) {
        let mut usage = self.usage.lock().unwrap_or_else(|e| e.into_inner());
        if self.done.load(Ordering::Acquire) {
            log::warn!(
                "usage reported after turn finished for story {}",
                self.story_id
            );
            return;
        }
        usage.push(record);
    }

    pub async fn with<R>(
        &self,
        f: impl FnOnce(&rusqlite::Connection) -> AppResult<R>,
    ) -> AppResult<R> {
        let conn = self.conn.lock().await;
        if self.done.load(Ordering::Acquire) {
            return Err(AppError::Invalid(
                "turn transaction is already finished".into(),
            ));
        }
        f(&conn)
    }

    /// Runs `f` inside a SAVEPOINT of the open turn. If `f` fails, only its
    /// writes are undone; the turn stays open and usable.
    pub async fn with_savepoint<R>(
        &self,
        f: impl FnOnce(&rusqlite::Connection) -> AppResult<R>,
    ) -> AppResult<R> {
        self.with(|conn| {
            conn.execute_batch("SAVEPOINT turn_step")?;
            match f(conn) {
                Ok(value) => {
                    conn.execute_batch("RELEASE turn_step")?;
                    Ok(value)
                }
                Err(error) => {
                    conn.execute_batch("ROLLBACK TO turn_step; RELEASE turn_step")?;
                    Err(error)
                }
            }
        })
        .await
    }

    pub async fn commit(&self) -> AppResult<()> {
        let conn = self.conn.lock().await;
        if self.done.load(Ordering::Acquire) {
            return Err(AppError::Invalid(
                "turn transaction is already finished".into(),
            ));
        }
        flush_usage(&self.story_id, &self.usage, &conn);
        conn.execute_batch("COMMIT")?;
        self.done.store(true, Ordering::Release);
        self.release_gate();
        Ok(())
    }

    pub async fn rollback(&self) -> AppResult<()> {
        let conn = self.conn.lock().await;
        if self.done.load(Ordering::Acquire) {
            return Err(AppError::Invalid(
                "turn transaction is already finished".into(),
            ));
        }
        conn.execute_batch("ROLLBACK")?;
        flush_usage(&self.story_id, &self.usage, &conn);
        self.done.store(true, Ordering::Release);
        self.release_gate();
        Ok(())
    }

    pub fn story_id(&self) -> &str {
        &self.story_id
    }
}

impl Drop for TurnTx {
    fn drop(&mut self) {
        if !self.done.load(Ordering::Acquire) {
            let _ = self.conn.get_mut().execute_batch("ROLLBACK");
            flush_usage(&self.story_id, &self.usage, self.conn.get_mut());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::usage::model::UsageRecord;
    use crate::shared::test_support;

    async fn setup_story(pool: &Pool, gate: &TurnGate) {
        let turn = TurnTx::begin(pool, gate, "s").unwrap();
        turn.with(|conn| {
            test_support::story(conn, "s");
            Ok(())
        })
        .await
        .unwrap();
        turn.commit().await.unwrap();
    }

    fn usage_count(pool: &Pool) -> i64 {
        pool.get()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM usage_records WHERE story_id = 's'",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    #[tokio::test]
    async fn usage_is_written_with_the_commit() {
        let pool = crate::shared::db::test_pool();
        let gate = TurnGate::default();
        setup_story(&pool, &gate).await;
        let turn = TurnTx::begin(&pool, &gate, "s").unwrap();
        turn.record_usage(UsageRecord::image("image", Some(0.01)));
        turn.record_usage(UsageRecord::image("image", Some(0.02)));
        assert_eq!(usage_count(&pool), 0);
        turn.commit().await.unwrap();
        assert_eq!(usage_count(&pool), 2);
    }

    #[tokio::test]
    async fn usage_survives_rollback() {
        let pool = crate::shared::db::test_pool();
        let gate = TurnGate::default();
        setup_story(&pool, &gate).await;
        let turn = TurnTx::begin(&pool, &gate, "s").unwrap();
        turn.with(|conn| {
            conn.execute(
                "INSERT INTO settings (key, value) VALUES ('discarded', 'value')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        turn.record_usage(UsageRecord::image("image", Some(0.01)));
        turn.rollback().await.unwrap();
        assert_eq!(usage_count(&pool), 1);
        let count: i64 = pool
            .get()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM settings WHERE key = 'discarded'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn usage_survives_drop() {
        let pool = crate::shared::db::test_pool();
        let gate = TurnGate::default();
        setup_story(&pool, &gate).await;
        let turn = TurnTx::begin(&pool, &gate, "s").unwrap();
        turn.record_usage(UsageRecord::image("image", Some(0.01)));
        drop(turn);
        assert_eq!(usage_count(&pool), 1);
    }

    #[tokio::test]
    async fn gate_opens_after_usage_is_flushed() {
        let pool = crate::shared::db::test_pool();
        let gate = TurnGate::default();
        setup_story(&pool, &gate).await;
        let turn = TurnTx::begin(&pool, &gate, "s").unwrap();
        turn.record_usage(UsageRecord::image("image", None));
        turn.rollback().await.unwrap();
        let next = TurnTx::begin(&pool, &gate, "s").unwrap();
        assert_eq!(usage_count(&pool), 1);
        next.rollback().await.unwrap();
    }

    #[tokio::test]
    async fn writes_are_private_until_commit() {
        let pool = crate::shared::db::test_pool();
        let gate = TurnGate::default();
        let turn = TurnTx::begin(&pool, &gate, "s").unwrap();
        turn.with(|conn| {
            test_support::story(conn, "s");
            Ok(())
        }).await.unwrap();
        assert_eq!(
            turn.with(|conn| Ok(conn
                .query_row("SELECT COUNT(*) FROM stories", [], |row| row
                    .get::<_, i64>(0))?))
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            pool.get()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM stories", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        turn.commit().await.unwrap();
        assert_eq!(
            pool.get()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM stories", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn savepoint_failure_preserves_other_turn_writes() {
        let pool = crate::shared::db::test_pool();
        let gate = TurnGate::default();
        let turn = TurnTx::begin(&pool, &gate, "s").unwrap();
        turn.with(|conn| {
            test_support::story(conn, "s");
            Ok(())
        })
        .await
        .unwrap();
        assert!(matches!(
            turn.with_savepoint(|conn| {
                conn.execute("INSERT INTO settings (key, value) VALUES ('step', 'lost')", [])?;
                Err::<(), _>(AppError::Other("x".into()))
            })
            .await,
            Err(AppError::Other(message)) if message == "x"
        ));
        turn.commit().await.unwrap();
        let conn = pool.get().unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM stories WHERE id = 's'", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM settings WHERE key = 'step'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn drop_rolls_back_and_releases_gate() {
        let pool = crate::shared::db::test_pool();
        let gate = TurnGate::default();
        let turn = TurnTx::begin(&pool, &gate, "s").unwrap();
        turn.with(|conn| {
            test_support::story(conn, "s");
            Ok(())
        }).await.unwrap();
        assert!(
            matches!(gate.check_idle("s"), Err(AppError::Invalid(message)) if message == "a turn is already generating")
        );
        assert!(
            matches!(TurnTx::begin(&pool, &gate, "other"), Err(AppError::Invalid(message)) if message == "another story is generating")
        );
        drop(turn);
        gate.check_idle("s").unwrap();
        assert_eq!(
            pool.get()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM stories", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        let next = TurnTx::begin(&pool, &gate, "other").unwrap();
        next.rollback().await.unwrap();
    }

    #[tokio::test]
    async fn commit_releases_gate_while_a_clone_is_alive() {
        let pool = crate::shared::db::test_pool();
        let gate = TurnGate::default();
        let turn = TurnTx::begin(&pool, &gate, "s").unwrap();
        let retained = Arc::clone(&turn);
        turn.commit().await.unwrap();

        let next = TurnTx::begin(&pool, &gate, "s").unwrap();
        next.rollback().await.unwrap();
        drop(retained);
    }

    #[tokio::test]
    async fn rollback_releases_gate_while_a_clone_is_alive() {
        let pool = crate::shared::db::test_pool();
        let gate = TurnGate::default();
        let turn = TurnTx::begin(&pool, &gate, "s").unwrap();
        let retained = Arc::clone(&turn);
        turn.rollback().await.unwrap();

        let next = TurnTx::begin(&pool, &gate, "s").unwrap();
        next.rollback().await.unwrap();
        drop(retained);
    }

    #[tokio::test]
    async fn same_story_turn_invalidates_idle_ticket_after_commit() {
        let pool = crate::shared::db::test_pool();
        let gate = TurnGate::default();
        let ticket = gate.check_idle("s").unwrap();
        let turn = TurnTx::begin(&pool, &gate, "s").unwrap();
        turn.commit().await.unwrap();
        assert!(matches!(
            gate.still_idle(&ticket),
            Err(AppError::Invalid(message)) if message == "a turn is already generating"
        ));
    }

    #[tokio::test]
    async fn other_story_turn_does_not_invalidate_idle_ticket() {
        let pool = crate::shared::db::test_pool();
        let gate = TurnGate::default();
        let ticket = gate.check_idle("s").unwrap();
        let turn = TurnTx::begin(&pool, &gate, "other").unwrap();
        turn.commit().await.unwrap();
        gate.still_idle(&ticket).unwrap();
    }

    #[tokio::test]
    async fn other_story_writes_wait_while_readers_see_committed_state() {
        let pool = crate::shared::db::test_pool();
        let gate = TurnGate::default();
        let turn = TurnTx::begin(&pool, &gate, "story-a").unwrap();
        turn.with(|conn| {
            conn.execute(
                "INSERT INTO settings (key, value) VALUES ('waiting_setting', 'turn value')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        assert!(matches!(
            TurnTx::begin(&pool, &gate, "story-b"),
            Err(AppError::Invalid(message)) if message == "another story is generating"
        ));
        assert_eq!(
            pool.get()
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM settings WHERE key = 'waiting_setting'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0,
        );

        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let finished = Arc::new(AtomicBool::new(false));
        let writer_finished = Arc::clone(&finished);
        let writer_pool = pool.clone();
        let writer = std::thread::spawn(move || {
            let conn = writer_pool.get().unwrap();
            started_tx.send(()).unwrap();
            conn.execute(
                "INSERT INTO settings (key, value) VALUES ('other_setting', 'saved')",
                [],
            )
            .unwrap();
            writer_finished.store(true, Ordering::Release);
        });
        started_rx.recv().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(75));
        assert!(!finished.load(Ordering::Acquire));

        turn.commit().await.unwrap();
        writer.join().unwrap();
        assert!(finished.load(Ordering::Acquire));
        assert_eq!(
            pool.get()
                .unwrap()
                .query_row(
                    "SELECT value FROM settings WHERE key = 'other_setting'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "saved",
        );
    }
}
