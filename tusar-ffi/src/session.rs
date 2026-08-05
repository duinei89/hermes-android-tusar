use parking_lot::RwLock;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

pub struct Session {
    pub input_path: String,

    // Add the real parsed hbc-decomp state here after verifying
    // the exact upstream types and constructors.
    //
    // Example only:
    // pub bytecode: BytecodeFile,
    // pub context: PipelineContext,
}

pub struct SessionStore {
    next_id: AtomicU64,
    sessions: RwLock<HashMap<u64, Arc<RwLock<Session>>>>,
}

impl SessionStore {
    pub fn new() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            sessions: RwLock::new(HashMap::new()),
        }
    }

    pub fn insert(&self, session: Session) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        self.sessions
            .write()
            .insert(id, Arc::new(RwLock::new(session)));

        id
    }

    pub fn get(&self, id: u64) -> Option<Arc<RwLock<Session>>> {
        self.sessions.read().get(&id).cloned()
    }

    pub fn remove(&self, id: u64) -> bool {
        self.sessions.write().remove(&id).is_some()
    }
}
