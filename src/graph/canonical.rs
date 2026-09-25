//! Entity name canonicalization.
//!
//! LLM extractors hallucinate freely on form: `Example Org`,
//! `example-org-co`, `customer-example-org` are the same company. The
//! graph treats them as 3 separate nodes — which fragments retrieval and
//! makes `mention_count` lie. Rule-based extractor mostly emits clean slugs,
//! but layered with LLM output we need a single chokepoint.
//!
//! Two operations:
//!
//! - `canonicalize_name(s)` — slugify + strip filler prefixes/suffixes
//!   (`the-`, `our-`, `client-`, `-app`, `-co`). Idempotent.
//! - `is_attribute_like(name)` — returns true for names that describe a
//!   value of something else (`sample-service-cost`, `example-org-release-date`)
//!   so the caller can drop them instead of growing junk nodes.

const FILLER_PREFIXES: &[&str] = &[
    "the-",
    "a-",
    "an-",
    "our-",
    "my-",
    "this-",
    "that-",
    "client-",
    "customer-",
    "user-",
];

const FILLER_SUFFIXES: &[&str] = &[
    "-app",
    "-application",
    "-co",
    "-corp",
    "-inc",
    "-llc",
    "-ltd",
    "-company",
    "-project",
];

/// Suffixes that mark the "name" as describing a value, not a thing.
/// These are graph attributes in disguise — drop the entity entirely.
const ATTRIBUTE_SUFFIXES: &[&str] = &[
    "-cost",
    "-price",
    "-amount",
    "-fee",
    "-budget",
    "-revenue",
    "-date",
    "-when",
    "-deadline",
    "-eta",
    "-status",
    "-priority",
    "-count",
    "-version",
];

/// Lowercase, hyphenate, trim filler. Returns empty string for unsalvageable
/// inputs — caller must check.
pub fn canonicalize_name(raw: &str) -> String {
    let mut slug = canonicalize_name_uncapped(raw);
    // Step 3: cap length. Long names hurt graph density and FK matching.
    if slug.chars().count() > 60 {
        slug = slug.chars().take(60).collect::<String>();
        slug = slug.trim_end_matches('-').to_string();
    }
    slug
}

/// `canonicalize_name` without the length cap: two long names that differ
/// only after the cap stay two names (fact subjects need that).
pub fn canonicalize_name_uncapped(raw: &str) -> String {
    let lower = raw.trim().to_lowercase();
    if lower.is_empty() {
        return String::new();
    }

    // Step 1: replace non-alphanumeric with '-', collapse runs, trim '-'.
    let mut slug: String = lower
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    // Collapse multiple dashes.
    while slug.contains("--") {
        slug = slug.replace("--", "-");
    }
    let mut slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        return String::new();
    }

    // Step 2: strip filler prefixes/suffixes (iteratively — "the-our-foo").
    let mut changed = true;
    while changed {
        changed = false;
        for p in FILLER_PREFIXES {
            if slug.starts_with(p) && slug.len() > p.len() {
                slug = slug[p.len()..].to_string();
                changed = true;
            }
        }
        for s in FILLER_SUFFIXES {
            if slug.ends_with(s) && slug.len() > s.len() {
                slug.truncate(slug.len() - s.len());
                changed = true;
            }
        }
        slug = slug.trim_matches('-').to_string();
    }
    slug
}

/// True if the name looks like an attribute/measurement, not a thing.
/// Examples:
///   "sample-service-cost"     → true   (cost is an attribute of the project)
///   "example-cli-deadline"    → true
///   "sample-service"          → false
///   "rust"                    → false
pub fn is_attribute_like(canonical: &str) -> bool {
    if canonical.is_empty() {
        return true; // can't be a useful entity
    }
    // Very short — likely junk.
    if canonical.chars().count() < 2 {
        return true;
    }
    // Attribute suffix detection (after canonicalization, all suffixes are
    // hyphenated lowercase).
    for s in ATTRIBUTE_SUFFIXES {
        if canonical.ends_with(s) {
            return true;
        }
    }
    // Pure-number / date-like / number-with-unit ("2026-05-13", "2k", "30m").
    // Heuristic: starts with a digit, or has no letters at all.
    let starts_with_digit = canonical.chars().next().is_some_and(|c| c.is_ascii_digit());
    if starts_with_digit {
        return true;
    }
    let has_letter = canonical.chars().any(|c| c.is_alphabetic());
    if !has_letter {
        return true;
    }
    // Long non-Latin single words with no hyphen are almost always non-English
    // verbs or adverbs that slipped past STOPWORDS (e.g. "кардинально",
    // "глобально", "автоматически" in a Russian-mixed memory). Short
    // non-Latin words like personal names stay — only word-length > 7 with
    // no Latin AND no hyphen triggers the drop.
    let has_latin = canonical.chars().any(|c| c.is_ascii_alphabetic());
    if !has_latin && !canonical.contains('-') && canonical.chars().count() > 7 {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn company_name_variants_collapse() {
        // All seven phrasings refer to the same company; canonicalize should
        // collapse them to one slug regardless of casing, filler prefix,
        // or "-co" / "Company" suffix.
        let cases = [
            "Example Org",
            "example-org",
            "example org co",
            "Example Org Co.",
            "customer-example-org",
            "our example org",
            "Example Org Company",
        ];
        let canons: Vec<String> = cases.iter().map(|c| canonicalize_name(c)).collect();
        let first = &canons[0];
        assert!(!first.is_empty());
        for (i, c) in canons.iter().enumerate() {
            assert_eq!(
                c, first,
                "case[{i}] '{}' → '{}' (expected '{}')",
                cases[i], c, first
            );
        }
    }

    #[test]
    fn project_name_variants_collapse() {
        let canon_a = canonicalize_name("Sample Service App");
        let canon_b = canonicalize_name("sample-service");
        let canon_c = canonicalize_name("the sample service project");
        let canon_d = canonicalize_name("SAMPLE_SERVICE");
        assert_eq!(canon_a, "sample-service");
        assert_eq!(canon_b, "sample-service");
        assert_eq!(canon_c, "sample-service");
        assert_eq!(canon_d, "sample-service");
    }

    #[test]
    fn idempotent() {
        let once = canonicalize_name("Example Org Co.");
        let twice = canonicalize_name(&once);
        assert_eq!(once, twice);
    }

    #[test]
    fn empty_and_punctuation_only() {
        assert_eq!(canonicalize_name(""), "");
        assert_eq!(canonicalize_name("   "), "");
        assert_eq!(canonicalize_name("---"), "");
        assert_eq!(canonicalize_name("!!!"), "");
    }

    #[test]
    fn caps_length_at_sixty_chars() {
        let long = "a".repeat(120);
        let canon = canonicalize_name(&long);
        assert!(canon.chars().count() <= 60);
    }

    #[test]
    fn attribute_suffixes_flagged() {
        assert!(is_attribute_like("sample-service-cost"));
        assert!(is_attribute_like("example-cli-price"));
        assert!(is_attribute_like("project-deadline"));
        assert!(is_attribute_like("auth-service-status"));
        assert!(is_attribute_like("retry-count"));
    }

    #[test]
    fn real_things_pass() {
        assert!(!is_attribute_like("sample-service"));
        assert!(!is_attribute_like("example-org"));
        assert!(!is_attribute_like("rust"));
        assert!(!is_attribute_like("auth-service"));
        assert!(!is_attribute_like("example-cli"));
    }

    #[test]
    fn number_only_or_too_short_flagged() {
        assert!(is_attribute_like("2k"));
        assert!(is_attribute_like("2026-05-13"));
        assert!(is_attribute_like(""));
        assert!(is_attribute_like("a"));
    }

    #[test]
    fn unicode_preserved() {
        // Non-ASCII glyphs in entity names should survive canonicalization
        // (Cyrillic example here, but the rule is general).
        let canon = canonicalize_name("Мирон");
        assert!(!canon.is_empty(), "got: '{canon}'");
        assert!(canon.chars().all(|c| c.is_alphanumeric() || c == '-'));
    }

    #[test]
    fn long_cyrillic_word_dropped_short_passes() {
        // Long non-Latin verbs/adverbs that snuck past STOPWORDS — gone.
        // (Examples are Russian: "gradually", "simultaneously", "finally".)
        assert!(is_attribute_like("постепенно"));
        assert!(is_attribute_like("одновременно"));
        assert!(is_attribute_like("окончательно"));
        // Short non-Latin words (e.g. personal names) — kept.
        assert!(!is_attribute_like("мирон"));
        assert!(!is_attribute_like("олег"));
        assert!(!is_attribute_like("игорь"));
        // Mixed alphabets with a hyphen are real (e.g. user-typed slug) — kept.
        assert!(!is_attribute_like("мирон-проект"));
    }

    #[test]
    fn nested_filler_iteratively_stripped() {
        // "the-our-foo-app" should reduce to "foo", not stop after one pass.
        let canon = canonicalize_name("The Our Foo App");
        assert_eq!(canon, "foo");
    }
}
