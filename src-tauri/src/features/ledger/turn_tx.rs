use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use tokio::sync::Mutex;

use crate::shared::db::{open_connection, Pool};
use crate::shared::error::{AppError, AppResult};

#[derive(Default)]
pub struct TurnGate {
    current: Arc<StdMutex<Option<String>>>,
}

pub struct GateGuard {
    current: Arc<StdMutex<Option<String>>>,
}

impl TurnGate {
    pub fn acquire(&self, story_id: &str) -> AppResult<GateGuard> {
        let mut current = self.current.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(active_story) = current.as_deref() {
            return Err(AppError::Invalid(if active_story == story_id {
                "a turn is already generating".into()
            } else {
                "another story is generating".into()
            }));
        }
        *current = Some(story_id.to_string());
        Ok(GateGuard {
            current: Arc::clone(&self.current),
        })
    }

    pub fn check_idle(&self, story_id: &str) -> AppResult<()> {
        let current = self.current.lock().unwrap_or_else(|error| error.into_inner());
        if current.as_deref() == Some(story_id) {
            return Err(AppError::Invalid("a turn is already generating".into()));
        }
        Ok(())
    }
}

impl Drop for GateGuard {
    fn drop(&mut self) {
        *self.current.lock().unwrap_or_else(|error| error.into_inner()) = None;
    }
}

pub struct TurnTx {
    story_id: String,
    conn: Mutex<rusqlite::Connection>,
    done: AtomicBool,
    _gate: GateGuard,
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
            _gate: guard,
        }))
    }

    pub async fn with<R>(&self, f: impl FnOnce(&rusqlite::Connection) -> AppResult<R>) -> AppResult<R> {
        let conn = self.conn.lock().await;
        if self.done.load(Ordering::Acquire) {
            return Err(AppError::Invalid("turn transaction is already finished".into()));
        }
        f(&conn)
    }

    pub async fn commit(&self) -> AppResult<()> {
        let conn = self.conn.lock().await;
        if self.done.load(Ordering::Acquire) {
            return Err(AppError::Invalid("turn transaction is already finished".into()));
        }
        conn.execute_batch("COMMIT")?;
        self.done.store(true, Ordering::Release);
        Ok(())
    }

    pub async fn rollback(&self) -> AppResult<()> {
        let conn = self.conn.lock().await;
        if self.done.load(Ordering::Acquire) {
            return Err(AppError::Invalid("turn transaction is already finished".into()));
        }
        conn.execute_batch("ROLLBACK")?;
        self.done.store(true, Ordering::Release);
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn writes_are_private_until_commit() {
        let pool = crate::shared::db::test_pool();
        let gate = TurnGate::default();
        let turn = TurnTx::begin(&pool, &gate, "s").unwrap();
        turn.with(|conn| {
            conn.execute(
                "INSERT INTO stories (id, title, created_at, updated_at) VALUES ('s', 'Story', 'now', 'now')",
                [],
            )?;
            Ok(())
        }).await.unwrap();
        assert_eq!(turn.with(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM stories", [], |row| row.get::<_, i64>(0))?)).await.unwrap(), 1);
        assert_eq!(pool.get().unwrap().query_row("SELECT COUNT(*) FROM stories", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
        turn.commit().await.unwrap();
        assert_eq!(pool.get().unwrap().query_row("SELECT COUNT(*) FROM stories", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
    }

    #[tokio::test]
    async fn drop_rolls_back_and_releases_gate() {
        let pool = crate::shared::db::test_pool();
        let gate = TurnGate::default();
        let turn = TurnTx::begin(&pool, &gate, "s").unwrap();
        turn.with(|conn| {
            conn.execute("INSERT INTO stories (id, title, created_at, updated_at) VALUES ('s', 'Story', 'now', 'now')", [])?;
            Ok(())
        }).await.unwrap();
        assert!(matches!(gate.check_idle("s"), Err(AppError::Invalid(message)) if message == "a turn is already generating"));
        assert!(matches!(TurnTx::begin(&pool, &gate, "other"), Err(AppError::Invalid(message)) if message == "another story is generating"));
        drop(turn);
        gate.check_idle("s").unwrap();
        assert_eq!(pool.get().unwrap().query_row("SELECT COUNT(*) FROM stories", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
        let next = TurnTx::begin(&pool, &gate, "other").unwrap();
        next.rollback().await.unwrap();
    }
}
