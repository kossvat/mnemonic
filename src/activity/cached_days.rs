//! Existing derived dates only: an undo must not invent a historical range.
use super::*;

impl ActivityStore {
    pub(crate) fn cached_attribution_days(&self) -> Result<Vec<NaiveDate>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("activity lock unavailable"))?;
        let mut query = conn.prepare(
            "SELECT day FROM project_attribution UNION SELECT day FROM unattributed_time ORDER BY day",
        )?;
        let dates = query
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(dates
            .into_iter()
            .filter_map(|day| NaiveDate::parse_from_str(&day, "%Y-%m-%d").ok())
            .collect())
    }
}
