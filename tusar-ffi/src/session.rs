use hbc_decomp::{BytecodeFile, BytecodeFormat, DecompileOptionsV2, PipelineContext};
use parking_lot::RwLock;
use std::{
    collections::HashMap,
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

pub struct Session {
    pub input_path: String,
    pub bytes: Vec<u8>,
    pub file: BytecodeFile,
    pub format: BytecodeFormat,
    pub pipeline_ctx: Option<PipelineContext>,
}

impl Session {
    pub fn ensure_pipeline(&mut self) -> hbc_decomp::Result<()> {
        if self.pipeline_ctx.is_none() {
            let cache_path = hbc_decomp::default_cache_path(Path::new(&self.input_path));
            self.pipeline_ctx = Some(PipelineContext::build_cached(
                &self.file,
                &self.format,
                &DecompileOptionsV2::optimized(),
                &self.bytes,
                &cache_path,
            )?);
        }
        Ok(())
    }
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
