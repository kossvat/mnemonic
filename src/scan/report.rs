//! What a scan says: the policy it judged by, what it covered, and where
//! it found what class of thing. Every name in it is one the scanner
//! knew before it opened the file, and every place is a number: it holds
//! no id, no key, no value and no path from the snapshot.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::redaction::Counts;

/// Why a scan is not complete. The codes are fixed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reason {
    UnsupportedFormat,
    InvalidPath,
    NotFound,
    NotARegularFile,
    SidecarPresent,
    NotADatabase,
    UnsupportedSchema,
    UnsupportedSchemaVersion,
    UnknownTable,
    UnknownColumn,
    UndecodableValue,
    StructureLimit,
    ReadFailed,
    ChangedDuringScan,
}

impl Reason {
    pub fn code(self) -> &'static str {
        match self {
            Reason::UnsupportedFormat => "UNSUPPORTED_FORMAT",
            Reason::InvalidPath => "INVALID_PATH",
            Reason::NotFound => "NOT_FOUND",
            Reason::NotARegularFile => "NOT_A_REGULAR_FILE",
            Reason::SidecarPresent => "SIDECAR_PRESENT",
            Reason::NotADatabase => "NOT_A_DATABASE",
            Reason::UnsupportedSchema => "UNSUPPORTED_SCHEMA",
            Reason::UnsupportedSchemaVersion => "UNSUPPORTED_SCHEMA_VERSION",
            Reason::UnknownTable => "UNKNOWN_TABLE",
            Reason::UnknownColumn => "UNKNOWN_COLUMN",
            Reason::UndecodableValue => "UNDECODABLE_VALUE",
            Reason::StructureLimit => "STRUCTURE_LIMIT",
            Reason::ReadFailed => "READ_FAILED",
            Reason::ChangedDuringScan => "CHANGED_DURING_SCAN",
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Finding {
    pub table: &'static str,
    pub column: &'static str,
    /// The row number, or for a table without one the position of the
    /// row in this snapshot (see the table's `locator`).
    pub row: i64,
    pub classes: BTreeMap<&'static str, u32>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TableCoverage {
    pub table: &'static str,
    /// `rowid`, or `ordinal` for a table without row numbers.
    pub locator: &'static str,
    pub rows: u64,
    /// How each column was read.
    pub columns: BTreeMap<&'static str, &'static str>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Coverage {
    pub tables: Vec<TableCoverage>,
    /// Counted, never named.
    pub unknown_tables: u64,
    pub unknown_columns: u64,
    pub undecodable_values: u64,
    pub structures_over_limit: u64,
}

/// Findings listed one by one. Past this only the totals grow.
pub const MAX_LISTED: usize = 10_000;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Report {
    pub policy_version: u32,
    /// Whether everything in the snapshot was read. A scan that is not
    /// complete says nothing of what it did not read.
    pub complete: bool,
    pub reasons: Vec<&'static str>,
    pub store: Option<&'static str>,
    pub schema_version: Option<i64>,
    pub coverage: Coverage,
    pub findings: Vec<Finding>,
    /// Findings counted in the totals and not listed.
    pub findings_not_listed: u64,
    pub totals: BTreeMap<&'static str, u64>,
    #[serde(skip)]
    reasons_seen: std::collections::BTreeSet<Reason>,
}

impl Report {
    pub fn new() -> Self {
        Self {
            policy_version: crate::redaction::POLICY_VERSION,
            complete: true,
            reasons: Vec::new(),
            store: None,
            schema_version: None,
            coverage: Coverage::default(),
            findings: Vec::new(),
            findings_not_listed: 0,
            totals: BTreeMap::new(),
            reasons_seen: std::collections::BTreeSet::new(),
        }
    }

    /// A report of a scan that could not be made.
    pub fn refused(reason: Reason) -> Self {
        let mut report = Self::new();
        report.incomplete(reason);
        report
    }

    pub fn incomplete(&mut self, reason: Reason) {
        self.complete = false;
        if self.reasons_seen.insert(reason) {
            self.reasons.push(reason.code());
        }
    }

    pub fn found(&mut self, table: &'static str, column: &'static str, row: i64, counts: &Counts) {
        if counts.is_empty() {
            return;
        }
        let mut classes = BTreeMap::new();
        for (class, n) in counts.iter() {
            classes.insert(class.as_str(), n);
            *self.totals.entry(class.as_str()).or_insert(0) += u64::from(n);
        }
        if self.findings.len() < MAX_LISTED {
            self.findings.push(Finding {
                table,
                column,
                row,
                classes,
            });
        } else {
            self.findings_not_listed += 1;
        }
    }

    pub fn has_findings(&self) -> bool {
        !self.totals.is_empty()
    }

    /// 0: everything was read and nothing was found. 2: everything was
    /// read and something was found. 1: not everything was read, whatever
    /// was found.
    pub fn exit_code(&self) -> i32 {
        match (self.complete, self.has_findings()) {
            (false, _) => 1,
            (true, true) => 2,
            (true, false) => 0,
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{\"complete\": false}".into())
    }
}

impl Default for Report {
    fn default() -> Self {
        Self::new()
    }
}
