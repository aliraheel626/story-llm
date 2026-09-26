use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use crate::shared::error::{AppError, AppResult};

#[derive(Clone, Default)]
pub struct TurnGate {
    state: Arc<StdMutex<GateState>>,
}

#[derive(Default)]
struct GateState {
    current: Option<String>,
    /// Number of turns started per story, used to invalidate queued writes.
    started: HashMap<String, u64>,
}

#[derive(Clone)]
pub struct TurnTicket {
    story_id: String,
    started: u64,
}

pub struct GateGuard {
    state: Arc<StdMutex<GateState>>,
}

impl TurnGate {
    pub fn acquire(&self, story_id: &str) -> AppResult<GateGuard> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(active_story) = state.current.as_deref() {
            return Err(AppError::Invalid(if active_story == story_id {
                "a turn is already generating".into()
            } else {
                "another story is generating".into()
            }));
        }
        state.current = Some(story_id.to_string());
        *state.started.entry(story_id.to_string()).or_default() += 1;
        Ok(GateGuard {
            state: Arc::clone(&self.state),
        })
    }

    pub fn check_idle(&self, story_id: &str) -> AppResult<TurnTicket> {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.current.as_deref() == Some(story_id) {
            return Err(AppError::Invalid("a turn is already generating".into()));
        }
        Ok(TurnTicket {
            story_id: story_id.to_string(),
            started: state.started.get(story_id).copied().unwrap_or_default(),
        })
    }

    pub fn still_idle(&self, ticket: &TurnTicket) -> AppResult<()> {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.current.as_deref() == Some(&ticket.story_id)
            || state
                .started
                .get(&ticket.story_id)
                .copied()
                .unwrap_or_default()
                != ticket.started
        {
            return Err(AppError::Invalid("a turn is already generating".into()));
        }
        Ok(())
    }
}

impl Drop for GateGuard {
    fn drop(&mut self) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .current = None;
    }
}
