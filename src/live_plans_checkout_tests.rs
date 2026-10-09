use super::*;

fn repository(root: &Path) -> git2::Repository {
    std::fs::create_dir_all(root).unwrap();
    let mut options = git2::RepositoryInitOptions::new();
    options.external_template(false).initial_head("main");
    let repo = git2::Repository::init_opts(root, &options).unwrap();
    let tree_id = repo.index().unwrap().write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let email = ["fixture", "invalid"].join("@");
    let signature = git2::Signature::new("Fixture", &email, &git2::Time::new(1, 0)).unwrap();
    repo.commit(Some("HEAD"), &signature, &signature, "Fixture", &tree, &[])
        .unwrap();
    drop(tree);
    repo
}

#[test]
fn plan_checkout_rejects_unrelated_standalone_and_monorepo_repositories() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    let standalone = workspace.join("standalone");
    repository(&standalone);
    let other = temp.path().join("unrelated");
    repository(&other);
    assert!(checkout_of(&standalone, &workspace, &other).is_none());

    let monorepo = workspace.join("monorepo");
    repository(&monorepo);
    let main = monorepo.join("projects/demoapp");
    std::fs::create_dir_all(&main).unwrap();
    let other_project = other.join("projects/demoapp");
    std::fs::create_dir_all(&other_project).unwrap();
    assert!(checkout_of(&main, &monorepo, &other).is_none());
    assert!(checkout_of(&main, &monorepo, &other_project).is_none());
    assert_eq!(
        checkout_of(&main, &monorepo, &main),
        Some(main.canonicalize().unwrap())
    );
}

#[test]
fn plan_checkout_accepts_only_linked_worktrees_of_the_same_git_directory() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    let repo = repository(&workspace);
    let linked = temp.path().join("linked");
    repo.worktree("linked", &linked, None).unwrap();
    assert_eq!(
        checkout_of(&workspace, &workspace, &linked),
        Some(linked.canonicalize().unwrap())
    );
    let main = workspace.join("projects/demoapp");
    let project = linked.join("projects/demoapp");
    let nested = project.join("src");
    std::fs::create_dir_all(&main).unwrap();
    std::fs::create_dir_all(&nested).unwrap();
    for cwd in [&linked, &project, &nested] {
        assert_eq!(
            checkout_of(&main, &workspace, cwd),
            Some(project.canonicalize().unwrap())
        );
    }
    // A separate nested repository inside the linked tree is unrelated too.
    let unrelated = nested.join("vendor");
    repository(&unrelated);
    assert!(checkout_of(&main, &workspace, &unrelated).is_none());
}

#[test]
fn plan_checkout_non_git_fallback_stays_in_the_configured_project() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    let projects = workspace.join("projects");
    let main = projects.join("demoapp");
    let nested = main.join("src");
    std::fs::create_dir_all(&nested).unwrap();
    for cwd in [&workspace, &projects, &main, &nested] {
        assert_eq!(
            checkout_of(&main, &workspace, cwd),
            Some(main.canonicalize().unwrap())
        );
    }
    let elsewhere = temp.path().join("elsewhere/projects/demoapp");
    std::fs::create_dir_all(&elsewhere).unwrap();
    assert!(checkout_of(&main, &workspace, &elsewhere).is_none());
    assert!(checkout_of(&main, &workspace, Path::new("projects/demoapp")).is_none());
}

#[test]
fn plan_checkout_finds_a_linked_project_omitted_from_the_main_checkout() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    let repo = repository(&workspace);
    let linked = temp.path().join("linked");
    repo.worktree("linked", &linked, None).unwrap();
    let main = workspace.join("projects/demoapp");
    let project = linked.join("projects/demoapp");
    let nested = project.join("src");
    std::fs::create_dir_all(&nested).unwrap();
    assert!(!workspace.join("projects").exists());
    for cwd in [&linked, &project, &nested] {
        assert_eq!(
            checkout_of(&main, &workspace, cwd),
            Some(project.canonicalize().unwrap())
        );
    }
    // A missing main folder does not authorize an identically named folder
    // from another repository or a traversal outside the workspace.
    let other = temp.path().join("unrelated");
    repository(&other);
    let other_project = other.join("projects/demoapp");
    std::fs::create_dir_all(&other_project).unwrap();
    assert!(checkout_of(&main, &workspace, &other_project).is_none());
    assert!(
        checkout_of(
            &workspace.join("../unrelated/projects/demoapp"),
            &workspace,
            &linked
        )
        .is_none()
    );
    repository(&project);
    assert!(checkout_of(&main, &workspace, &linked).is_none());
}

#[cfg(unix)]
#[test]
fn plan_checkout_missing_main_path_does_not_follow_symlinks_outside_either_checkout() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("workspace");
    let repo = repository(&workspace);
    let linked = temp.path().join("linked");
    repo.worktree("linked", &linked, None).unwrap();
    let main = workspace.join("projects/demoapp");
    let project = linked.join("projects/demoapp");
    std::fs::create_dir_all(&project).unwrap();
    let outside = temp.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    symlink(&outside, workspace.join("projects")).unwrap();
    assert!(checkout_of(&main, &workspace, &linked).is_none());
    std::fs::remove_file(workspace.join("projects")).unwrap();
    std::fs::remove_dir_all(linked.join("projects")).unwrap();
    std::fs::create_dir_all(outside.join("demoapp")).unwrap();
    symlink(&outside, linked.join("projects")).unwrap();
    assert!(checkout_of(&main, &workspace, &linked).is_none());
}
