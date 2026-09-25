use super::*;
use crate::updates::scan::scan;

/// How the save path would route `new` against an existing `old`:
/// `plain` when the new text carries no values (the old duplicate rule
/// applies unchanged), otherwise the verdict.
fn route(new: &str, old: &str) -> &'static str {
    let (a, b) = (scan(new), scan(old));
    if !a.has_values() {
        return "plain";
    }
    match verdict(&a, &b) {
        Verdict::Same => "same",
        Verdict::Update { .. } => "update",
        Verdict::Distinct => "distinct",
    }
}

const CORPUS: &[(&str, &str, &str)] = &[
    // Changed values of the same thing.
    ("Widget price is now $6", "Widget price is $5", "update"),
    ("Цена Widget теперь $6", "Цена Widget $5", "update"),
    (
        "Widget commission is 15%",
        "Widget commission is 10%",
        "update",
    ),
    (
        "Widget payment terms Net 60",
        "Widget payment terms Net 30",
        "update",
    ),
    ("Widget price is $500", "Widget price is $5", "update"),
    (
        "Widget price is now $6 (replaces the earlier fact: was $5)",
        "Widget price is $5",
        "update",
    ),
    (
        "Widget price is now $6 because of new tariffs",
        "Widget price is $5",
        "update",
    ),
    ("Answer limit is 99", "Answer limit is 42", "update"),
    (
        "Launch deadline is 2026-10-15",
        "Launch deadline is 2026-10-01",
        "update",
    ),
    ("Widget price is back to $5", "Widget price is $6", "update"),
    ("Widget costs $6 now", "Widget price is $5", "update"),
    (
        "Widget plan is $12/month",
        "Widget plan is $9/month",
        "update",
    ),
    (
        "Скидка для Widget теперь 20%",
        "Скидка для Widget 15%",
        "update",
    ),
    ("Комиссия партнёра 12%", "Комиссия партнёра 10%", "update"),
    (
        "Условия оплаты Gadget net 45",
        "Условия оплаты Gadget net 30",
        "update",
    ),
    (
        "Бюджет на рекламу 50 тыс руб",
        "Бюджет на рекламу 40 тыс руб",
        "update",
    ),
    ("Gadget price raised to €9", "Gadget price is €8", "update"),
    ("Gadget MOQ is 1000", "Gadget MOQ is 500", "update"),
    (
        "Gadget release date moved: due november 3 2026",
        "Gadget release: due october 20 2026",
        "update",
    ),
    (
        "Widget price is $6 (was $5)",
        "Widget price is $5",
        "update",
    ),
    (
        "Widget price went from $5 to $6",
        "Widget price is $5",
        "update",
    ),
    ("Цена Widget 6 000 руб", "Цена Widget 5 000 руб", "update"),
    ("Widget fee is now $3", "Widget fee is $2", "update"),
    (
        "Widget margin is 30 percent",
        "Widget margin is 25 percent",
        "update",
    ),
    ("Widget tax rate 8%", "Widget tax rate 7%", "update"),
    (
        "Widget price was raised to $6",
        "Widget price is $5",
        "update",
    ),
    ("Old Widget price is $6", "Old Widget price is $5", "update"),
    (
        "Widget subscription is USD 6/month",
        "Widget subscription is USD 5/month",
        "update",
    ),
    // The same statement again.
    (
        "Widget price went from $5 to $6",
        "Widget price is $6",
        "same",
    ),
    ("The price of Widget is $5", "Widget price is $5", "same"),
    ("Widget price is $1,200", "Widget price is $1200", "same"),
    ("Цена Widget 5 000 руб", "Цена Widget 5000 руб", "same"),
    ("Widget price is $5.00", "Widget price is $5", "same"),
    ("Widget price is 5 usd", "Widget price is $5", "same"),
    ("FYI Widget price is $5", "Widget price is $5", "same"),
    ("Widget price is $5k", "Widget price is $5,000", "same"),
    ("Widget commission: 15%", "Widget commission is 15%", "same"),
    ("Цена на Widget: $5", "Цена Widget $5", "same"),
    // Different things, or not comparable.
    (
        "Gadget price is $5 per month",
        "Widget price is $5 per month",
        "distinct",
    ),
    (
        "Widget shipping price is $6",
        "Widget price is $5",
        "distinct",
    ),
    (
        "Widget price is $9 for the pro plan",
        "Widget price is $5",
        "distinct",
    ),
    ("цена на чай 5$", "цена на кофе 5$", "distinct"),
    ("Widget price is $6", "Widget fee is $5", "distinct"),
    ("Gadget price is $5", "Widget price is $5", "distinct"),
    (
        "Widget price is $6",
        "Widget notes without any values",
        "distinct",
    ),
    ("Widget price is $6", "Widget commission is 10%", "distinct"),
    (
        "Widget price is $6 per month",
        "Widget price is $6 per year",
        "distinct",
    ),
    ("Widget budget is $500", "Widget price is $500", "distinct"),
    (
        "Widget price is $5 in Europe",
        "Widget price is $5 in Canada",
        "distinct",
    ),
    (
        "Gadget commission is 15%",
        "Widget commission is 10%",
        "distinct",
    ),
    (
        "Deadline for Widget is 2026-10-15",
        "Deadline for Gadget is 2026-10-15",
        "distinct",
    ),
    (
        "Widget answer limit is 99",
        "Gadget answer limit is 42",
        "distinct",
    ),
    (
        "Widget price is $5. Gadget price is $6.",
        "Widget price is $6. Gadget price is $5.",
        "distinct",
    ),
    // A title that repeats the first line states its value once.
    (
        "Widget price is $6\nWidget price is $6",
        "Widget price\nWidget price is $5",
        "update",
    ),
    // Several predicates: only the pairing tells which value is whose.
    (
        "Widget commission is 10%; Widget discount is 20%",
        "Widget discount is 10%; Widget commission is 20%",
        "distinct",
    ),
    (
        "Widget commission is 10%; Widget discount is 20%",
        "Widget commission is 10%; Widget discount is 20%",
        "same",
    ),
    // Several subjects under one predicate: the pairing tells them apart.
    (
        "Widget price $5; Gadget price $6",
        "Gadget price $5; Widget price $6",
        "distinct",
    ),
    (
        "Widget price $5; Gadget price $6",
        "Widget price $5; Gadget price $6",
        "same",
    ),
    // The same amount for another subject is its own mention.
    (
        "Widget price $5; Gadget price $6; Thing price $5",
        "Widget price $5; Gadget price $6; Thing price $6",
        "distinct",
    ),
    // A number in a name or a unit tells two things apart.
    ("Widget 2 price is $6", "Widget 1 price is $5", "distinct"),
    (
        "Widget price is $5 per 1000 units",
        "Widget price is $5 per 100 units",
        "distinct",
    ),
    // A sign is part of the value.
    ("Widget margin is -5%", "Widget margin is 5%", "update"),
    // A limit per day and one per year are two limits.
    (
        "Widget limit is 10 per day",
        "Widget limit is 10 per year",
        "distinct",
    ),
    (
        "Widget limit is 12 per day",
        "Widget limit is 10 per year",
        "distinct",
    ),
    (
        "Widget limit is 12 per day",
        "Widget limit is 10 per day",
        "update",
    ),
    // A unit the lexicon does not know still tells prices apart.
    (
        "Widget price is $6 per pallet",
        "Widget price is $5 per square foot",
        "distinct",
    ),
    // Variants of one product are different things.
    (
        "Widget-Premium price is $9",
        "Widget-Basic price is $5",
        "distinct",
    ),
    // A question, a proposal or a bound asserts no value.
    (
        "Should Widget price be $6?",
        "Widget price is $5",
        "distinct",
    ),
    ("Maybe Widget price $6", "Widget price is $5", "distinct"),
    ("Widget price is <$6", "Widget price is $5", "distinct"),
    ("Widget price is $5+", "Widget price is $5", "distinct"),
    // One value each, about different subjects.
    (
        "Widget price is $6. Gadget is free.",
        "Gadget price is $5. Widget is free.",
        "distinct",
    ),
    ("Widget price is under $6", "Widget price is $5", "distinct"),
    // A single letter can be the whole difference.
    (
        "Widget plan B price is $6",
        "Widget plan C price is $5",
        "distinct",
    ),
    // A pricing basis and a long name's tail are identity.
    (
        "Widget price is $6 for all",
        "Widget price is $5 each",
        "distinct",
    ),
    (
        "WidgetConfigurationB price is $6",
        "WidgetConfigurationA price is $5",
        "distinct",
    ),
    // The quantity a price is per is part of what it is.
    (
        "Widget price is $6 per 500 tokens",
        "Widget price is $5 per 1000 tokens",
        "distinct",
    ),
    (
        "Widget price is $5 per 500 tokens",
        "Widget price is $5 per 1000 tokens",
        "distinct",
    ),
    // Before or after tax are two prices.
    (
        "Widget price is $6 after tax",
        "Widget price is $5 before tax",
        "distinct",
    ),
    // Words that merely start like a currency are products.
    ("Цена рубильника $6", "Цена рубашки $5", "distinct"),
    // No values in the new text: the plain duplicate rule decides.
    (
        "Released mnemonic v0.153.5",
        "Released mnemonic v0.153.4",
        "plain",
    ),
    (
        "Dashboard runs on localhost:3001",
        "Dashboard runs on localhost:3000",
        "plain",
    ),
    (
        "Merged d7e41a90 into main",
        "Merged 5f3c28bd into main",
        "plain",
    ),
    ("Backup ran at 11:00", "Backup ran at 10:30", "plain"),
    ("Fixed 4 bugs in parser", "Fixed 3 bugs in parser", "plain"),
    (
        "Uploaded build 14 to TestFlight",
        "Uploaded build 13 to TestFlight",
        "plain",
    ),
    (
        "demo-app 2026-09-23: session log",
        "demo-app 2026-09-22: session log",
        "plain",
    ),
    (
        "The new parser is 15% faster",
        "The new parser is 10% faster",
        "plain",
    ),
    ("Coverage is up to 80%", "Coverage is up to 75%", "plain"),
    (
        "Export renders at 1080p and 30fps",
        "Export renders at 720p and 24fps",
        "plain",
    ),
    ("Listen on port 8081", "Listen on port 8080", "plain"),
    ("Step 4 done", "Step 3 done", "plain"),
    ("Session took 200ms", "Session took 150ms", "plain"),
    ("See issue #42", "See issue #41", "plain"),
    // A denied value is not stated: nothing to compare.
    ("Widget price is not $6", "Widget price is $5", "plain"),
    ("Цена Widget не $6", "Цена Widget $5", "plain"),
    (
        "Widget payment terms are not Net 60",
        "Widget payment terms are Net 30",
        "plain",
    ),
    (
        "Widget price is not currently $6",
        "Widget price is $5",
        "plain",
    ),
    ("Widget does not cost $6", "Widget costs $5", "plain"),
    ("Deploy commit a1b2c3d", "Deploy commit e4f5a6b", "plain"),
];

#[test]
fn corpus_routes_every_case() {
    let mut misses = Vec::new();
    let mut counts = std::collections::BTreeMap::new();
    for (new, old, want) in CORPUS {
        let got = route(new, old);
        *counts.entry(got).or_insert(0) += 1;
        if got != *want {
            misses.push(format!("{want} but {got}: {new:?} vs {old:?}"));
        }
    }
    assert!(misses.is_empty(), "{misses:#?}");
    assert!(CORPUS.len() >= 60, "{}", CORPUS.len());
    assert_eq!(
        counts,
        [
            ("distinct", 37),
            ("plain", 20),
            ("same", 12),
            ("update", 31)
        ]
        .into_iter()
        .collect()
    );
}

#[test]
fn an_update_names_what_was_and_what_is() {
    let (new, old) = (scan("Widget price is now $6"), scan("Widget price is $5"));
    let Verdict::Update { class, was, now } = verdict(&new, &old) else {
        panic!("not an update");
    };
    assert_eq!(class, Class::Money);
    let surfaces = |values: &[crate::updates::scan::Value]| -> Vec<String> {
        values.iter().map(|v| v.surface.clone()).collect()
    };
    assert_eq!(surfaces(&was), vec!["$5"]);
    assert_eq!(surfaces(&now), vec!["$6"]);
    assert_eq!(now[0].key, "usd:6");
}

#[test]
fn value_less_on_either_side_is_distinct() {
    let valued = scan("Widget price is $5");
    let plain = scan("Widget notes");
    assert_eq!(verdict(&valued, &plain), Verdict::Distinct);
    assert_eq!(verdict(&plain, &valued), Verdict::Distinct);
}

#[test]
fn measured_and_numbered_variants_are_different_things() {
    assert_eq!(
        route("RAM 32GB price is $60", "RAM 16GB price is $50"),
        "distinct"
    );
    assert_eq!(
        route(
            "Widget price for phase 2 is $6",
            "Widget price for phase 1 is $5"
        ),
        "distinct"
    );
    assert_eq!(
        route("Invoice #13 total is $600", "Invoice #12 total is $500"),
        "distinct"
    );
    // The same variant still updates, and repeats still match.
    assert_eq!(
        route("RAM 16GB price is now $55", "RAM 16GB price is $50"),
        "update"
    );
    assert_eq!(
        route("RAM 16GB price is $50", "RAM 16GB price is $50"),
        "same"
    );
}
