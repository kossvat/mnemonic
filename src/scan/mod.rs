//! `mnemonic redact scan`: what the redaction policy recognizes in a
//! database that was written before it, or by something else.
//!
//! The scan is of a closed snapshot, a file the caller names and nothing
//! has open. It reads that one file and changes nothing: no profile is
//! selected, no configuration, store, model or sink is loaded, the file
//! is opened so that SQLite writes no journal and takes no lock, and
//! nothing is migrated, repaired or removed. It is a report, not a
//! remedy.
//!
//! The report says where (a table and a column the scanner knows, and a
//! row by its number) and what class of thing, never what: no id, no
//! key, no value and no path of the snapshot is in it. A table or a
//! column the scanner does not know is not read; it is counted, and the
//! scan is then not complete, whatever it found.

mod report;
mod schema;
mod snapshot;
#[cfg(test)]
mod tests;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use rusqlite::Connection;
use rusqlite::types::ValueRef;

use crate::redaction::{Counts, Limits, inspect_json, parse_strict, redact_text};
use report::TableCoverage;
pub use report::{Reason, Report};
use schema::{Column, Locator, SCHEMA, Store, Table};
use snapshot::Snapshot;

#[derive(Subcommand)]
pub enum RedactCommands {
    /// Report what the redaction policy recognizes in a closed snapshot
    /// of a database. Reads the one file and changes nothing.
    Scan(ScanArgs),
}

#[derive(Args)]
pub struct ScanArgs {
    /// The snapshot: a database file that nothing has open
    #[arg(long, value_name = "PATH")]
    pub db: PathBuf,
    /// The format of the report
    #[arg(long, default_value = "json")]
    pub format: String,
}

/// What a command line that is the scanner's and cannot be read is
/// answered with. The arguments are not said back: one of them is a path.
pub const USAGE: &str = "usage: mnemonic redact scan --db <PATH> [--format json]";

/// Whether a command line (without the program's name) asks for the
/// scanner: its first word, past the global `--home`, is `redact`. The
/// words are compared as bytes: a directory's name need not be text.
pub fn is_scan_invocation(args: impl IntoIterator<Item = OsString>) -> bool {
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let word = arg.as_encoded_bytes();
        if word == b"--home" {
            args.next();
        } else if !word.starts_with(b"--home=") {
            return word == b"redact";
        }
    }
    false
}

/// Run the command: the report on the standard output, and the exit code.
pub fn run(command: &RedactCommands) -> i32 {
    let RedactCommands::Scan(args) = command;
    let report = if args.format == "json" {
        scan(&args.db)
    } else {
        Report::refused(Reason::UnsupportedFormat)
    };
    println!("{}", report.to_json());
    report.exit_code()
}

/// Scan the snapshot at `path`.
pub fn scan(path: &Path) -> Report {
    scan_then(path, || {})
}

/// [`scan`], with `then` run once the snapshot is read and before it is
/// compared with what it was: where a test changes it.
fn scan_then(path: &Path, then: impl FnOnce()) -> Report {
    let (snapshot, conn) = match Snapshot::open(path) {
        Ok(opened) => opened,
        Err(reason) => return Report::refused(reason),
    };
    let mut report = Report::new();
    if let Err(reason) = read(&conn, &mut report) {
        report.incomplete(reason);
    }
    drop(conn);
    then();
    // A snapshot that changed while it was read was not closed: what was
    // read of it is no report of it.
    if !snapshot.unchanged() {
        report.incomplete(Reason::ChangedDuringScan);
    }
    report
}

fn failed(_: rusqlite::Error) -> Reason {
    Reason::ReadFailed
}

fn read(conn: &Connection, report: &mut Report) -> Result<(), Reason> {
    // Every object of the file, SQLite's own included: what is left out
    // here is never looked at.
    let mut objects: Vec<(String, String)> = {
        let mut stmt = conn
            .prepare("SELECT type, name FROM sqlite_schema")
            .map_err(failed)?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(failed)?;
        rows.collect::<Result<_, _>>().map_err(failed)?
    };
    // In one order whatever the file's: sorted here, not by SQLite.
    objects.sort_by(|a, b| a.1.cmp(&b.1));
    let has = |name: &str| objects.iter().any(|(kind, n)| kind == "table" && n == name);
    let store = Store::ALL
        .into_iter()
        .find(|store| has(store.signature()))
        .ok_or(Reason::UnsupportedSchema)?;
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(failed)?;
    report.store = Some(store.as_str());
    report.schema_version = Some(version);
    if !store.versions().contains(&version) {
        report.incomplete(Reason::UnsupportedSchemaVersion);
    }

    scan_table(conn, &SCHEMA, report)?;
    for (kind, name) in &objects {
        match kind.as_str() {
            // An index holds copies of columns that are read where they
            // are; a trigger holds no row. The statement either was made
            // by is read with the schema.
            "index" | "trigger" => {}
            "table" => match store.table(name) {
                Some(table) => scan_table(conn, table, report)?,
                None => {
                    report.coverage.unknown_tables += 1;
                    report.incomplete(Reason::UnknownTable);
                }
            },
            _ => {
                report.coverage.unknown_tables += 1;
                report.incomplete(Reason::UnknownTable);
            }
        }
    }
    Ok(())
}

fn scan_table(conn: &Connection, table: &Table, report: &mut Report) -> Result<(), Reason> {
    // The columns the file has, those included that a plain listing
    // leaves out. Only a name the scanner knows is put in a statement or
    // in the report.
    let present: Vec<(String, i64)> = {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_xinfo(\"{}\")", table.name))
            .map_err(failed)?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(1)?, row.get(6)?)))
            .map_err(failed)?;
        rows.collect::<Result<_, _>>().map_err(failed)?
    };
    let mut known: Vec<(&'static str, Column)> = Vec::new();
    for (name, hidden) in &present {
        // A column that is computed is not one the scanner knows, whatever
        // it is called: what it holds is an expression of the file's.
        let listed = table
            .columns
            .iter()
            .find(|(column, _)| column == name)
            .filter(|_| *hidden == 0);
        match listed {
            Some(column) => known.push(*column),
            None if *hidden == 1 && schema::hidden(table.name).contains(&name.as_str()) => {}
            None => {
                report.coverage.unknown_columns += 1;
                report.incomplete(Reason::UnknownColumn);
            }
        }
    }
    let mut coverage = TableCoverage {
        table: table.name,
        locator: table.locator.as_str(),
        rows: 0,
        columns: known
            .iter()
            .map(|(column, kind)| (*column, kind.as_str()))
            .collect(),
    };
    if known.is_empty() {
        report.coverage.tables.push(coverage);
        return Ok(());
    }

    let columns = known
        .iter()
        .map(|(column, _)| format!("\"{column}\""))
        .collect::<Vec<_>>()
        .join(", ");
    // Nothing is asked for in an order: SQLite reads a table as it is
    // kept, and sorts nothing.
    let sql = match table.locator {
        Locator::RowId => format!("SELECT rowid, {columns} FROM \"{}\"", table.name),
        Locator::Ordinal => format!("SELECT 0, {columns} FROM \"{}\"", table.name),
    };
    let mut stmt = conn.prepare(&sql).map_err(failed)?;
    let mut rows = stmt.query([]).map_err(failed)?;
    while let Some(row) = rows.next().map_err(failed)? {
        coverage.rows += 1;
        let at = match table.locator {
            Locator::RowId => row.get::<_, i64>(0).map_err(failed)?,
            Locator::Ordinal => i64::try_from(coverage.rows).unwrap_or(i64::MAX),
        };
        for (i, (column, kind)) in known.iter().enumerate() {
            let bytes = match row.get_ref(i + 1).map_err(failed)? {
                ValueRef::Null | ValueRef::Integer(_) | ValueRef::Real(_) => continue,
                ValueRef::Blob(_) if matches!(kind, Column::Vector | Column::Index) => continue,
                ValueRef::Text(bytes) | ValueRef::Blob(bytes) => bytes,
            };
            let Ok(text) = std::str::from_utf8(bytes) else {
                report.coverage.undecodable_values += 1;
                report.incomplete(Reason::UndecodableValue);
                continue;
            };
            let counts = inspect(*kind, text, report);
            report.found(table.name, column, at, &counts);
        }
    }
    report.coverage.tables.push(coverage);
    Ok(())
}

/// What the policy recognizes in one value, read as its column is.
fn inspect(kind: Column, text: &str, report: &mut Report) -> Counts {
    match kind {
        // Read strictly: of two members of one name a parser keeps the
        // last, and the first would not be looked at.
        Column::Json => match parse_strict(text) {
            Some(value) => match inspect_json(&value, Limits::default()) {
                Ok(counts) => counts,
                Err(_) => {
                    report.coverage.structures_over_limit += 1;
                    report.incomplete(Reason::StructureLimit);
                    redact_text(text).counts
                }
            },
            // What is not the document it should be is read as the text
            // it is, and the scan is not complete: a value under a name
            // is only seen in a document.
            None => {
                report.coverage.undecodable_values += 1;
                report.incomplete(Reason::UndecodableValue);
                redact_text(text).counts
            }
        },
        // As it is, and what is left of it in lower case: what the first
        // reading found is a marker by then, and is not counted again.
        Column::Name => {
            let given = redact_text(text);
            let mut counts = given.counts;
            counts.merge(&redact_text(&given.value.to_lowercase()).counts);
            counts
        }
        Column::Text | Column::Plain | Column::Vector | Column::Index => redact_text(text).counts,
    }
}
