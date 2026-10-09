//! Aggregate evidence from explicitly named plan roots, without a profile.
use super::{Input, Reader};
use anyhow::{Result, ensure};
use std::path::Path;

pub fn read(root: &Path) -> Result<serde_json::Value> {
    ensure!(
        root.is_absolute() && root.is_dir(),
        "plan root must be an existing absolute directory"
    );
    let plan = Reader::default().read(
        &Input {
            main: root.to_path_buf(),
            config: super::Config::default(),
            checkouts: Vec::new(),
        },
        chrono::Utc::now(),
    );
    let units: Vec<_> = plan
        .units
        .iter()
        .map(|unit| {
            serde_json::json!({
                "id": unit.id, "done": unit.done, "total": unit.total,
                "provider": unit.provider, "counts": unit.counts,
                "weekly_count": unit.weekly_count,
                "waiting_on_owner": unit.waiting_on_owner.len(),
                "stage": unit.stage, "error": unit.error,
            })
        })
        .collect();
    Ok(serde_json::json!({"units": units, "message": plan.message}))
}
