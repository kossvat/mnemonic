use super::*;
use std::sync::Arc;

#[test]
fn live_signal_the_identity_is_made_once_and_read_after() {
    let dir = crate::test_support::temp_dir("mn-live-id-");
    let made = identity(dir.path()).unwrap();
    assert_eq!(
        read_identity(&dir.path().join(IDENTITY_FILE)),
        Some(made.clone())
    );
    assert_eq!(identity(dir.path()).unwrap(), made);
    // It is the file the clients make and read, and nothing is left
    // beside it.
    assert_eq!(IDENTITY_FILE, "profile.id");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    assert_eq!(
        std::fs::read_to_string(dir.path().join(IDENTITY_FILE)).unwrap(),
        made
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.path().join(IDENTITY_FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    // Two profiles have two identities, and so two names, whatever port
    // they share.
    let other = crate::test_support::temp_dir("mn-live-id-");
    let theirs = identity(other.path()).unwrap();
    assert_ne!(name_of(&made), name_of(&theirs));
    assert!(name_of(&made).ends_with(&made));
}

#[test]
fn live_signal_a_daemon_and_a_client_that_start_at_once_get_one_identity() {
    let dir = crate::test_support::temp_dir("mn-live-id-");
    let home = Arc::new(dir.path().to_path_buf());
    let asked: Vec<_> = (0..8)
        .map(|_| {
            let home = home.clone();
            std::thread::spawn(move || identity(&home).unwrap())
        })
        .collect();
    let got: Vec<String> = asked.into_iter().map(|one| one.join().unwrap()).collect();
    assert!(got.iter().all(|value| *value == got[0]), "{got:?}");
}

#[test]
fn live_signal_a_file_that_is_not_an_identity_is_refused_and_kept() {
    let dir = crate::test_support::temp_dir("mn-live-id-");
    std::fs::write(dir.path().join(IDENTITY_FILE), "something else").unwrap();
    assert!(identity(dir.path()).is_err());
    assert_eq!(
        std::fs::read_to_string(dir.path().join(IDENTITY_FILE)).unwrap(),
        "something else"
    );
}

#[test]
fn live_signal_at_most_one_in_two_seconds_and_one_after_a_burst() {
    let start = Instant::now();
    let at = |millis: u64| start + Duration::from_millis(millis);
    let mut throttle = Throttle::default();
    // The first change is signalled at once, and nothing is owed.
    assert!(throttle.changed(at(0)));
    assert_eq!(throttle.due(), None);
    // Ten changes in the next second: none is signalled, one is owed.
    for step in 1..=10 {
        assert!(!throttle.changed(at(step * 100)));
    }
    assert_eq!(throttle.due(), Some(at(2000)));
    assert!(!throttle.tick(at(1999)));
    // It follows the burst, once.
    assert!(throttle.tick(at(2000)));
    assert_eq!(throttle.due(), None);
    assert!(!throttle.tick(at(2100)));
    // A change after a quiet time is signalled at once again.
    assert!(!throttle.changed(at(3000)));
    assert!(throttle.tick(at(4000)));
    assert!(throttle.changed(at(6000)));

    // Over a long burst: signals are two seconds apart or more.
    let mut throttle = Throttle::default();
    let mut posted = Vec::new();
    for millis in (0..20_000).step_by(50) {
        if throttle.changed(at(millis)) || throttle.tick(at(millis)) {
            posted.push(millis);
        }
    }
    assert!(posted.windows(2).all(|pair| pair[1] - pair[0] >= 2000));
    assert_eq!(posted.len(), 10);
}

#[test]
fn live_signal_an_identity_a_client_made_meanwhile_is_read_and_not_refused() {
    // The daemon and clients start at once, many times over: whoever
    // links the file first, every one of them gets its value.
    for _ in 0..40 {
        let dir = crate::test_support::temp_dir("mn-live-id-");
        let home = Arc::new(dir.path().to_path_buf());
        let asked: Vec<_> = (0..6)
            .map(|n| {
                let home = home.clone();
                std::thread::spawn(move || {
                    if n % 2 == 0 {
                        return identity(&home).unwrap();
                    }
                    // A client: its file written whole, linked into place.
                    let value = uuid::Uuid::new_v4().simple().to_string();
                    let scratch = home.join(format!("{IDENTITY_FILE}.client.{n}"));
                    std::fs::write(&scratch, &value).unwrap();
                    let linked = std::fs::hard_link(&scratch, home.join(IDENTITY_FILE));
                    std::fs::remove_file(&scratch).unwrap();
                    match linked {
                        Ok(()) => value,
                        Err(_) => read_identity(&home.join(IDENTITY_FILE)).unwrap(),
                    }
                })
            })
            .collect();
        let got: Vec<String> = asked.into_iter().map(|one| one.join().unwrap()).collect();
        assert!(got.iter().all(|value| *value == got[0]), "{got:?}");
    }
}
