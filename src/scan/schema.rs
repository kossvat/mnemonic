//! The tables and columns the scanner knows, by store. A table or a
//! column that is not here is not read and not named: it is counted, and
//! the scan is incomplete.

/// How the values of a column are read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    /// Narrative.
    Text,
    /// What names something: judged as it is and in lower case.
    Name,
    /// A JSON document, decoded and walked field by field.
    Json,
    /// A number, a time or a word of a fixed set. A text found in one is
    /// read like any text.
    Plain,
    /// A vector. Its bytes are not text and are not scanned.
    Vector,
    /// What the full-text index keeps for itself: terms and positions in
    /// a binary form. Its bytes are not scanned; the text the index was
    /// built from is, where it is kept.
    Index,
}

impl Column {
    pub fn as_str(self) -> &'static str {
        match self {
            Column::Text => "text",
            Column::Name => "name",
            Column::Json => "json",
            Column::Plain => "plain",
            Column::Vector => "vector_not_scanned",
            Column::Index => "index_not_scanned",
        }
    }
}

/// How a row is pointed at without saying what it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locator {
    /// SQLite's own row number.
    RowId,
    /// The table has none: the position of the row in the order the
    /// table is read in, which is the order of its key, counted from one.
    Ordinal,
}

impl Locator {
    pub fn as_str(self) -> &'static str {
        match self {
            Locator::RowId => "rowid",
            Locator::Ordinal => "ordinal",
        }
    }
}

pub struct Table {
    pub name: &'static str,
    pub locator: Locator,
    pub columns: &'static [(&'static str, Column)],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Store {
    Private,
    Shared,
    Activity,
}

impl Store {
    pub fn as_str(self) -> &'static str {
        match self {
            Store::Private => "private",
            Store::Shared => "shared",
            Store::Activity => "activity",
        }
    }

    /// The table that tells the store.
    pub fn signature(self) -> &'static str {
        match self {
            Store::Private => "memories",
            Store::Shared => "shared_records",
            Store::Activity => "work_sessions",
        }
    }

    /// The schema generations (`PRAGMA user_version`) the lists below
    /// were written for.
    pub fn versions(self) -> std::ops::RangeInclusive<i64> {
        match self {
            Store::Private => 0..=2,
            Store::Shared => 1..=2,
            Store::Activity => 0..=0,
        }
    }

    pub fn tables(self) -> &'static [Table] {
        match self {
            Store::Private => PRIVATE,
            Store::Shared => SHARED,
            Store::Activity => ACTIVITY,
        }
    }

    /// The table of that name, of the store or of SQLite itself.
    pub fn table(self, name: &str) -> Option<&'static Table> {
        self.tables()
            .iter()
            .chain(SQLITE)
            .find(|table| table.name == name)
    }

    pub const ALL: [Store; 3] = [Store::Private, Store::Shared, Store::Activity];
}

use Column::{Index, Json, Name, Plain, Text, Vector};

/// The schema itself: what every object of the file is called and the
/// statement it was made by. A statement is text like any other: a
/// default, a check, the body of a trigger or of a view can hold what a
/// row can.
pub const SCHEMA: Table = Table {
    name: "sqlite_schema",
    locator: RowId,
    columns: &[
        ("type", Plain),
        ("name", Name),
        ("tbl_name", Name),
        ("rootpage", Plain),
        ("sql", Text),
    ],
};

/// The tables SQLite keeps for itself in a database of any store. One it
/// keeps samples of rows in (`sqlite_stat4` and its like) is not here:
/// the samples are bytes, and the table is an unknown one.
const SQLITE: &[Table] = &[
    Table {
        name: "sqlite_sequence",
        locator: RowId,
        columns: &[("name", Name), ("seq", Plain)],
    },
    Table {
        name: "sqlite_stat1",
        locator: RowId,
        columns: &[("tbl", Name), ("idx", Name), ("stat", Plain)],
    },
];

/// The columns a table has that are not of its rows: those the full-text
/// index answers queries by.
pub fn hidden(table: &str) -> &'static [&'static str] {
    match table {
        "memories_fts" => &["memories_fts", "rank"],
        _ => &[],
    }
}
use Locator::{Ordinal, RowId};

const PRIVATE: &[Table] = &[
    Table {
        name: "memories",
        locator: RowId,
        columns: &[
            ("id", Name),
            ("timestamp", Plain),
            ("title", Text),
            ("content", Text),
            ("memory_type", Plain),
            ("tags", Json),
            ("source", Json),
            ("importance", Plain),
            ("metadata", Json),
            ("embedding", Vector),
            ("created_at", Plain),
            ("session_id", Name),
            ("access_count", Plain),
            ("last_accessed_at", Plain),
            ("superseded_by", Name),
            ("canonical_memory_id", Name),
        ],
    },
    // The index reads its text from `memories`: the same tags, which are
    // a document there and here.
    Table {
        name: "memories_fts",
        locator: RowId,
        columns: &[("title", Text), ("content", Text), ("tags", Json)],
    },
    // The tables the index keeps for itself, by the columns the index
    // makes them with. A table of one of these names with another column
    // has an unknown column, like any table; a text where the index
    // keeps bytes is read like any text.
    Table {
        name: "memories_fts_data",
        locator: RowId,
        columns: &[("id", Plain), ("block", Index)],
    },
    Table {
        name: "memories_fts_idx",
        locator: Ordinal,
        columns: &[("segid", Plain), ("term", Index), ("pgno", Plain)],
    },
    Table {
        name: "memories_fts_docsize",
        locator: RowId,
        columns: &[("id", Plain), ("sz", Index)],
    },
    Table {
        name: "memories_fts_config",
        locator: Ordinal,
        columns: &[("k", Plain), ("v", Plain)],
    },
    Table {
        name: "conclusions",
        locator: RowId,
        columns: &[
            ("id", Name),
            ("subject", Name),
            ("kind", Name),
            ("statement", Text),
            ("confidence", Plain),
            ("support_count", Plain),
            ("created_at", Plain),
            ("last_evaluated_at", Plain),
            ("superseded_by", Name),
        ],
    },
    Table {
        name: "conclusion_sources",
        locator: RowId,
        columns: &[("conclusion_id", Name), ("memory_id", Name)],
    },
    Table {
        name: "consumer_receipts",
        locator: RowId,
        columns: &[
            ("consumer", Name),
            ("event_id", Plain),
            ("outcome", Plain),
            ("memory_id", Name),
        ],
    },
    Table {
        name: "decision_conflicts",
        locator: RowId,
        columns: &[
            ("old_id", Name),
            ("new_id", Name),
            ("project", Name),
            ("status", Plain),
            ("confidence", Plain),
            ("reason", Text),
            ("checked_at", Plain),
            ("checker_version", Plain),
        ],
    },
    Table {
        name: "edges",
        locator: RowId,
        columns: &[
            ("id", Name),
            ("source_entity", Name),
            ("target_entity", Name),
            ("relation", Name),
            ("memory_id", Name),
            ("weight", Plain),
            ("timestamp", Plain),
        ],
    },
    Table {
        name: "entities",
        locator: RowId,
        columns: &[
            ("id", Name),
            ("name", Name),
            ("entity_type", Name),
            ("mention_count", Plain),
            ("first_seen", Plain),
            ("last_seen", Plain),
        ],
    },
    Table {
        name: "entity_aliases",
        locator: RowId,
        columns: &[("alias", Name), ("canonical", Name), ("merged_at", Plain)],
    },
    Table {
        name: "extraction_queue",
        locator: RowId,
        columns: &[
            ("memory_id", Name),
            ("enqueued_at", Plain),
            ("attempts", Plain),
        ],
    },
    Table {
        name: "fact_events",
        locator: RowId,
        columns: &[
            ("id", Plain),
            ("slot_id", Name),
            ("value_id", Name),
            ("action", Plain),
            ("outcome", Plain),
            ("revision_after", Plain),
            ("actor", Name),
            ("agent", Name),
            ("evidence_memory_id", Name),
            ("occurred_at", Plain),
            ("request_id", Name),
        ],
    },
    Table {
        name: "fact_slots",
        locator: RowId,
        columns: &[
            ("id", Name),
            ("scope_key", Name),
            ("scope", Name),
            ("subject_key", Name),
            ("subject", Name),
            ("predicate", Name),
            ("predicate_label", Name),
            ("qualifier_key", Name),
            ("qualifier", Name),
            ("revision", Plain),
            ("created_at", Plain),
            ("updated_at", Plain),
        ],
    },
    Table {
        name: "fact_values",
        locator: RowId,
        columns: &[
            ("seq", Plain),
            ("id", Name),
            ("slot_id", Name),
            ("kind", Plain),
            ("value", Text),
            ("value_norm", Text),
            ("status", Plain),
            ("trust", Plain),
            ("valid_from", Plain),
            ("valid_from_ms", Plain),
            ("asserted_ms", Plain),
            ("confidence", Plain),
            ("source_memory_id", Name),
            ("source_forgotten_at", Plain),
            ("evidence", Text),
            ("actor", Name),
            ("agent", Name),
            ("extractor", Name),
            ("legacy_fact_id", Name),
            ("legacy_valid_to", Plain),
            ("reconfirmed_at", Plain),
            ("reconfirm_count", Plain),
            ("created_at", Plain),
        ],
    },
    Table {
        name: "facts",
        locator: RowId,
        columns: &[
            ("id", Name),
            ("subject", Name),
            ("predicate", Name),
            ("value", Text),
            ("valid_from", Plain),
            ("valid_to", Plain),
            ("confidence", Plain),
            ("source_memory_id", Name),
            ("created_at", Plain),
        ],
    },
    Table {
        name: "followup_events",
        locator: RowId,
        columns: &[
            ("id", Plain),
            ("followup_id", Name),
            ("action", Plain),
            ("evidence_memory_id", Name),
            ("actor", Name),
            ("occurred_at", Plain),
            ("request_id", Name),
        ],
    },
    Table {
        name: "followups",
        locator: RowId,
        columns: &[
            ("id", Name),
            ("project", Name),
            ("project_key", Name),
            ("title", Text),
            ("status", Plain),
            ("source_memory_id", Name),
            ("revision", Plain),
            ("created_at", Plain),
            ("updated_at", Plain),
        ],
    },
    Table {
        name: "ingest_cursors",
        locator: RowId,
        columns: &[
            ("stream", Name),
            ("generation", Plain),
            ("offset", Plain),
            ("file_id", Name),
            ("prefix_len", Plain),
            ("prefix_hash", Name),
            ("anchor_hash", Name),
        ],
    },
    Table {
        name: "ingest_events",
        locator: RowId,
        columns: &[
            ("seq", Plain),
            ("source_key", Name),
            ("source_at", Plain),
            ("observed_at", Plain),
            ("schema_version", Plain),
            ("payload", Json),
        ],
    },
    Table {
        name: "ingest_facts",
        locator: RowId,
        columns: &[("name", Name), ("value", Text)],
    },
    Table {
        name: "llm_extraction_cache",
        locator: RowId,
        columns: &[
            ("content_hash", Name),
            ("extractor_id", Name),
            ("result_json", Json),
            ("created_at", Plain),
        ],
    },
    Table {
        name: "memory_entities",
        locator: RowId,
        columns: &[("memory_id", Name), ("entity_id", Name)],
    },
    Table {
        name: "memory_peers",
        locator: RowId,
        columns: &[("memory_id", Name), ("peer_id", Name), ("role", Name)],
    },
    Table {
        name: "memory_reaffirmed",
        locator: Ordinal,
        columns: &[("memory_id", Name), ("at", Plain)],
    },
    Table {
        name: "memory_updates",
        locator: Ordinal,
        columns: &[
            ("new_id", Name),
            ("old_id", Name),
            ("status", Plain),
            ("rule", Plain),
            ("rule_version", Plain),
            ("value_class", Plain),
            ("was_values", Json),
            ("now_values", Json),
            ("similarity", Plain),
            ("actor", Name),
            ("created_at", Plain),
        ],
    },
    Table {
        name: "peers",
        locator: RowId,
        columns: &[
            ("id", Name),
            ("name", Name),
            ("display_name", Text),
            ("kind", Name),
            ("created_at", Plain),
            ("last_seen_at", Plain),
        ],
    },
    Table {
        name: "pending_extractions",
        locator: RowId,
        columns: &[
            ("memory_id", Name),
            ("attempts", Plain),
            ("last_error", Text),
            ("last_attempt_at", Plain),
            ("next_attempt_at", Plain),
            ("created_at", Plain),
        ],
    },
    Table {
        name: "reflection_runs",
        locator: RowId,
        columns: &[
            ("id", Name),
            ("run_at", Plain),
            ("mode", Plain),
            ("threshold", Plain),
            ("clusters_found", Plain),
            ("applied_count", Plain),
            ("synthesizer", Plain),
        ],
    },
    Table {
        name: "reflection_sources",
        locator: RowId,
        columns: &[
            ("canonical_id", Name),
            ("source_id", Name),
            ("run_id", Name),
            ("cosine", Plain),
            ("position", Plain),
        ],
    },
    Table {
        name: "sessions",
        locator: RowId,
        columns: &[
            ("id", Name),
            ("peer_id", Name),
            ("label", Text),
            ("started_at", Plain),
            ("ended_at", Plain),
            ("source", Name),
            ("external_key", Name),
            ("last_activity_at", Plain),
        ],
    },
    Table {
        name: "vector_index_changes",
        locator: RowId,
        columns: &[("memory_id", Name), ("revision", Plain)],
    },
    Table {
        name: "vector_index_state",
        locator: RowId,
        columns: &[("id", Plain), ("revision", Plain)],
    },
];

const SHARED: &[Table] = &[
    Table {
        name: "shared_projects",
        locator: RowId,
        columns: &[("project_id", Name), ("revision", Plain)],
    },
    Table {
        name: "shared_records",
        locator: RowId,
        columns: &[
            ("project_id", Name),
            ("key", Name),
            ("title", Text),
            ("content", Text),
            ("source", Name),
            ("revision", Plain),
            ("updated_at", Plain),
            ("published_by", Name),
            ("origin_observation_id", Name),
            ("origin_writer_id", Name),
        ],
    },
    Table {
        name: "shared_observations",
        locator: RowId,
        columns: &[
            ("id", Name),
            ("project_id", Name),
            ("writer_id", Name),
            ("request_id", Name),
            ("title", Text),
            ("content", Text),
            ("source", Name),
            ("status", Plain),
            ("created_at", Plain),
            ("principal_id", Name),
            ("reviewed_by", Name),
            ("reviewed_at", Plain),
            ("promoted_key", Name),
            ("promoted_revision", Plain),
        ],
    },
    Table {
        name: "shared_revocations",
        locator: RowId,
        columns: &[
            ("project_id", Name),
            ("principal_id", Name),
            ("revoked_at", Plain),
            ("revoked_by", Name),
        ],
    },
    Table {
        name: "shared_meta",
        locator: RowId,
        columns: &[("id", Plain), ("pinned_project", Name)],
    },
];

const ACTIVITY: &[Table] = &[
    Table {
        name: "work_sessions",
        locator: RowId,
        columns: &[
            ("id", Name),
            ("started_at", Plain),
            ("ended_at", Plain),
            ("open", Plain),
        ],
    },
    Table {
        name: "project_attribution",
        locator: RowId,
        columns: &[
            ("day", Plain),
            ("project_key", Name),
            ("seconds", Plain),
            ("confidence", Plain),
        ],
    },
    Table {
        name: "unattributed_time",
        locator: RowId,
        columns: &[("day", Plain), ("seconds", Plain)],
    },
];
