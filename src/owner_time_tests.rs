//! The split of the owner's time, as expected allocations: a check of the
//! sum alone passes when everything is given to nobody.
use super::*;

const P: ProjectId = 1;
const Q: ProjectId = 2;

/// Midnight of a day, in seconds since the epoch.
const DAY: i64 = 1_790_208_000;

/// The second `minute` minutes and `second` seconds into the day.
fn at(minute: i64, second: i64) -> i64 {
    DAY + minute * 60 + second
}

/// The split where the owner corrected nothing.
fn split(activity: &[(i64, i64)], transcripts: &[Transcript]) -> Split {
    super::split(activity, transcripts, &[])
}

/// An interval of activity, in milliseconds.
fn active(from: i64, to: i64) -> (i64, i64) {
    (from * 1000, to * 1000)
}

/// A transcript on one project from minute `from` to minute `to`, with
/// records in `records` (minutes of the day) and prompts in `prompts`.
fn transcript(
    project: Option<ProjectId>,
    (from, to): (i64, i64),
    records: &[i64],
    prompts: &[i64],
) -> Transcript {
    Transcript {
        spans: vec![Span {
            project,
            started: at(from, 0),
            last: at(to, 0),
        }],
        prompts: prompts.iter().map(|minute| at(*minute, 0)).collect(),
        minutes: records.iter().map(|minute| at(*minute, 0) / 60).collect(),
        ..Transcript::default()
    }
    .sorted()
}

/// The same, where every record names the project.
fn touching(project: ProjectId, span: (i64, i64), records: &[i64], prompts: &[i64]) -> Transcript {
    let mut transcript = transcript(Some(project), span, records, prompts);
    transcript.touches = transcript
        .minutes
        .iter()
        .map(|minute| (*minute, project))
        .collect();
    transcript
}

fn seconds(split: &Split, project: ProjectId) -> f64 {
    split
        .projects
        .get(&project)
        .map_or(0.0, |by| by.iter().sum())
}

#[test]
fn owner_time_a_session_of_37_seconds_goes_to_the_project_of_its_transcript() {
    let work = touching(P, (0, 10), &[0, 1, 2], &[0]);
    let split = split(&[active(at(1, 3), at(1, 40))], &[work]);
    assert_eq!(split.projects, BTreeMap::from([(P, [37.0, 0.0, 0.0])]));
    assert!(split.uncovered.is_empty());
}

#[test]
fn owner_time_two_sessions_on_one_project_are_one_share() {
    // All three are inside a segment and nothing more: the last tier.
    let transcripts = [
        transcript(Some(P), (0, 30), &[0, 30], &[]),
        transcript(Some(P), (0, 30), &[0, 30], &[]),
        transcript(Some(Q), (0, 30), &[0, 30], &[]),
    ];
    let split = split(&[active(at(10, 0), at(12, 0))], &transcripts);
    assert_eq!((seconds(&split, P), seconds(&split, Q)), (60.0, 60.0));
}

#[test]
fn owner_time_the_first_tier_that_names_a_project_takes_all() {
    // P was prompted five minutes ago; Q has a record in this minute.
    let prompted = transcript(Some(P), (0, 30), &[0, 30], &[5]);
    let recorded = transcript(Some(Q), (0, 30), &[0, 10, 30], &[]);
    let inside = transcript(Some(3), (0, 30), &[0, 30], &[]);
    let whole = [prompted.clone(), recorded.clone(), inside.clone()];
    let minute = [active(at(10, 0), at(11, 0))];
    assert_eq!(seconds(&split(&minute, &whole), P), 60.0);
    assert_eq!(split(&minute, &whole).projects.len(), 1);
    // Without the prompted one the recorded one takes all.
    let split_b = split(&minute, &[recorded, inside.clone()]);
    assert_eq!(split_b.projects.keys().collect::<Vec<_>>(), [&Q]);
    // And alone, the one inside a segment.
    assert_eq!(seconds(&split(&minute, &[inside]), 3), 60.0);
}

#[test]
fn owner_time_which_minutes_hold_a_record_changes_the_split() {
    // The same segment, 0 to 20, beside another project's.
    let beside = transcript(Some(Q), (0, 20), &[0, 20], &[]);
    let minute_nine = [active(at(9, 0), at(10, 0))];
    // A record at minute 8 keeps it in the tier of records at minute 9.
    let early = transcript(Some(P), (0, 20), &[0, 8, 20], &[]);
    let with_record = split(&minute_nine, &[early, beside.clone()]);
    assert_eq!(
        (seconds(&with_record, P), seconds(&with_record, Q)),
        (60.0, 0.0)
    );
    // A record at minute 15 does not: both are only inside a segment.
    let late = transcript(Some(P), (0, 20), &[0, 15, 20], &[]);
    let without = split(&minute_nine, &[late, beside]);
    assert_eq!((seconds(&without, P), seconds(&without, Q)), (30.0, 30.0));
}

#[test]
fn owner_time_a_tier_ends_at_its_own_second() {
    // A prompt at minute 0 holds for 15 minutes, a record for its minute
    // and two more, a segment until its last record.
    let work = transcript(Some(P), (0, 40), &[0, 40], &[0]);
    let other = transcript(Some(Q), (0, 40), &[0, 16, 40], &[]);
    let split = split(&[active(at(14, 0), at(20, 0))], &[work, other]);
    // 14:00 to 15:00 prompted: P. 15:00 to 16:00 both inside: halves.
    // 16:00 to 19:00 Q has a record: Q. 19:00 to 20:00 halves again.
    assert_eq!(seconds(&split, P), 60.0 + 30.0 + 30.0);
    assert_eq!(seconds(&split, Q), 30.0 + 180.0 + 30.0);
}

#[test]
fn owner_time_a_prompt_keeps_the_project_after_its_segment_ended() {
    // The segment ended at minute 2; the owner reads on until minute 20.
    let work = transcript(Some(P), (0, 2), &[0, 2], &[0]);
    let split = split(&[active(at(0, 0), at(20, 0))], &[work]);
    assert_eq!(seconds(&split, P), 900.0);
    assert_eq!(split.uncovered, [(at(15, 0) * 1000, at(20, 0) * 1000)]);
}

#[test]
fn owner_time_a_transcript_with_no_known_project_is_in_no_tier() {
    let unknown = transcript(None, (0, 30), &[0, 10, 30], &[5]);
    let known = transcript(Some(P), (0, 30), &[0, 30], &[]);
    let minute = [active(at(10, 0), at(11, 0))];
    assert_eq!(seconds(&split(&minute, &[unknown.clone(), known]), P), 60.0);
    let alone = split(&minute, &[unknown]);
    assert!(alone.projects.is_empty());
    assert_eq!(alone.uncovered, [(at(10, 0) * 1000, at(11, 0) * 1000)]);
    // No transcript at all, and time outside every one, is uncovered too.
    assert_eq!(split(&minute, &[]).uncovered.len(), 1);
}

#[test]
fn owner_time_the_trust_follows_the_tier_and_the_age_of_the_touch() {
    let hour = [active(at(0, 0), at(120, 0))];
    // Prompted and touched at minute 0, a segment of two hours.
    let mut work = touching(P, (0, 120), &[0], &[0]);
    work.minutes = vec![at(0, 0) / 60, at(120, 0) / 60];
    let touched = split(&hour, std::slice::from_ref(&work));
    // High while prompted (15 minutes); medium while the touch is an hour
    // old or less; low after it.
    assert_eq!(touched.projects[&P], [900.0, 3600.0 - 900.0 + 1.0, 3599.0]);
    assert_eq!(dominant(&touched.projects[&P]), Trust::Low);

    // A prompt with no touch at all is not high: nothing named the
    // project lately. It is medium, as a record in the minute is; a
    // segment alone, with no touch in the last hour, is low.
    let untouched = transcript(Some(Q), (0, 120), &[0, 4, 120], &[0]);
    let none = split(&[active(at(0, 0), at(30, 0))], &[untouched]);
    assert_eq!(none.projects[&Q], [0.0, 900.0, 900.0]);
    let recorded = transcript(Some(Q), (0, 120), &[0, 20, 120], &[]);
    let by_record = split(&[active(at(20, 0), at(25, 0))], &[recorded]);
    assert_eq!(by_record.projects[&Q], [0.0, 180.0, 120.0]);
    assert_eq!(dominant(&[10.0, 10.0, 0.0]), Trust::High);
    assert_eq!(dominant(&[0.0, 5.0, 5.0]), Trust::Medium);
}

#[test]
fn owner_time_what_is_given_and_what_is_left_add_up_to_the_activity() {
    let transcripts = [
        touching(P, (0, 12), &[0, 3, 7, 12], &[0, 7]),
        transcript(Some(Q), (5, 50), &[5, 20, 50], &[20]),
        transcript(None, (30, 60), &[30, 60], &[]),
    ];
    let activity = [
        active(at(0, 13), at(0, 50)),
        active(at(2, 0), at(26, 30)),
        active(at(45, 10), at(75, 0)),
    ];
    let split = split(&activity, &transcripts);
    let left: i64 = split.uncovered.iter().map(|(from, to)| to - from).sum();
    let total = 37.0 + (24.0 * 60.0 + 30.0) + (29.0 * 60.0 + 50.0);
    assert!((split.given() + left as f64 / 1000.0 - total).abs() < 1e-6);
    assert!(seconds(&split, P) > 0.0 && seconds(&split, Q) > 0.0);
    // What is uncovered lies inside the activity, in order, not touching.
    assert!(split.uncovered.windows(2).all(|pair| pair[0].1 < pair[1].0));
}

#[test]
fn owner_time_an_empty_or_backward_interval_gives_nothing() {
    let work = transcript(Some(P), (0, 30), &[0, 30], &[0]);
    let split = split(
        &[active(at(5, 0), at(5, 0)), active(at(9, 0), at(8, 0))],
        &[work],
    );
    assert_eq!(split, Split::default());
}

#[test]
fn override_a_transcript_that_is_not_live_has_no_say_against_the_owners_range() {
    // The owner gave minutes 40 to 50 to P by hand. A range made later
    // relabels a transcript there, but the transcript ended at minute 10
    // and nothing keeps it live: the owner's range holds.
    let mut ended = transcript(Some(Q), (0, 10), &[0, 10], &[0]);
    ended.relabels = vec![Relabel {
        from: at(30, 0),
        to: at(60, 0),
        project: Some(Q),
        order: 2,
    }];
    let owner = [Relabel {
        from: at(40, 0),
        to: at(50, 0),
        project: Some(P),
        order: 1,
    }];
    let activity = [active(at(40, 0), at(50, 0))];
    let split = super::split(&activity, std::slice::from_ref(&ended), &owner);
    assert_eq!(split.projects, BTreeMap::from([(P, [600.0, 0.0, 0.0])]));
    // Live there, the later range has its say.
    let mut live = transcript(Some(Q), (0, 60), &[0, 60], &[]);
    live.relabels = ended.relabels.clone();
    let split = super::split(&activity, &[live.clone()], &owner);
    assert_eq!(split.projects, BTreeMap::from([(Q, [600.0, 0.0, 0.0])]));
    // Relabelled to no project, it names none: the owner's range holds.
    live.relabels[0].project = None;
    let split = super::split(&activity, &[live], &owner);
    assert_eq!(split.projects, BTreeMap::from([(P, [600.0, 0.0, 0.0])]));
}
