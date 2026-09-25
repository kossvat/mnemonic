//! Effective importance: combines base importance with usage signal.
//!
//! Pure functions, no DB. Used by ranking paths (Whisper, MCP context, search).
//!
//! Model:
//!   effective = base * recency_decay(last_access) * (1 + α·ln(1+access_count))
//!
//! - Untouched memories decay exponentially (half-life HALF_LIFE_DAYS).
//! - Touched memories get a sublinear access boost (ln dampens runaway).
//! - Decisions/feedback get a floor so critical history never vanishes.

use chrono::{DateTime, Utc};

use crate::event::MemoryType;

/// Decay half-life. After this many days untouched, recency factor = 0.5.
pub const HALF_LIFE_DAYS: f32 = 30.0;

/// Per-access boost coefficient (applied to ln(1+access_count)).
pub const ACCESS_BOOST_ALPHA: f32 = 0.15;

/// Hard floor for high-signal memory types — they never decay below this.
pub const DECISION_FLOOR: f32 = 0.30;
pub const FEEDBACK_FLOOR: f32 = 0.40;
pub const SECURITY_FLOOR: f32 = 0.50;

/// Compute effective importance.
///
/// - `base_importance`: stored static score (0..1)
/// - `last_active`: most recent of (last_accessed_at, timestamp)
/// - `access_count`: how many times this memory has been retrieved
/// - `memory_type`: drives the floor
pub fn effective_score(
    base_importance: f32,
    last_active: DateTime<Utc>,
    access_count: u32,
    memory_type: &MemoryType,
    now: DateTime<Utc>,
) -> f32 {
    let recency = recency_factor(last_active, now);
    let access = access_boost(access_count);
    let raw = (base_importance * recency * access).clamp(0.0, 1.0);
    raw.max(floor_for(memory_type))
}

/// Exponential decay: 1.0 at now, 0.5 at +HALF_LIFE_DAYS, → 0.
fn recency_factor(last_active: DateTime<Utc>, now: DateTime<Utc>) -> f32 {
    let hours = (now - last_active).num_minutes() as f32 / 60.0;
    let days = (hours / 24.0).max(0.0);
    // 2^(-days/half_life) == exp(-ln2 * days / half_life)
    (-std::f32::consts::LN_2 * days / HALF_LIFE_DAYS).exp()
}

/// Sublinear boost: untouched=1.0, 1 access=1.10, 5≈1.27, 20≈1.46, 100≈1.69.
/// Capped to keep effective <= 1.0 (via clamp upstream).
fn access_boost(access_count: u32) -> f32 {
    1.0 + ACCESS_BOOST_ALPHA * (1.0 + access_count as f32).ln()
}

/// Per-type floor — critical types never tank below this.
fn floor_for(memory_type: &MemoryType) -> f32 {
    match memory_type {
        MemoryType::Feedback => FEEDBACK_FLOOR,
        MemoryType::Decision => DECISION_FLOOR,
        MemoryType::Security => SECURITY_FLOOR,
        MemoryType::SessionSummary | MemoryType::Note => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn t(days_ago: i64) -> DateTime<Utc> {
        Utc::now() - Duration::days(days_ago)
    }

    #[test]
    fn recency_factor_at_now_is_one() {
        let now = Utc::now();
        assert!((recency_factor(now, now) - 1.0).abs() < 1e-3);
    }

    #[test]
    fn recency_factor_halves_at_half_life() {
        let now = Utc::now();
        let old = now - Duration::days(HALF_LIFE_DAYS as i64);
        let r = recency_factor(old, now);
        assert!(
            (r - 0.5).abs() < 0.05,
            "expected ~0.5 at half-life, got {r}"
        );
    }

    #[test]
    fn recency_factor_near_zero_at_six_months() {
        let now = Utc::now();
        let very_old = now - Duration::days(180);
        let r = recency_factor(very_old, now);
        assert!(r < 0.05, "expected near-zero at 6mo, got {r}");
    }

    #[test]
    fn access_boost_monotonic() {
        let b0 = access_boost(0);
        let b1 = access_boost(1);
        let b10 = access_boost(10);
        let b100 = access_boost(100);
        assert!(b0 < b1, "boost must grow with access");
        assert!(b1 < b10);
        assert!(b10 < b100);
        assert!((b0 - 1.0).abs() < 1e-6, "no access => no boost");
    }

    #[test]
    fn untouched_old_note_decays() {
        let now = Utc::now();
        let score = effective_score(0.6, t(90), 0, &MemoryType::Note, now);
        // 90d ago, 3 half-lives: 0.6 * 0.125 * 1.0 ≈ 0.075
        assert!(score < 0.15, "old untouched note should decay, got {score}");
    }

    #[test]
    fn touched_recent_note_stays_high() {
        let now = Utc::now();
        let score = effective_score(0.6, t(1), 5, &MemoryType::Note, now);
        assert!(
            score > 0.5,
            "recent touched note should stay high, got {score}"
        );
    }

    #[test]
    fn decision_floor_protects_old_decisions() {
        let now = Utc::now();
        // Very old, never accessed
        let score = effective_score(0.6, t(365), 0, &MemoryType::Decision, now);
        assert!(
            score >= DECISION_FLOOR,
            "decision must respect floor, got {score}"
        );
    }

    #[test]
    fn feedback_floor_higher_than_decision() {
        let now = Utc::now();
        let s_fb = effective_score(0.1, t(365), 0, &MemoryType::Feedback, now);
        let s_dec = effective_score(0.1, t(365), 0, &MemoryType::Decision, now);
        assert!(s_fb > s_dec);
        assert!(s_fb >= FEEDBACK_FLOOR);
    }

    #[test]
    fn clamps_to_unit_interval() {
        let now = Utc::now();
        // Artificially huge base + access count
        let score = effective_score(2.0, now, 10_000, &MemoryType::Note, now);
        assert!(score <= 1.0 + 1e-6);
        assert!(score >= 0.0);
    }
}
