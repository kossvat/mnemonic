use super::*;
use crate::projects::lookup::{Merges, Named};
use crate::projects::resolver::{Facts, Folder, ToolClass};

const A: ProjectId = 1;
const B: ProjectId = 2;

fn at(minute: i64) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-09-30T10:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
        + Duration::minutes(minute)
}

fn record(minute: i64, facts: Facts) -> Observed {
    Observed {
        at: Some(at(minute)),
        facts,
        ..Observed::default()
    }
}
fn quiet(minute: i64) -> Observed {
    record(minute, Facts::default())
}
fn folder(minute: i64, id: ProjectId) -> Observed {
    record(
        minute,
        Facts {
            folder: Folder::Project(id),
            call: None,
            ..Facts::default()
        },
    )
}
fn write(minute: i64, id: ProjectId) -> Observed {
    record(
        minute,
        Facts {
            folder: Folder::Unsaid,
            call: Some((ToolClass::Write, Named::One(id))),
            ..Facts::default()
        },
    )
}

/// The segments as (project, how it was learned, first minute, last minute).
fn walk(records: &[Observed]) -> Vec<(Option<ProjectId>, Learned, i64, i64)> {
    let mut walker = Walker::new(1800);
    for observed in records {
        walker.record(observed, None);
    }
    let minute = |time: DateTime<Utc>| (time - at(0)).num_minutes();
    walker
        .finish()
        .0
        .into_iter()
        .map(|s| {
            (
                s.project,
                s.learned,
                minute(s.started_at),
                minute(s.last_at),
            )
        })
        .collect()
}

#[test]
fn replay_segments_the_time_between_two_records_belongs_to_the_state_before_the_later() {
    let segments = walk(&[folder(0, A), quiet(5), write(10, B), quiet(20), quiet(25)]);
    assert_eq!(
        segments,
        [
            // Up to the write the session was on A: the write's own minute
            // ends that segment.
            (Some(A), Learned::By(Source::Launch), 0, 10),
            (Some(B), Learned::By(Source::Path), 10, 25),
        ]
    );
}

#[test]
fn replay_segments_a_pause_as_long_as_the_idle_time_ends_a_stretch() {
    let segments = walk(&[folder(0, A), quiet(10), quiet(45), quiet(50)]);
    assert_eq!(
        segments,
        [
            (Some(A), Learned::By(Source::Launch), 0, 10),
            (Some(A), Learned::By(Source::Launch), 45, 50),
        ]
    );
    // One second under the idle time is the same stretch.
    let mut walker = Walker::new(1800);
    walker.record(&folder(0, A), None);
    walker.record(
        &Observed {
            at: Some(at(30) - Duration::seconds(1)),
            ..Observed::default()
        },
        None,
    );
    assert_eq!(walker.finish().0.len(), 1);
}

#[test]
fn replay_segments_the_unknown_head_of_a_stretch_takes_the_first_signal() {
    let segments = walk(&[quiet(0), quiet(4), write(9, B), quiet(12)]);
    assert_eq!(
        segments,
        [
            (Some(B), Learned::Backfill, 0, 9),
            (Some(B), Learned::By(Source::Path), 9, 12),
        ]
    );
    // A stretch no signal named stays unknown, and a later stretch does
    // not reach back into it.
    let segments = walk(&[quiet(0), quiet(4), write(60, B), quiet(62)]);
    assert_eq!(
        segments,
        [
            (None, Learned::By(Source::Unknown), 0, 4),
            (Some(B), Learned::By(Source::Path), 60, 62),
        ]
    );
}

#[test]
fn replay_segments_a_record_without_a_time_or_out_of_order_moves_no_time() {
    let mut walker = Walker::new(1800);
    walker.record(&folder(10, A), None);
    // Copied by a compaction: an earlier time, later in the file.
    walker.record(&quiet(2), None);
    // No time at all: it still moves the rules.
    walker.record(
        &Observed {
            at: None,
            facts: Facts {
                folder: Folder::Unsaid,
                call: Some((ToolClass::Write, Named::One(B))),
                ..Facts::default()
            },
            ..Observed::default()
        },
        None,
    );
    assert_eq!(walker.state().project, Some(B));
    walker.record(&quiet(15), None);
    let (segments, counts) = walker.finish();
    assert_eq!(segments.len(), 2);
    assert_eq!((segments[0].project, segments[0].seconds()), (Some(A), 300));
    assert_eq!((segments[1].project, segments[1].seconds()), (Some(B), 0));
    assert_eq!(counts.records, 4);
}

#[test]
fn replay_segments_what_was_counted() {
    let many = Facts {
        folder: Folder::Unsaid,
        call: Some((ToolClass::Read, Named::Many)),
        ..Facts::default()
    };
    let none = Facts {
        folder: Folder::Unsaid,
        call: Some((ToolClass::Read, Named::None)),
        ..Facts::default()
    };
    let mut walker = Walker::new(1800);
    for observed in [
        folder(0, A),
        write(1, B),
        write(2, B),
        record(3, many),
        record(4, none),
    ] {
        walker.record(&observed, None);
    }
    let (_, counts) = walker.finish();
    assert_eq!(
        counts,
        Counts {
            records: 5,
            naming: 2,
            ambiguous: 1,
            switches: 2
        }
    );
}

#[test]
fn replay_segments_a_pin_names_every_record() {
    let mut walker = Walker::new(1800);
    for observed in [folder(0, A), write(5, A), quiet(9)] {
        walker.record(&observed, Some(B));
    }
    let (segments, _) = walker.finish();
    assert_eq!(segments.len(), 1);
    assert_eq!(
        (segments[0].project, segments[0].learned),
        (Some(B), Learned::By(Source::Manual))
    );
}

#[test]
fn replay_segments_a_shell_call_that_runs_in_the_project_touches_it() {
    // The command names another project too, so its input names two; the
    // call says it runs in the folder of the project in work.
    let shell = |minute: i64, shell_dir: Folder| {
        record(
            minute,
            Facts {
                folder: Folder::Unsaid,
                call: Some((ToolClass::Shell, Named::Many)),
                shell_dir,
            },
        )
    };
    let touches = |records: &[Observed]| {
        let mut walker = Walker::new(1800);
        for observed in records {
            walker.record(observed, None);
        }
        let (segments, _) = walker.finish();
        let touched: u32 = segments[0].activity.iter().map(|minute| minute.2).sum();
        (touched, segments[0].last_touch_at)
    };
    assert_eq!(
        touches(&[folder(0, A), shell(4, Folder::Project(A)), quiet(9)]),
        (2, Some(at(4)))
    );
    // One that runs elsewhere, or does not say where, names nothing here.
    assert_eq!(
        touches(&[folder(0, A), shell(4, Folder::Outside), quiet(9)]),
        (1, Some(at(0)))
    );
    assert_eq!(
        touches(&[folder(0, A), shell(4, Folder::Unsaid), quiet(9)]),
        (1, Some(at(0)))
    );
}

#[test]
fn replay_segments_merged_folders_are_one_stretch_kept_under_the_folder_named() {
    let mut merges = Merges::default();
    merges.insert(B, A);
    let mut walker = Walker::new(1800).merged(merges);
    for observed in [folder(0, A), write(5, B), quiet(9), write(12, B)] {
        walker.record(&observed, None);
    }
    let (segments, counts) = walker.finish();
    // No switch was counted; the time is kept under the folder it was in.
    assert_eq!(counts.switches, 1);
    let kept: Vec<_> = segments
        .iter()
        .map(|segment| (segment.project, segment.seconds()))
        .collect();
    assert_eq!(kept, [(Some(A), 300), (Some(B), 420)]);
    // A write into the merged folder names the project of the segment.
    assert_eq!(segments[1].last_touch_at, Some(at(12)));
}
