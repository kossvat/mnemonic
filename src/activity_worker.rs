//! Background work-activity sampler.
//!
//! Wakes every `sample_interval_secs`, reads the system idle time
//! (seconds since the last keyboard/mouse/trackpad event), feeds it to
//! the pure `activity::decide` state machine, and persists the result
//! to `activity.db`. No keystrokes, no titles, no screenshots — just an
//! idle counter.
//!
//! The idle read is a single CoreGraphics call
//! (`CGEventSourceSecondsSinceLastEventType`) that needs no entitlement
//! or accessibility permission and costs microseconds, so the daemon
//! footprint stays negligible.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::interval;
use tracing::{debug, info, warn};

use crate::activity::{ActivityStore, WorkSession, decide};
use crate::config::ActivityConfig;

/// Spawn the activity worker. Returns immediately; runs until the
/// daemon exits.
pub fn spawn_worker(store: Arc<ActivityStore>, cfg: ActivityConfig) -> tokio::task::JoinHandle<()> {
    info!(
        "Activity worker starting (sample={}s, idle_threshold={}s)",
        cfg.sample_interval_secs, cfg.idle_threshold_secs
    );
    tokio::spawn(async move {
        // Clamp the cadence: faster than 5s is pointless (idle has
        // second granularity and the daily total doesn't need it) and
        // would just spin the DB.
        let secs = cfg.sample_interval_secs.max(5);
        let mut ticker = interval(Duration::from_secs(secs));
        // In-memory mirror of the open session to avoid a DB read each
        // tick. Seeded from disk (which `ActivityStore::open` has
        // already finalized, so this is None on a fresh start).
        let mut open: Option<WorkSession> = match store.current_open() {
            Ok(o) => o,
            Err(e) => {
                warn!("Activity worker: current_open read failed: {e}");
                None
            }
        };
        loop {
            ticker.tick().await;
            let idle = match read_idle_seconds() {
                Some(v) => v,
                None => {
                    debug!("Activity worker: idle read unavailable this tick");
                    continue;
                }
            };
            let now = chrono::Utc::now();
            let action = decide(open.as_ref(), idle, now, cfg.idle_threshold_secs, || {
                uuid::Uuid::new_v4().to_string()
            });
            match store.apply(&action) {
                Ok(next_open) => open = next_open,
                Err(e) => warn!("Activity worker: apply failed: {e}"),
            }
        }
    })
}

/// Seconds since the last HID input event, or `None` on platforms /
/// builds where the call isn't available.
///
/// macOS: `CGEventSourceSecondsSinceLastEventType` with the HID system
/// source and the any-input event type. This is the same signal the OS
/// uses for its own idle/screensaver logic.
#[cfg(target_os = "macos")]
pub fn read_idle_seconds() -> Option<f64> {
    // kCGEventSourceStateHIDSystemState = 1
    const HID_SYSTEM_STATE: u32 = 1;
    // kCGAnyInputEventType = ~0 (0xFFFFFFFF)
    const ANY_INPUT_EVENT: u32 = u32::MAX;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventSourceSecondsSinceLastEventType(state: u32, event_type: u32) -> f64;
    }
    let secs = unsafe { CGEventSourceSecondsSinceLastEventType(HID_SYSTEM_STATE, ANY_INPUT_EVENT) };
    if secs.is_finite() && secs >= 0.0 {
        Some(secs)
    } else {
        None
    }
}

#[cfg(not(target_os = "macos"))]
pub fn read_idle_seconds() -> Option<f64> {
    // No portable idle source wired up off macOS yet; the worker simply
    // records nothing rather than guessing.
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn idle_read_returns_a_sane_value_on_macos() {
        // In CI / headless runs this still returns a finite, non-negative
        // number (often large). We only assert the contract, not a range.
        if let Some(v) = read_idle_seconds() {
            assert!(v.is_finite() && v >= 0.0);
        }
    }
}
