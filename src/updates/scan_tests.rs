use super::*;

fn keys(text: &str) -> Vec<String> {
    scan(text).values.into_iter().map(|v| v.key).collect()
}

fn prior(text: &str) -> Vec<String> {
    scan(text).prior.into_iter().map(|v| v.key).collect()
}

#[test]
fn money_in_every_common_shape() {
    for (text, key) in [
        ("price is $5", "usd:5"),
        ("price is $5.50", "usd:5.5"),
        ("price is $1,200", "usd:1200"),
        ("price is $1,200.00", "usd:1200"),
        ("price is 1.200,50 eur", "eur:1200.5"),
        ("price is €7", "eur:7"),
        ("price is 5 usd", "usd:5"),
        ("price is 5 dollars", "usd:5"),
        ("цена 5000 руб", "rub:5000"),
        ("цена 5 000 руб", "rub:5000"),
        ("цена 5 тыс руб", "rub:5000"),
        ("цена 5 рублей", "rub:5"),
        ("цена 5 долларов", "usd:5"),
        ("budget is $5k", "usd:5000"),
        ("budget is $2m", "usd:2000000"),
        ("plan is $9/month", "usd:9/month"),
        ("plan is $9 per month", "usd:9/month"),
        ("тариф 900 руб в месяц", "rub:900/month"),
        ("стоимость 7₽", "rub:7"),
        ("subscription is USD 6/month", "usd:6/month"),
        ("fee is EUR 12", "eur:12"),
    ] {
        assert_eq!(keys(text), vec![key.to_owned()], "{text}");
    }
}

#[test]
fn commercial_percentages_only() {
    assert_eq!(keys("commission is 15%"), vec!["pct:15"]);
    assert_eq!(keys("скидка 10 процентов"), vec!["pct:10"]);
    assert_eq!(keys("margin is 12.5 percent"), vec!["pct:12.5"]);
    assert!(keys("the new parser is 15% faster").is_empty());
    assert!(keys("coverage went up to 80%").is_empty());
}

#[test]
fn terms_dates_and_anchored_numbers() {
    assert_eq!(keys("payment terms net 30"), vec!["net:30"]);
    assert_eq!(keys("terms are Net-60"), vec!["net:60"]);
    assert_eq!(
        keys("launch deadline is 2026-10-15"),
        vec!["date:2026-10-15"]
    );
    assert_eq!(keys("due october 15 2026"), vec!["date:2026-10-15"]);
    assert_eq!(keys("срок до 15 октября"), vec!["date:10-15"]);
    assert_eq!(keys("answer limit is 99"), vec!["num:limit:99"]);
    assert_eq!(keys("лимит 40"), vec!["num:limit:40"]);
    assert_eq!(keys("moq 500"), vec!["num:moq:500"]);
    assert_eq!(keys("limit is 10 per day"), vec!["num:limit:10/day"]);
    // A date or number with nothing that says what it is: not a value.
    assert!(keys("session log 2026-09-23").is_empty());
    assert!(keys("fixed 4 bugs in the parser").is_empty());
}

#[test]
fn identifiers_and_measurements_are_masked() {
    for text in [
        "released v1.2.3",
        "released 0.153.4",
        "released v2",
        "server at 10.0.0.1",
        "dashboard on localhost:3000",
        "listens on :8080",
        "listens on port 8080",
        "merged d7e41a90 into main",
        "id 123e4567-e89b-12d3-a456-426614174000",
        "backup ran at 10:30",
        "started 2026-09-23T10:00:00Z",
        "started 2026-09-23 10:00",
        "export at 1920x1080",
        "export at 1080p",
        "runs at 30fps",
        "took 200ms",
        "took 200 ms",
        "uses 16gb",
        "see #42",
        "uploaded build 14",
        "step 3 of the plan",
        "open https://example.com/pricing/5",
        "mail ops@example.com",
        "edit src/main.rs",
        "edit /etc/app/5.conf",
        "run `price = 5`",
    ] {
        assert!(
            keys(&format!("price {text}")).is_empty(),
            "{text}: {:?}",
            keys(text)
        );
    }
}

#[test]
fn a_fenced_block_is_not_read() {
    let text = "Widget notes\n```\nprice = $5\n```\nnothing else";
    assert!(keys(text).is_empty());
}

#[test]
fn prior_values_are_kept_apart() {
    assert_eq!(keys("price went from $5 to $6"), vec!["usd:6"]);
    assert_eq!(prior("price went from $5 to $6"), vec!["usd:5"]);
    assert_eq!(keys("price is $6, was $5"), vec!["usd:6"]);
    assert_eq!(prior("цена 6 руб вместо 5 руб"), vec!["rub:5"]);
    assert_eq!(keys("price is $6 instead of $5"), vec!["usd:6"]);
    assert_eq!(keys("price is not $6, it is $5"), vec!["usd:5"]);
    assert_eq!(keys("цена не 6 руб, а 5 руб"), vec!["rub:5"]);
    // A cue before a value of several tokens covers all of it.
    assert!(keys("terms are not net 60").is_empty());
    assert_eq!(prior("terms were net 30"), vec!["net:30"]);
    assert_eq!(prior("price was USD 5"), vec!["usd:5"]);
    // Signs.
    assert_eq!(keys("margin is -5%"), vec!["pct:-5"]);
    assert_eq!(keys("fee is -$5"), vec!["usd:-5"]);
    // A dash inside a range is not a sign.
    assert!(!keys("price range 5-6").iter().any(|k| k.contains('-')));
}

#[test]
fn identifying_words_skip_markers_reasons_and_predicates() {
    let s = scan("Widget price is now $6 because of new tariffs");
    assert_eq!(s.words.iter().collect::<Vec<_>>(), vec!["widget"]);
    assert_eq!(s.preds.iter().collect::<Vec<_>>(), vec![&"price"]);
    let s = scan("Цена Widget теперь $6");
    assert_eq!(s.words.iter().collect::<Vec<_>>(), vec!["widget"]);
    assert_eq!(s.preds.iter().collect::<Vec<_>>(), vec![&"price"]);
}

#[test]
fn amounts_normalise() {
    assert_eq!(amount("5", 0).unwrap(), "5");
    assert_eq!(amount("5.50", 0).unwrap(), "5.5");
    assert_eq!(amount("1,200", 0).unwrap(), "1200");
    assert_eq!(amount("1.200,5", 0).unwrap(), "1200.5");
    assert_eq!(amount("5,5", 0).unwrap(), "5.5");
    assert_eq!(amount("1.5", 3).unwrap(), "1500");
    assert_eq!(amount("0.5", 6).unwrap(), "500000");
    assert_eq!(amount("-5", 0).unwrap(), "-5");
    assert_eq!(amount("007.10", 0).unwrap(), "7.1");
    // Exact: tiny per-unit prices stay apart.
    assert_ne!(amount("0.00001", 0), amount("0.00002", 0));
    assert_eq!(amount("0.00001", 0).unwrap(), "0.00001");
    assert!(amount("1.5.6", 0).is_none());
    assert_eq!(amount("0.005", 0).unwrap(), "0.005");
    assert_eq!(amount("1,005", 0).unwrap(), "1005");
    // Both separators: the last one is the decimal point.
    assert_eq!(amount("1,200.005", 0).unwrap(), "1200.005");
    assert_eq!(amount("1.200,5", 0).unwrap(), "1200.5");
    assert_eq!(amount("1,234,567", 0).unwrap(), "1234567");
    assert!(amount("1,20,300", 0).is_none());
    assert!(amount("1.200.5,5", 0).is_none());
}

#[test]
fn input_is_capped_on_a_char_boundary() {
    let long = format!("price is $5 {}", "я".repeat(MAX_INPUT_BYTES));
    assert_eq!(keys(&long), vec!["usd:5"]);
}

#[test]
fn compound_names_and_inflections_keep_their_identity() {
    let words = |text: &str| scan(text).words.into_iter().collect::<Vec<_>>();
    assert_ne!(
        words("Widget-Basic price is $5"),
        words("Widget-Premium price is $9")
    );
    assert_eq!(words("Widgets price is $5"), words("Widget price is $5"));
    assert_eq!(
        words("бюджет на рекламу 5 тыс руб"),
        words("бюджет на рекламы 5 тыс руб")
    );
}

#[test]
fn asked_proposed_and_bounded_values_are_hypothetical() {
    for text in [
        "Should Widget price be $6?",
        "Maybe Widget price $6",
        "Может, цена Widget $6",
        "Widget price is <$6",
        "Widget price is $5+",
        "Widget price is ~$6",
    ] {
        let s = scan(text);
        assert!(s.has_values() && s.hypothetical, "{text}: {s:?}");
    }
    assert!(!scan("Widget price is $6. Is that fine?").values.is_empty());
    assert!(!scan("Widget price is $6").hypothetical);
    assert!(!scan("Widget fee is $5 + 10%").values.is_empty());
}

#[test]
fn verbal_bounds_are_hypothetical_too() {
    for text in [
        "Widget price is under $6",
        "Widget price is up to $6",
        "Widget costs at least $6",
        "Widget price is less than $6",
        "Цена Widget не более 6 руб",
    ] {
        assert!(scan(text).hypothetical, "{text}");
    }
    assert!(!scan("Widget price is $6").hypothetical);
}
