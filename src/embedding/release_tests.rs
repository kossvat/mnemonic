use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A model that says what it was asked and counts how often it was built.
struct Counted {
    id: &'static str,
}

impl Embedder for Counted {
    fn embed(&self, text: &str) -> Result<Embedding> {
        Ok(vec![text.len() as f32, 1.0, 2.0])
    }
    fn model_id(&self) -> &'static str {
        self.id
    }
    fn dim_hint(&self) -> Option<usize> {
        Some(3)
    }
}

fn releasing(builds: Arc<AtomicUsize>, id: &'static str) -> ReleasingEmbedder {
    let builder = move || -> Result<Box<dyn Embedder>> {
        builds.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Counted { id }))
    };
    ReleasingEmbedder::new(Box::new(Counted { id: "model-a" }), builder)
}

#[test]
fn embedding_a_released_model_answers_with_the_same_vector() {
    let builds = Arc::new(AtomicUsize::new(0));
    let embedder = releasing(builds.clone(), "model-a");
    let before = embedder.embed_query("what was decided").unwrap();
    assert!(embedder.release_if_idle(Duration::ZERO));
    assert!(!embedder.is_loaded());
    // What the daemon reports of its model holds while it is released.
    assert_eq!(embedder.model_id(), "model-a");
    assert_eq!(embedder.dim_hint(), Some(3));
    assert_eq!(embedder.last_reload(), None);

    let after = embedder.embed_query("what was decided").unwrap();
    assert_eq!(before, after);
    assert_eq!(builds.load(Ordering::SeqCst), 1);
    assert!(embedder.is_loaded());
    // The first query after the release is timed.
    assert!(embedder.last_reload().is_some());
}

#[test]
fn embedding_a_model_in_use_is_not_released() {
    let builds = Arc::new(AtomicUsize::new(0));
    let embedder = releasing(builds.clone(), "model-a");
    embedder.embed("a note").unwrap();
    assert!(!embedder.release_if_idle(Duration::from_secs(3600)));
    assert!(embedder.is_loaded());
    // Released once, a second release finds nothing to drop.
    assert!(embedder.release_if_idle(Duration::ZERO));
    assert!(!embedder.release_if_idle(Duration::ZERO));
    assert_eq!(builds.load(Ordering::SeqCst), 0);
}

#[test]
fn embedding_a_build_after_a_release_that_fails_is_an_error_not_another_embedder() {
    let embedder = ReleasingEmbedder::new(Box::new(Counted { id: "model-a" }), || {
        anyhow::bail!("the model cannot be read")
    });
    assert!(embedder.release_if_idle(Duration::ZERO));
    assert!(embedder.embed("a note").is_err());
    assert!(!embedder.is_loaded());

    // A builder that brings another model is refused the same way.
    let builds = Arc::new(AtomicUsize::new(0));
    let other = releasing(builds, "model-b");
    assert!(other.release_if_idle(Duration::ZERO));
    let refused = other.embed("a note");
    assert!(refused.is_err_and(|e| e.to_string().contains("model-b")));
}

#[test]
fn embedding_a_call_in_flight_keeps_the_model_until_it_ends() {
    let builds = Arc::new(AtomicUsize::new(0));
    let embedder = releasing(builds.clone(), "model-a");
    let held = embedder.model().unwrap();
    // Released now, the next call would build a second model beside the
    // one this call holds.
    assert!(!embedder.release_if_idle(Duration::ZERO));
    assert!(embedder.is_loaded());
    assert_eq!(held.embed("abc").unwrap(), vec![3.0, 1.0, 2.0]);
    drop(held);
    assert!(embedder.release_if_idle(Duration::ZERO));
    assert_eq!(builds.load(Ordering::SeqCst), 0);
}

/// A release judges the idle time from the last use as noted under the
/// lock every use passes, not from a reading taken before it: a model last
/// used an hour ago and used again just now is kept.
#[test]
fn embedding_a_use_just_before_a_release_keeps_the_model() {
    let builds = Arc::new(AtomicUsize::new(0));
    let embedder = releasing(builds.clone(), "model-a");
    *embedder.last_use.lock().unwrap() = Instant::now() - Duration::from_secs(3600);
    embedder.embed("again").unwrap();
    assert!(!embedder.release_if_idle(Duration::from_secs(1800)));
    assert!(embedder.is_loaded());
    assert_eq!(builds.load(Ordering::SeqCst), 0);
}

#[test]
fn embedding_concurrent_calls_after_a_release_build_the_model_once() {
    let builds = Arc::new(AtomicUsize::new(0));
    let embedder = Arc::new(releasing(builds.clone(), "model-a"));
    assert!(embedder.release_if_idle(Duration::ZERO));
    let calls: Vec<_> = (0..8)
        .map(|n| {
            let embedder = embedder.clone();
            std::thread::spawn(move || embedder.embed(&"x".repeat(n)).unwrap())
        })
        .collect();
    for call in calls {
        call.join().unwrap();
    }
    assert_eq!(builds.load(Ordering::SeqCst), 1);
}
