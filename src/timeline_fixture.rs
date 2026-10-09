//! Synthetic stores shared by the timeline value and guarded-route tests.
use crate::activity::ActivityStore;
use crate::owner_time_day::Calendar;
use crate::storage::Storage;
use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

pub struct Zone {
    pub offset: i64,
    pub turn: Option<(DateTime<Utc>, i64)>,
}
impl Zone {
    pub const UTC: Self = Self {
        offset: 0,
        turn: None,
    };
    fn offset_at(&self, at: DateTime<Utc>) -> i64 {
        self.turn
            .filter(|(turn, _)| at >= *turn)
            .map_or(self.offset, |(_, later)| later)
    }
    fn start(&self, day: NaiveDate) -> DateTime<Utc> {
        let local = day.and_hms_opt(0, 0, 0).unwrap().and_utc();
        [Some(self.offset), self.turn.map(|(_, later)| later)]
            .into_iter()
            .flatten()
            .map(|offset| local - Duration::seconds(offset))
            .find(|at| self.date_of(*at) == day && self.date_of(*at - Duration::seconds(1)) != day)
            .unwrap()
    }
}
impl Calendar for Zone {
    fn bounds(&self, day: NaiveDate) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
        Some((self.start(day), self.start(day.succ_opt()?)))
    }
    fn date_of(&self, at: DateTime<Utc>) -> NaiveDate {
        (at + Duration::seconds(self.offset_at(at))).date_naive()
    }
}

pub fn at(hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 29, hour, minute, 0).unwrap()
}

pub struct Fixture {
    _temp: tempfile::TempDir,
    pub storage: Arc<Storage>,
    pub activity: Arc<ActivityStore>,
    pub memory_path: PathBuf,
    pub activity_path: PathBuf,
    pub demo: i64,
    pub notes: i64,
}
impl Fixture {
    pub fn empty() -> Self {
        let temp = crate::test_support::temp_dir("mn-timeline-");
        let memory_path = temp.path().join("memory.db");
        let activity_path = temp.path().join("activity.db");
        let storage = Arc::new(Storage::open(&memory_path).unwrap());
        let activity = Arc::new(ActivityStore::open(&activity_path).unwrap());
        let demo = storage.project_upsert("demoapp", "Demo app", None).unwrap();
        let notes = storage.project_upsert("notes", "Notes", None).unwrap();
        Self {
            _temp: temp,
            storage,
            activity,
            memory_path,
            activity_path,
            demo,
            notes,
        }
    }
    pub fn full() -> Self {
        let fixture = Self::empty();
        fixture.segment(
            "fixture-claude",
            "claude",
            false,
            Some(fixture.demo),
            at(9, 0),
            at(10, 0),
        );
        fixture.segment(
            "fixture-codex",
            "codex",
            false,
            Some(fixture.notes),
            at(9, 5),
            at(9, 45),
        );
        fixture.segment(
            "fixture-point",
            "codex",
            false,
            Some(fixture.demo),
            at(9, 40),
            at(9, 40),
        );
        fixture.segment(
            "fixture-program",
            "other",
            true,
            None,
            at(13, 0),
            at(13, 15),
        );
        fixture.prompt("fixture-claude", at(9, 0));
        fixture.prompt("fixture-codex", at(9, 5));
        fixture.active(at(9, 0), at(9, 20));
        fixture.active(at(10, 30), at(10, 40));
        fixture.active(
            at(11, 0) + Duration::milliseconds(250),
            at(11, 5) + Duration::milliseconds(750),
        );
        fixture
            .storage
            .override_add(None, Some((at(10, 30), at(10, 40))), None)
            .unwrap();
        fixture.memory("fallback", at(11, 10), fixture.demo);
        let conn = fixture.storage.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO recall_events
            (id, at, surface, tool, client, run, returned, corrections, chars, hidden)
            VALUES (1, ?1, 'mcp', 'memory_search', NULL, 'fixture', 1, 0, 20, 0)",
            [at(12, 0).to_rfc3339()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO recall_items (event_id, rank, memory_id, section)
            VALUES (1, 0, 'fallback', NULL)",
            [],
        )
        .unwrap();
        drop(conn);
        fixture
    }
    pub fn active(&self, from: DateTime<Utc>, to: DateTime<Utc>) {
        let conn = rusqlite::Connection::open(&self.activity_path).unwrap();
        conn.execute(
            "INSERT INTO work_sessions (id, started_at, ended_at, open) VALUES (?1, ?2, ?3, 0)",
            rusqlite::params![
                uuid::Uuid::new_v4().to_string(),
                from.to_rfc3339(),
                to.to_rfc3339()
            ],
        )
        .unwrap();
    }
    pub fn segment(
        &self,
        stream: &str,
        agent: &str,
        program: bool,
        project: Option<i64>,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> i64 {
        let conn = self.storage.conn.lock().unwrap();
        conn.execute("INSERT INTO work_segments
            (stream, driver, agent, project_id, started_at, last_at, source) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'path')",
            rusqlite::params![stream, if program { "program" } else { "owner" }, agent, project,
                from.format("%Y-%m-%d %H:%M:%S").to_string(), to.format("%Y-%m-%d %H:%M:%S").to_string()]).unwrap();
        let id = conn.last_insert_rowid();
        conn.execute("INSERT INTO segment_activity (segment_id, minute, records, touches) VALUES (?1, ?2, 1, 1)",
            rusqlite::params![id, from.timestamp().div_euclid(60)]).unwrap();
        id
    }
    pub fn prompt(&self, stream: &str, time: DateTime<Utc>) {
        self.storage
            .conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO owner_prompts (stream, at) VALUES (?1, ?2)",
                rusqlite::params![stream, time.format("%Y-%m-%d %H:%M:%S").to_string()],
            )
            .unwrap();
    }
    pub fn memory(&self, id: &str, time: DateTime<Utc>, project: i64) {
        let name = self
            .storage
            .projects()
            .unwrap()
            .into_iter()
            .find(|row| row.id == project)
            .unwrap()
            .name;
        let entity = self
            .storage
            .upsert_entity(&crate::graph::Entity {
                name: name.clone(),
                entity_type: crate::graph::EntityType::Project,
            })
            .unwrap();
        self.storage.project_link_entity(project, &name).unwrap();
        let conn = self.storage.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO memories (id, timestamp, title, content, memory_type, source)
            VALUES (?1, ?2, 'Fixture note', 'Synthetic content', 'note', 'test')",
            rusqlite::params![id, time.to_rfc3339()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO memory_entities (memory_id, entity_id) VALUES (?1, ?2)",
            rusqlite::params![id, entity],
        )
        .unwrap();
    }
    pub fn changes(&self) -> u64 {
        self.storage.conn.lock().unwrap().total_changes()
    }
    pub fn stores(&self) -> Vec<BTreeMap<String, Vec<String>>> {
        let activity = rusqlite::Connection::open_with_flags(
            &self.activity_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        vec![dump(&self.storage.conn.lock().unwrap()), dump(&activity)]
    }
    pub fn files(&self) -> Vec<Option<Vec<u8>>> {
        [&self.memory_path, &self.activity_path]
            .into_iter()
            .flat_map(|path| {
                [
                    path.clone(),
                    PathBuf::from(format!("{}-wal", path.to_string_lossy())),
                ]
            })
            .map(|path| std::fs::read(path).ok())
            .collect()
    }
}

fn dump(conn: &rusqlite::Connection) -> BTreeMap<String, Vec<String>> {
    let mut names = conn
        .prepare("SELECT name FROM sqlite_schema WHERE type = 'table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    names.push("sqlite_schema".into());
    names
        .into_iter()
        .map(|name| {
            let quoted = name.replace('"', "\"\"");
            let mut statement = conn
                .prepare(&format!("SELECT * FROM \"{quoted}\""))
                .unwrap();
            let columns = statement.column_count();
            let mut values = statement
                .query_map([], |row| {
                    let values = (0..columns)
                        .map(|column| row.get::<_, rusqlite::types::Value>(column))
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    Ok(format!("{values:?}"))
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            values.sort();
            (name, values)
        })
        .collect()
}
