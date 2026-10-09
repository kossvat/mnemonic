//! A model that leaves memory when it is not used (live-projects, design 5.2).
//!
//! The neural model is most of the daemon's memory: its weights and the
//! buffers of its runtime, about 1.8 GB on the heap. The daemon needs it
//! when it saves or searches, which comes in bursts while the owner works
//! and not at all at night. So the model is dropped after an idle time and
//! built again on the next use; the first query after a release pays for
//! the load, and that time is recorded.
use anyhow::Result;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use super::{Embedder, Embedding};

type Builder = Box<dyn Fn() -> Result<Box<dyn Embedder>> + Send + Sync>;

/// The time the default configuration lets the model stay unused.
pub const DEFAULT_IDLE_RELEASE: Duration = Duration::from_secs(900);

/// An embedder held while it is used and dropped after an idle time. A
/// release frees what the model held; the next call builds it again with
/// the same builder, so it is the same model: a build that fails is an
/// error, never another embedder whose vectors would not match.
pub struct ReleasingEmbedder {
    model: RwLock<Option<Arc<dyn Embedder>>>,
    builder: Builder,
    model_id: &'static str,
    dim: Option<usize>,
    last_use: Mutex<Instant>,
    /// How long the last build after a release took.
    last_reload: Mutex<Option<Duration>>,
}

impl ReleasingEmbedder {
    /// Hold `first`, built already, and build again with `builder` after
    /// a release.
    pub fn new(
        first: Box<dyn Embedder>,
        builder: impl Fn() -> Result<Box<dyn Embedder>> + Send + Sync + 'static,
    ) -> Self {
        Self {
            model_id: first.model_id(),
            dim: first.dim_hint(),
            model: RwLock::new(Some(Arc::from(first))),
            builder: Box::new(builder),
            last_use: Mutex::new(Instant::now()),
            last_reload: Mutex::new(None),
        }
    }

    /// Whether the model is in memory now.
    #[cfg(test)]
    pub fn is_loaded(&self) -> bool {
        self.model.read().is_ok_and(|model| model.is_some())
    }

    /// Note a use now. Called with a lock of the model held, so that a
    /// release, which judges the idle time under the write lock, sees it.
    fn note_use(&self) {
        if let Ok(mut last) = self.last_use.lock() {
            *last = Instant::now();
        }
    }

    /// The model, built again when it was released.
    fn model(&self) -> Result<Arc<dyn Embedder>> {
        {
            let slot = self
                .model
                .read()
                .map_err(|e| anyhow::anyhow!("lock: {e}"))?;
            if let Some(model) = slot.as_ref() {
                self.note_use();
                return Ok(model.clone());
            }
        }
        let mut slot = self
            .model
            .write()
            .map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        self.note_use();
        // Another caller may have built it while this one waited.
        if let Some(model) = slot.as_ref() {
            return Ok(model.clone());
        }
        let began = Instant::now();
        let built: Arc<dyn Embedder> = Arc::from((self.builder)()?);
        anyhow::ensure!(
            built.model_id() == self.model_id,
            "the model built again is {} where {} was released",
            built.model_id(),
            self.model_id
        );
        let took = began.elapsed();
        if let Ok(mut last) = self.last_reload.lock() {
            *last = Some(took);
        }
        tracing::info!(
            "Embedding model loaded again after a release in {} ms",
            took.as_millis()
        );
        *slot = Some(built.clone());
        Ok(built)
    }
}

impl Embedder for ReleasingEmbedder {
    fn embed(&self, text: &str) -> Result<Embedding> {
        self.model()?.embed(text)
    }

    fn embed_query(&self, text: &str) -> Result<Embedding> {
        self.model()?.embed_query(text)
    }

    fn model_id(&self) -> &'static str {
        self.model_id
    }

    fn dim_hint(&self) -> Option<usize> {
        self.dim
    }

    /// Drop the model when it was not used for `idle` and no call holds
    /// it. The idle time is judged under the write lock, which no use can
    /// pass without noting itself, so a use that began a moment ago keeps
    /// the model; and a call in flight keeps it too, so two models are
    /// never in memory at once.
    fn release_if_idle(&self, idle: Duration) -> bool {
        let Ok(mut slot) = self.model.write() else {
            return false;
        };
        let unused = self
            .last_use
            .lock()
            .map(|last| last.elapsed() >= idle)
            .unwrap_or(false);
        let held = slot
            .as_ref()
            .is_some_and(|model| Arc::strong_count(model) > 1);
        if !unused || held || slot.take().is_none() {
            return false;
        }
        drop(slot);
        return_freed_memory();
        tracing::info!("Embedding model released after {} s unused", idle.as_secs());
        true
    }

    fn last_reload(&self) -> Option<Duration> {
        self.last_reload.lock().ok().and_then(|last| *last)
    }
}

/// Give the pages the allocator holds free back to the system. Without
/// it the memory a released model held stays counted to the daemon until
/// the system asks for it.
fn return_freed_memory() {
    #[cfg(target_os = "macos")]
    {
        unsafe extern "C" {
            fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
        }
        // SAFETY: a null zone asks every zone, and a goal of zero asks for
        // as much as can be given back; the call only returns free pages.
        unsafe {
            malloc_zone_pressure_relief(std::ptr::null_mut(), 0);
        }
    }
}

#[cfg(test)]
#[path = "release_tests.rs"]
mod tests;
