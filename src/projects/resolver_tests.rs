//! Each rule of the resolver has a fixture that turns on it. The projects
//! are numbers; no name of a project appears here.
use super::*;
use crate::projects::lookup::ProjectMap;
use std::path::Path;

const A: ProjectId = 1;
const B: ProjectId = 2;
const C: ProjectId = 3;

fn call(class: ToolClass, named: Named) -> Facts {
    Facts {
        folder: Folder::Unsaid,
        call: Some((class, named)),
        ..Facts::default()
    }
}
fn write(id: ProjectId) -> Facts {
    call(ToolClass::Write, Named::One(id))
}
fn read(id: ProjectId) -> Facts {
    call(ToolClass::Read, Named::One(id))
}
fn shell(named: Named) -> Facts {
    call(ToolClass::Shell, named)
}
fn at(folder: Folder) -> Facts {
    Facts {
        folder,
        call: None,
        ..Facts::default()
    }
}

/// The states and outcomes of a run of records from `state`.
fn run(mut state: State, records: &[Facts]) -> Vec<(Option<ProjectId>, Outcome)> {
    records
        .iter()
        .map(|facts| {
            let (next, outcome) = step(state, None, facts);
            state = next;
            (next.project, outcome)
        })
        .collect()
}

fn working_on(id: ProjectId) -> State {
    step(State::default(), None, &at(Folder::Project(id))).0
}

#[test]
fn resolver_a_pin_decides_whatever_the_record() {
    let state = working_on(A);
    for facts in [write(C), read(C), at(Folder::Project(C)), Facts::default()] {
        let (next, _) = step(state, Some(B), &facts);
        assert_eq!((next.project, next.source), (Some(B), Source::Manual));
        assert_eq!(next.candidate, None);
    }
    // Pinned to what it already works on: nothing switched, and it is the
    // owner's word from here on.
    let (next, outcome) = step(state, Some(A), &Facts::default());
    assert_eq!((outcome, next.source), (Outcome::Stayed, Source::Manual));
}

#[test]
fn resolver_a_write_that_names_one_project_switches_at_once() {
    let (next, outcome) = step(working_on(A), None, &write(B));
    assert_eq!((next.project, outcome), (Some(B), Outcome::Switched));
    assert_eq!(next.source, Source::Path);
    // A write into the project in force changes nothing.
    assert_eq!(step(next, None, &write(B)).1, Outcome::Stayed);
    // A write that names no project is no signal.
    let unnamed = call(ToolClass::Write, Named::None);
    assert_eq!(step(next, None, &unnamed), (next, Outcome::Stayed));
}

#[test]
fn resolver_a_read_that_names_one_project_needs_two_in_a_row() {
    use Outcome::*;
    assert_eq!(
        run(working_on(A), &[read(B), read(B)]),
        [(Some(A), Candidate), (Some(B), Switched)]
    );
    // A call that names no project is not among those that name one: it
    // does not break the row.
    let silent = call(ToolClass::Read, Named::None);
    assert_eq!(
        run(working_on(A), &[read(B), silent, Facts::default(), read(B)])[3],
        (Some(B), Switched)
    );
    // A read of the project in force between them does.
    assert_eq!(
        run(working_on(A), &[read(B), read(A), read(B), read(B)]),
        [
            (Some(A), Candidate),
            (Some(A), Stayed),
            (Some(A), Candidate),
            (Some(B), Switched)
        ]
    );
    // So does a read of a third project.
    assert_eq!(
        run(working_on(A), &[read(B), read(C), read(B), read(B)]),
        [
            (Some(A), Candidate),
            (Some(A), Candidate),
            (Some(A), Candidate),
            (Some(B), Switched)
        ]
    );
}

#[test]
fn resolver_a_shell_call_is_judged_by_its_folder() {
    use Outcome::*;
    // In the folder of a project it names that project, whatever its
    // command names: a program kept in another project's folder and run
    // from here does not move the state.
    let state = working_on(A);
    assert_eq!(
        run(
            state,
            &[
                shell(Named::One(B)),
                shell(Named::One(B)),
                shell(Named::Many)
            ]
        ),
        [(Some(A), Stayed), (Some(A), Stayed), (Some(A), Stayed)]
    );
    // As a read: after a write elsewhere, two calls from the folder bring
    // the state back, one does not.
    assert_eq!(
        run(state, &[write(B), shell(Named::None), shell(Named::None)]),
        [
            (Some(B), Switched),
            (Some(B), Candidate),
            (Some(A), Switched)
        ]
    );
    // In no project's folder it is judged by the one project its command
    // names, and not on its first call.
    let outside = step(State::default(), None, &at(Folder::Outside)).0;
    assert_eq!(
        run(outside, &[shell(Named::One(B)), shell(Named::One(B))]),
        [(None, Candidate), (Some(B), Switched)]
    );
    assert_eq!(run(outside, &[shell(Named::None)]), [(None, Stayed)]);
}

#[test]
fn resolver_a_call_that_names_two_projects_changes_nothing() {
    let state = step(working_on(A), None, &read(B)).0;
    assert_eq!(state.candidate, Some(B));
    for class in [ToolClass::Write, ToolClass::Read] {
        let (next, outcome) = step(state, None, &call(class, Named::Many));
        assert_eq!(outcome, Outcome::Ambiguous);
        assert_eq!(next, state, "an ambiguous call moved the state");
    }
}

#[test]
fn resolver_a_change_of_folder_into_a_project_switches_at_once() {
    use Outcome::*;
    let (launched, outcome) = step(State::default(), None, &at(Folder::Project(A)));
    assert_eq!((launched.project, outcome), (Some(A), Switched));
    assert_eq!(launched.source, Source::Launch);

    let (moved, outcome) = step(launched, None, &at(Folder::Project(B)));
    assert_eq!(
        (moved.project, outcome, moved.source),
        (Some(B), Switched, Source::Folder)
    );
    // The same folder again is no change; nor is a record that says nothing.
    assert_eq!(step(moved, None, &at(Folder::Project(B))).1, Stayed);
    assert_eq!(step(moved, None, &Facts::default()), (moved, Stayed));

    // Out of every project and back in.
    let (out, outcome) = step(moved, None, &at(Folder::Outside));
    assert_eq!((out.project, outcome), (Some(B), Stayed));
    let (back, outcome) = step(out, None, &write(A));
    assert_eq!((back.project, outcome), (Some(A), Switched));
    let (again, outcome) = step(back, None, &at(Folder::Project(B)));
    assert_eq!(
        (again.project, outcome, again.source),
        (Some(B), Switched, Source::Folder)
    );

    // A write decides before the folder does, in one record.
    let both = Facts {
        folder: Folder::Project(C),
        call: Some((ToolClass::Write, Named::One(A))),
        ..Facts::default()
    };
    let (next, _) = step(moved, None, &both);
    assert_eq!((next.project, next.folder), (Some(A), Some(C)));
    // A read that only made a candidate leaves the folder its say.
    let both = Facts {
        folder: Folder::Project(C),
        call: Some((ToolClass::Read, Named::One(A))),
        ..Facts::default()
    };
    let (next, outcome) = step(moved, None, &both);
    assert_eq!(
        (next.project, outcome, next.candidate),
        (Some(C), Switched, Some(A))
    );
}

#[test]
fn resolver_before_the_first_signal_the_state_is_the_launch_folder_or_unknown() {
    let state = State::default();
    assert_eq!((state.project, state.source), (None, Source::Unknown));
    // A session launched outside every project stays unknown until a
    // signal; its later move into a project is a change of folder.
    let outside = step(state, None, &at(Folder::Outside)).0;
    assert_eq!((outside.project, outside.placed), (None, true));
    let moved = step(outside, None, &at(Folder::Project(A))).0;
    assert_eq!((moved.project, moved.source), (Some(A), Source::Folder));
}

#[test]
fn resolver_the_function_is_pure() {
    let states = [
        State::default(),
        working_on(A),
        step(working_on(A), None, &read(B)).0,
    ];
    let records = [
        write(B),
        read(B),
        read(C),
        shell(Named::One(C)),
        shell(Named::None),
        call(ToolClass::Read, Named::Many),
        at(Folder::Project(C)),
        at(Folder::Outside),
        Facts::default(),
    ];
    let pins = [None, Some(C)];
    let mut inputs = Vec::new();
    for state in states {
        for pin in pins {
            for facts in records {
                inputs.push((state, pin, facts));
            }
        }
    }
    let answer = |inputs: &[(State, Option<ProjectId>, Facts)]| -> Vec<(State, Outcome)> {
        inputs
            .iter()
            .map(|(state, pin, facts)| step(*state, *pin, facts))
            .collect()
    };
    let forward = answer(&inputs);
    // The same inputs in another order of calls, and once more.
    let mut reversed: Vec<_> = inputs.iter().rev().copied().collect();
    let mut backward = answer(&reversed);
    backward.reverse();
    reversed.reverse();
    assert_eq!(forward, backward);
    assert_eq!(forward, answer(&inputs));
}

#[test]
fn resolver_a_state_kept_aside_resumes_the_rules_where_they_stopped() {
    let records = [
        read(B),
        read(B),
        shell(Named::None),
        write(C),
        read(A),
        read(A),
    ];
    let whole = run(working_on(A), &records);
    for cut in 0..records.len() {
        let mut state = working_on(A);
        for facts in &records[..cut] {
            state = step(state, None, facts).0;
        }
        // The state as a checkpoint would hold it: a copy of its fields.
        let kept = State { ..state };
        assert_eq!(run(kept, &records[cut..]), whole[cut..], "cut at {cut}");
    }
}

// ------------------------------------------------------------- the lookup

fn map() -> ProjectMap {
    let mut map = ProjectMap::new(Path::new("/h/workspace/projects"));
    map.insert("demoapp", A);
    map.insert_inside("demoapp", "tools", C);
    map.insert("Side Project", B);
    map
}

#[test]
fn resolver_lookup_a_path_names_its_project_in_each_layout() {
    let map = map();
    for path in [
        // The main checkout.
        "/h/workspace/projects/demoapp/src/main.rs",
        // A worktree under the workspace.
        "/h/workspace/.agent/worktrees/topic/projects/demoapp/src/main.rs",
        // A worktree beside the workspace.
        "/h/workspace-wt/topic/projects/demoapp/src/main.rs",
        // A worktree in a scratch folder.
        "/private/tmp/agent/worktrees/topic/projects/demoapp/src/main.rs",
        // A review checkout.
        "/private/tmp/scratch/review-abc123/projects/demoapp/src/main.rs",
        // The folder itself, and a path from the workspace.
        "/h/workspace/projects/demoapp",
        "projects/demoapp/src/main.rs",
    ] {
        assert_eq!(map.project_of(path), Some(A), "{path}");
    }
    // The deepest project that holds the path.
    assert_eq!(
        map.project_of("/h/workspace/projects/demoapp/tools/build.rs"),
        Some(C)
    );
    assert_eq!(
        map.project_of("/h/workspace/projects/demoapp/other/build.rs"),
        Some(A)
    );
    assert_eq!(
        map.project_of("/h/workspace/projects/Side Project/a.txt"),
        Some(B)
    );
    for path in [
        "/h/workspace/projects/unknown/src/main.rs",
        "/h/workspace/projects/demoapp-old/src/main.rs",
        "/h/workspace/projects/demoapp.bak/a",
        "/h/myprojects/demoapp/src/main.rs",
        "/h/.projects/demoapp/a",
        "/h/workspace/projects/",
        "/h/elsewhere/demoapp/src/main.rs",
        "",
    ] {
        assert_eq!(map.project_of(path), None, "{path}");
    }
    assert_eq!(map.folders(), 3);
}

#[test]
fn resolver_lookup_a_text_names_the_projects_of_its_paths() {
    let map = map();
    let one = r#"{"command":"cd /h/workspace/projects/demoapp && cargo test","timeout":5}"#;
    assert_eq!(map.named_in(one), Named::One(A));
    // The same project twice is one project.
    let twice = "diff /h/workspace/projects/demoapp/a /x/review-1/projects/demoapp/a";
    assert_eq!(map.named_in(twice), Named::One(A));
    let two = "cp /h/workspace/projects/demoapp/a '/h/workspace/projects/Side Project/a'";
    assert_eq!(map.named_in(two), Named::Many);
    // Inside a text that was itself written into a string.
    let nested = r#"{"arguments":"{\"command\":[\"ls\",\"/h/workspace/projects/demoapp\"]}"}"#;
    assert_eq!(map.named_in(nested), Named::One(A));
    for none in [
        "cargo test --release",
        "cat /dev/null",
        "ls /h/workspace/projects",
    ] {
        assert_eq!(map.named_in(none), Named::None, "{none}");
    }
    // A workspace whose folder of projects has no name marks nothing.
    assert_eq!(ProjectMap::new(Path::new("/")).named_in(one), Named::None);
}

#[test]
fn resolver_a_shell_call_that_names_its_own_folder_is_judged_by_it() {
    use Outcome::*;
    // The session is in A; the call says it runs in B.
    let in_b = Facts {
        shell_dir: Folder::Project(B),
        ..shell(Named::One(B))
    };
    assert_eq!(
        run(working_on(A), &[in_b, in_b]),
        [(Some(A), Candidate), (Some(B), Switched)]
    );
    // A call that says it runs outside every project is judged by what its
    // command names, not by the session's folder.
    let outside = Facts {
        shell_dir: Folder::Outside,
        ..shell(Named::One(C))
    };
    assert_eq!(
        run(working_on(A), &[outside, outside]),
        [(Some(A), Candidate), (Some(C), Switched)]
    );
    // The folder of a call is not where the session is: it moves nothing
    // that later records are judged by.
    let state = step(working_on(A), None, &in_b).0;
    assert_eq!(state.folder, Some(A));
}

#[test]
fn resolver_two_folders_the_owner_merged_are_one_project_to_the_rules() {
    use Outcome::*;
    let mut merges = Merges::default();
    merges.insert(B, A);
    let step = |state, facts: &Facts| step_merged(state, None, facts, &merges);

    // A write into the merged folder is no switch. The state takes the id
    // of that folder: what is kept says where the work was.
    let (next, outcome) = step(working_on(A), &write(B));
    assert_eq!((next.project, outcome), (Some(B), Stayed));
    assert_eq!(next.source, Source::Launch);
    // A read there is a read in the project in work.
    let state = State {
        candidate: Some(C),
        ..working_on(A)
    };
    let (next, outcome) = step(state, &read(B));
    assert_eq!(
        (next.project, next.candidate, outcome),
        (Some(A), None, Stayed)
    );
    // Two reads in a row, one in each of the merged folders, name one
    // project twice.
    let (first, outcome) = step(working_on(C), &read(A));
    assert_eq!((first.candidate, outcome), (Some(A), Candidate));
    let (second, outcome) = step(first, &read(B));
    assert_eq!((second.project, outcome), (Some(B), Switched));
    // The session moves into the merged folder: the same project, kept
    // from here under that folder.
    let (moved, outcome) = step(working_on(A), &at(Folder::Project(B)));
    assert_eq!(
        (moved.project, moved.folder, outcome),
        (Some(B), Some(B), Stayed)
    );
    // A record that repeats where the session is moves nothing: a write
    // into the other folder holds until the session itself moves.
    let (wrote, _) = step(moved, &write(A));
    let (again, outcome) = step(wrote, &at(Folder::Project(B)));
    assert_eq!((again.project, outcome), (Some(A), Stayed));

    // A call that names both folders names one project.
    assert_eq!(Named::One(A).and(Named::One(B), &merges), Named::One(A));
    assert_eq!(
        Named::One(A).and(Named::One(B), &Merges::default()),
        Named::Many
    );
    assert_eq!(Named::One(A).and(Named::One(C), &merges), Named::Many);
    // Without the merge the same records are two switches.
    assert_eq!(
        run(working_on(A), &[write(B), at(Folder::Project(A))]),
        [(Some(B), Switched), (Some(B), Stayed)]
    );
}
