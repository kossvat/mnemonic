//! The real router under a midnight gap, isolated from the parent's timezone.
use super::*;
use chrono::{NaiveDate, TimeZone, Utc};

#[test]
fn timeline_router_midnight_gap_and_previous_date_both_answer() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "http::timeline::tests::zones::timeline_router_midnight_gap_child",
            "--nocapture",
        ])
        .env("TZ", "America/Santiago")
        .env("MNEMONIC_TIMELINE_GAP_CHILD", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("midnight gap route passed"));
}

#[tokio::test]
async fn timeline_router_midnight_gap_child() {
    if std::env::var("MNEMONIC_TIMELINE_GAP_CHILD").as_deref() != Ok("1") {
        return;
    }
    let f = Fixture::empty();
    let from = Utc.with_ymd_and_hms(2024, 9, 7, 4, 0, 0).unwrap();
    let transition = Utc.with_ymd_and_hms(2024, 9, 8, 4, 0, 0).unwrap();
    let to = Utc.with_ymd_and_hms(2024, 9, 9, 3, 0, 0).unwrap();
    f.segment("gap-fixture", "codex", true, Some(f.demo), from, to);
    f.active(from, to);
    let state = state(&f, true);
    let before = f.stores();
    let changes = f.changes();
    for (day, lower, upper, hours) in [
        ("2024-09-07", from, transition, 24),
        ("2024-09-08", transition, to, 23),
    ] {
        let date = NaiveDate::parse_from_str(day, "%Y-%m-%d").unwrap();
        assert_eq!(
            crate::activity::local_day_bounds_utc(date),
            Some((lower, upper))
        );
        let (status, value) = get(&state, &format!("/api/projects/timeline?day={day}")).await;
        assert_eq!(status, StatusCode::OK, "{day}: {value}");
        assert_eq!(value["from"], lower.timestamp() as f64);
        assert_eq!(value["to"], upper.timestamp() as f64);
        assert_eq!(
            value["to"].as_f64().unwrap() - value["from"].as_f64().unwrap(),
            hours as f64 * 3600.0
        );
        assert_eq!(value["segments"][0]["from"], value["from"]);
        assert_eq!(value["segments"][0]["to"], value["to"]);
        assert_eq!(value["owner"][0]["from"], value["from"]);
        assert_eq!(value["owner"][0]["to"], value["to"]);
    }
    assert_eq!(f.stores(), before);
    assert_eq!(f.changes(), changes);
    println!("midnight gap route passed");
}
