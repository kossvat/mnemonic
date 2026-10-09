//! The live state under a clock the tests hold. Every name is invented.
use super::*;
use chrono::TimeZone;

const P: ProjectId = 1;
const Q: ProjectId = 2;

/// The second `minute` minutes and `second` seconds after ten o'clock.
fn at(minute: i64, second: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 30, 10, 0, 0).unwrap() + Duration::seconds(minute * 60 + second)
}

fn session(agent: &str, project: Option<ProjectId>, kind: Kind, last: DateTime<Utc>) -> Session {
    Session {
        agent: agent.into(),
        program: false,
        project,
        last_kind: kind,
        last_at: last,
        started_at: at(0, 0),
    }
}

fn named(key: &str) -> Named {
    Named {
        key: key.into(),
        name: key.into(),
    }
}

/// A state that knows two projects.
fn state() -> LiveState {
    let live = LiveState::new("boot-a");
    let names = HashMap::from([(P, named("demoapp")), (Q, named("notes"))]);
    live.set_projects(names, HashMap::new(), at(0, 0));
    live
}

fn body(live: &LiveState) -> serde_json::Value {
    serde_json::from_slice(&live.snapshot().1).unwrap()
}

#[test]
fn live_two_agents_on_two_projects_are_two_live_projects() {
    let live = state();
    assert_eq!(body(&live)["projects"], serde_json::json!([]));
    live.observed(
        "a",
        session("claude", Some(P), Kind::Call, at(1, 0)),
        at(1, 0),
    );
    let settled = live.observed(
        "b",
        session("codex", Some(Q), Kind::TurnEnd, at(1, 0)),
        at(1, 0),
    );
    assert!(settled.changed);
    let snapshot = body(&live);
    assert_eq!(snapshot["boot"], "boot-a");
    assert_eq!(snapshot["v"], live.version());
    let projects = snapshot["projects"].as_array().unwrap();
    // The one an agent is at work in comes first.
    assert_eq!(
        (projects[0]["key"].as_str(), projects[0]["state"].as_str()),
        (Some("demoapp"), Some("running"))
    );
    assert_eq!(
        projects[0]["sessions"],
        serde_json::json!([{"agent": "claude", "driver": "owner", "state": "running",
            "started_at": at(0, 0).timestamp()}])
    );
    assert_eq!(
        (projects[1]["key"].as_str(), projects[1]["state"].as_str()),
        (Some("notes"), Some("waiting"))
    );
    assert_eq!(projects[1]["sessions"][0]["agent"], "codex");
    assert_eq!(snapshot["more_projects"], 0);
    assert_eq!(snapshot["unknown"]["sessions"], serde_json::json!([]));
}

#[test]
fn live_a_session_with_no_project_is_listed_under_unknown() {
    let live = state();
    live.observed("a", session("claude", None, Kind::Call, at(1, 0)), at(1, 0));
    // A project the registry does not know is no project either.
    live.observed(
        "b",
        session("codex", Some(99), Kind::Call, at(1, 0)),
        at(1, 0),
    );
    let snapshot = body(&live);
    assert_eq!(snapshot["projects"], serde_json::json!([]));
    assert_eq!(snapshot["unknown"]["sessions"].as_array().unwrap().len(), 2);
    // A session a program ran says so.
    let mut by_program = session("claude", Some(P), Kind::Call, at(1, 0));
    by_program.program = true;
    live.observed("c", by_program, at(1, 0));
    assert_eq!(
        body(&live)["projects"][0]["sessions"][0]["driver"],
        "program"
    );
}

#[test]
fn live_the_state_of_a_session_follows_the_kind_and_the_age_of_its_last_record() {
    let working = session("claude", Some(P), Kind::Call, at(0, 0));
    assert_eq!(working.state_at(at(0, 0)), Some(SessionState::Running));
    assert_eq!(working.state_at(at(1, 59)), Some(SessionState::Running));
    // A long build: six minutes without a record, and no turn closed.
    assert_eq!(working.state_at(at(2, 0)), Some(SessionState::Quiet));
    assert_eq!(working.state_at(at(6, 0)), Some(SessionState::Quiet));
    assert_eq!(working.state_at(at(29, 59)), Some(SessionState::Quiet));
    assert_eq!(working.state_at(at(30, 0)), None);
    // A prompt of the owner the agent has not answered is work too.
    let prompted = session("claude", Some(P), Kind::Prompt, at(0, 0));
    assert_eq!(prompted.state_at(at(1, 0)), Some(SessionState::Running));
    // The agent closed its turn: the owner has the move, for ten minutes.
    let closed = session("claude", Some(P), Kind::TurnEnd, at(0, 0));
    assert_eq!(closed.state_at(at(0, 0)), Some(SessionState::Waiting));
    assert_eq!(closed.state_at(at(9, 59)), Some(SessionState::Waiting));
    assert_eq!(closed.state_at(at(10, 0)), None);

    // The state says when it changes next, and time alone changes it.
    let live = state();
    let settled = live.observed("a", working.clone(), at(0, 0));
    assert_eq!(settled.next, Some(at(2, 0)));
    let today = at(0, 0).date_naive();
    let before = live.version();
    assert!(!live.tick(at(1, 59), today).changed);
    let settled = live.tick(at(2, 0), today);
    assert_eq!((settled.changed, settled.next), (true, Some(at(30, 0))));
    assert_eq!(body(&live)["projects"][0]["state"], "quiet");
    let settled = live.tick(at(30, 0), today);
    assert_eq!((settled.changed, settled.next), (true, None));
    assert_eq!(body(&live)["projects"], serde_json::json!([]));
    assert_eq!(live.version(), before + 2);
}

#[test]
fn live_records_that_change_nothing_shown_leave_the_version() {
    let live = state();
    live.observed(
        "a",
        session("claude", Some(P), Kind::Call, at(0, 0)),
        at(0, 0),
    );
    let version = live.version();
    let tag = live.snapshot().0;
    // A thousand records of one project in one running session.
    for second in 1..=1000 {
        let kind = [Kind::Call, Kind::Other, Kind::Prompt][second as usize % 3];
        let now = at(0, second % 100);
        let settled = live.observed("a", session("claude", Some(P), kind, now), now);
        assert!(!settled.changed);
    }
    assert_eq!((live.version(), live.snapshot().0), (version, tag));

    // Each of these is in the snapshot, and moves it once.
    let now = at(1, 0);
    let moves = |what: &str, settled: Settled, expected: u64| {
        assert!(settled.changed, "{what}");
        assert_eq!(live.version(), expected, "{what}");
    };
    let hours = Hours {
        day: Some(now.date_naive()),
        projects: HashMap::from([(
            P,
            Today {
                owner_s: 60,
                agent_s: 90,
                confidence: Some("high"),
            },
        )]),
        owner_s: 60,
        agent_s: 90,
    };
    moves("hours", live.set_hours(hours.clone(), now), version + 1);
    assert!(!live.set_hours(hours, now).changed);
    let closed = session("claude", Some(P), Kind::TurnEnd, now);
    moves(
        "the state of a session",
        live.observed("a", closed, now),
        version + 2,
    );
    let plan = PlanLine {
        unit: "a change".into(),
        done: 3,
        total: 7,
        stage: "building".into(),
        task: Some("the third task".into()),
        gate: None,
    };
    moves(
        "a plan",
        live.set_plan(P, Some(plan.clone()), now),
        version + 3,
    );
    assert!(!live.set_plan(P, Some(plan), now).changed);
    let other = session("codex", Some(Q), Kind::Call, now);
    moves("membership", live.observed("b", other, now), version + 4);
    moves("a session gone", live.forget("b", now), version + 5);

    let project = &body(&live)["projects"][0];
    assert_eq!(
        project["today"],
        serde_json::json!({"owner_s": 60, "agent_s": 90, "confidence": "high"})
    );
    assert_eq!(project["plan"]["done"], 3);
    assert_eq!(
        body(&live)["today"],
        serde_json::json!({"owner_s": 60, "agent_s": 90})
    );
}

#[test]
fn live_the_hours_of_the_day_start_again_at_midnight_without_a_record() {
    let live = state();
    let day = at(0, 0).date_naive();
    let hours = Hours {
        day: Some(day),
        projects: HashMap::new(),
        owner_s: 3600,
        agent_s: 7200,
    };
    live.set_hours(hours, at(0, 0));
    assert!(live.is_armed());
    assert!(!live.tick(at(5, 0), day).changed);
    assert_eq!(body(&live)["today"]["owner_s"], 3600);
    let settled = live.tick(at(5, 0), day.succ_opt().unwrap());
    assert!(settled.changed);
    assert_eq!(
        body(&live)["today"],
        serde_json::json!({"owner_s": 0, "agent_s": 0})
    );
}

#[test]
fn live_merged_projects_are_one_project_with_both_sessions() {
    let live = state();
    live.set_projects(
        HashMap::from([(P, named("demoapp")), (Q, named("notes"))]),
        HashMap::from([(Q, P)]),
        at(0, 0),
    );
    live.observed(
        "a",
        session("claude", Some(P), Kind::Call, at(0, 0)),
        at(0, 0),
    );
    live.observed(
        "b",
        session("codex", Some(Q), Kind::Call, at(0, 0)),
        at(0, 0),
    );
    let snapshot = body(&live);
    let projects = snapshot["projects"].as_array().unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0]["key"], "demoapp");
    assert_eq!(projects[0]["sessions"].as_array().unwrap().len(), 2);
}

/// Letters of three bytes each, and letters of one.
const WIDE: [char; 4] = ['\u{3042}', '\u{3044}', '\u{3046}', '\u{3048}'];
const NARROW: [char; 4] = ['a', 'b', 'c', 'd'];

/// A crowded state: `projects` projects with `each` sessions, the hours
/// and counts at their widest, and texts written in `letters`: names and
/// units of `name` characters and tasks of `task`.
fn crowded(
    projects: usize,
    each: usize,
    letters: [char; 4],
    (name, task): (usize, usize),
) -> LiveState {
    let live = LiveState::new("0123456789abcdef");
    let long = |seed: usize, chars: usize| -> String {
        (0..chars).map(|at| letters[(seed + at) % 4]).collect()
    };
    let now = at(1, 0);
    let names = (0..projects as i64)
        .map(|id| {
            let text = long(id as usize, name);
            (
                id,
                Named {
                    key: text.clone(),
                    name: text,
                },
            )
        })
        .collect();
    live.set_projects(names, HashMap::new(), now);
    let mut hours = Hours {
        day: Some(now.date_naive()),
        owner_s: 86_399,
        agent_s: 86_399,
        ..Hours::default()
    };
    for id in 0..projects as i64 {
        let today = Today {
            owner_s: 86_399,
            agent_s: 86_399,
            confidence: Some("medium"),
        };
        hours.projects.insert(id, today);
        let plan = PlanLine {
            unit: long(1, name),
            done: 9999,
            total: 9999,
            stage: "building".into(),
            task: Some(long(2, task)),
            gate: Some("passed".into()),
        };
        live.set_plan(id, Some(plan), now);
        for n in 0..each {
            let mut one = session("claude", Some(id), Kind::Call, now);
            one.program = n % 2 == 1;
            live.observed(&format!("s-{id}-{n}"), one, now);
        }
    }
    live.set_hours(hours, now);
    live
}

#[test]
fn live_the_snapshot_keeps_its_caps_and_counts_what_it_leaves_out() {
    // Twelve projects of three sessions each: over both caps, with every
    // text at its longest in a script of three bytes a character.
    // The texts given are longer than the snapshot allows.
    let live = crowded(12, 3, WIDE, (60, 80));
    live.set_memories(u64::MAX, at(1, 0));
    let bytes = live.snapshot().1;
    assert!(bytes.len() <= 8 * 1024, "{} bytes", bytes.len());
    let snapshot: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let projects = snapshot["projects"].as_array().unwrap();
    assert_eq!(
        (projects.len(), snapshot["more_projects"].as_u64()),
        (8, Some(4))
    );
    let shown: usize = projects
        .iter()
        .map(|project| project["sessions"].as_array().unwrap().len())
        .sum();
    let left: u64 = projects
        .iter()
        .map(|project| project["more_sessions"].as_u64().unwrap())
        .sum();
    assert_eq!((shown, left), (16, 8));
    // A text is cut to its length by characters, not bytes.
    let name = projects[0]["name"].as_str().unwrap();
    assert_eq!((name.chars().count(), name.len()), (32, 96));
    // The key is what the routes of a project take: it is served whole.
    assert_eq!(projects[0]["key"].as_str().unwrap().chars().count(), 60);
    assert_eq!(
        projects[0]["plan"]["task"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        40
    );
    assert_eq!(
        projects[0]["plan"]["unit"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        32
    );

    // Four projects and six sessions, with names of twenty-four letters
    // and every task at its longest.
    let small = crowded(4, 1, NARROW, (24, 40));
    let now = at(1, 0);
    small.observed("extra-1", session("codex", Some(0), Kind::Call, now), now);
    small.observed("extra-2", session("codex", None, Kind::Call, now), now);
    let bytes = small.snapshot().1;
    assert!(bytes.len() <= 2 * 1024, "{} bytes", bytes.len());
    let snapshot: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(snapshot["projects"].as_array().unwrap().len(), 4);
    assert_eq!(snapshot["unknown"]["sessions"].as_array().unwrap().len(), 1);
}

#[test]
fn live_a_text_that_holds_a_credential_is_served_redacted() {
    let live = LiveState::new("boot-a");
    let secret: String = ["sk-", "proj-", &"a1B2c3D4e5F6".repeat(4)].concat();
    live.set_projects(
        HashMap::from([(P, named("demoapp"))]),
        HashMap::new(),
        at(0, 0),
    );
    let plan = PlanLine {
        unit: "a change".into(),
        done: 0,
        total: 1,
        stage: "planned".into(),
        task: Some(format!("rotate {secret}")),
        gate: None,
    };
    live.set_plan(P, Some(plan), at(0, 0));
    live.observed(
        "a",
        session("claude", Some(P), Kind::Call, at(0, 0)),
        at(0, 0),
    );
    let served = String::from_utf8(live.snapshot().1.to_vec()).unwrap();
    assert!(served.contains("rotate"));
    assert!(!served.contains(&secret) && !served.contains(&secret[..20]));
}

#[test]
fn live_a_new_run_has_another_tag_and_an_empty_state_is_a_snapshot() {
    let first = LiveState::new("boot-a");
    let second = LiveState::new("boot-b");
    assert_ne!(first.snapshot().0, second.snapshot().0);
    assert_eq!(first.snapshot().0, "\"boot-a-0\"");
    assert!(!first.is_armed());
    let snapshot = body(&first);
    assert_eq!(
        snapshot,
        serde_json::json!({"v": 0, "boot": "boot-a", "projects": [], "more_projects": 0,
            "unknown": {"sessions": [], "more_sessions": 0},
            "today": {"owner_s": 0, "agent_s": 0}})
    );
}

/// The state the fixture of the clients is the snapshot of: two projects,
#[test]
fn live_the_count_of_memories_is_served_and_moves_the_snapshot_only_when_it_differs() {
    let live = state();
    // Until it is told, the snapshot names no count.
    assert!(body(&live).get("memories").is_none());
    assert!(live.set_memories(41, at(0, 1)).changed);
    assert_eq!(body(&live)["memories"], 41);
    let tag = live.snapshot().0;
    assert!(!live.set_memories(41, at(0, 2)).changed);
    assert_eq!(live.snapshot().0, tag);
    assert!(live.set_memories(42, at(0, 3)).changed);
    assert_eq!(body(&live)["memories"], 42);
    assert_ne!(live.snapshot().0, tag);
}

/// one with two sessions and a plan, a session with no project, the
/// hours of a day, and a count of memories.
fn fixture_state() -> LiveState {
    let live = LiveState::new("0123456789abcdef");
    let now = at(1, 0);
    let names = HashMap::from([(P, named("demoapp")), (Q, named("notes"))]);
    live.set_projects(names, HashMap::new(), now);
    live.observed("a", session("claude", Some(P), Kind::Call, at(0, 50)), now);
    let mut by_program = session("codex", Some(P), Kind::Other, at(0, 10));
    by_program.program = true;
    by_program.started_at = at(0, 5);
    live.observed("b", by_program, now);
    live.observed(
        "c",
        session("codex", Some(Q), Kind::TurnEnd, at(0, 30)),
        now,
    );
    live.observed("d", session("claude", None, Kind::Prompt, at(0, 40)), now);
    let plan = PlanLine {
        unit: "a change".into(),
        done: 3,
        total: 7,
        stage: "building".into(),
        task: Some("the fourth task".into()),
        gate: Some("stale".into()),
    };
    live.set_plan(P, Some(plan), now);
    let today = |owner_s, agent_s, confidence| Today {
        owner_s,
        agent_s,
        confidence,
    };
    let hours = Hours {
        day: Some(now.date_naive()),
        projects: HashMap::from([
            (P, today(5400, 9000, Some("high"))),
            (Q, today(0, 1200, None)),
        ]),
        owner_s: 6000,
        agent_s: 10_800,
    };
    live.set_hours(hours, now);
    live.set_memories(128, now);
    live
}

/// The snapshot the clients' tests decode is the one this state serves.
/// Print it with `MNEMONIC_PRINT_FIXTURES=1 cargo test --release
/// live_the_fixture -- --nocapture` after a change of the snapshot.
#[test]
fn live_the_fixture_of_the_clients_is_the_snapshot_served() {
    let served = String::from_utf8(fixture_state().snapshot().1.to_vec()).unwrap();
    if std::env::var_os("MNEMONIC_PRINT_FIXTURES").is_some() {
        println!("FIXTURE {served}");
    }
    let fixture = include_str!("../clients/macos/Tests/Fixtures/live.json");
    assert_eq!(fixture.trim_end(), served);
}
