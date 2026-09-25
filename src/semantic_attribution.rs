//! Semantic project attribution — deciding which project a memory belongs to by
//! the **meaning** of its embedding, not just explicit graph links.
//!
//! The current attribution only sees memories the extractor hard-linked to a
//! project entity. Work that left a memory but no explicit link (a cross-session
//! "Bumped the lockfile" captured as a generic decision) becomes
//! invisible or, worse, time bleeds to whichever project *was* linked nearby.
//!
//! This module adds the missing half via **k-NN**: an unlinked memory is
//! compared to a pool of project-linked memories, and the `k` nearest vote
//! (weighted by similarity) for their project. k-NN beats a per-project mean
//! (centroid) here because projects are multi-topic: a terse "Bumped the
//! lockfile" lands next to other dependency chores even when it's far from
//! the project's docs-heavy average. Attribution happens only when the
//! nearest neighbour clears `threshold` AND the winning project's vote share
//! beats the runner-up by `vote_margin`; otherwise Unattributed. Honest and
//! probabilistic: confidence follows similarity, weak/ambiguous → no guess.
//!
//! Pure decision core — no IO. The storage layer feeds it the pool + memory
//! embeddings; the daemon/CLI orchestrate. Used as an **advisory dry-run**:
//! show before→after; the DB write path is deliberately NOT wired (semantic
//! still mislabels semantically-adjacent but distinct projects).

use crate::embedding::{Embedding, cosine_similarity};
use std::collections::HashMap;
use std::sync::OnceLock;

/// User-supplied alias mappings (canonical project -> alias terms), loaded once
/// at startup from `[graph.aliases]` and merged ON TOP of the generic built-in
/// defaults. Every tool/project alias relationship beyond mnemonic's own names
/// comes from here, so none of them live in the source. Empty in library use
/// (init is only called from the binary); unit-test builds seed it with the
/// shared example projects (see `user_aliases`).
#[derive(Default)]
struct AliasMaps {
    /// alias term (lowercased) -> canonical project.
    canonical_of: HashMap<String, String>,
    /// canonical project (lowercased) -> extra alias terms.
    extra_aliases: HashMap<String, Vec<String>>,
}

impl AliasMaps {
    /// Build the lookup maps from a `[graph.aliases]`-shaped map (canonical
    /// project -> alias terms). Terms are trimmed and lowercased; blanks dropped.
    fn from_config(map: &HashMap<String, Vec<String>>) -> Self {
        let mut canonical_of = HashMap::new();
        let mut extra_aliases: HashMap<String, Vec<String>> = HashMap::new();
        for (canonical, aliases) in map {
            let canon = canonical.trim().to_lowercase();
            if canon.is_empty() {
                continue;
            }
            let terms: Vec<String> = aliases
                .iter()
                .map(|a| a.trim().to_lowercase())
                .filter(|a| !a.is_empty())
                .collect();
            for term in &terms {
                canonical_of.insert(term.clone(), canon.clone());
            }
            extra_aliases.entry(canon).or_default().extend(terms);
        }
        Self {
            canonical_of,
            extra_aliases,
        }
    }
}

static USER_ALIASES: OnceLock<AliasMaps> = OnceLock::new();

/// Merge the user's private `[graph.aliases]` map into attribution. Call once at
/// process startup (after `Config::load`). Idempotent — later calls are no-ops.
pub fn init_user_aliases(map: &HashMap<String, Vec<String>>) {
    let _ = USER_ALIASES.set(AliasMaps::from_config(map));
}

/// The active user alias maps, if any were configured.
#[cfg(not(test))]
fn user_aliases() -> Option<&'static AliasMaps> {
    USER_ALIASES.get()
}

/// Unit-test builds seed the maps with the shared example projects, so tests
/// exercise the same path a configured `[graph.aliases]` takes in production.
#[cfg(test)]
fn user_aliases() -> Option<&'static AliasMaps> {
    Some(USER_ALIASES.get_or_init(|| {
        AliasMaps::from_config(&HashMap::from([
            (
                "example-app".to_string(),
                vec![
                    "example-cli".to_string(),
                    "example-api".to_string(),
                    "example-web".to_string(),
                ],
            ),
            (
                "sample-service".to_string(),
                vec!["sample-worker".to_string()],
            ),
        ]))
    }))
}

/// Tunables for the semantic match. Defaults are deliberately conservative —
/// we would rather leave time Unattributed than mislabel a project.
#[derive(Debug, Clone)]
pub struct SemanticCfg {
    /// Minimum cosine to the nearest pooled memory to attribute at all.
    pub threshold: f32,
    /// Cosine at/above which a semantic match is reported High confidence.
    pub high: f32,
    /// k for k-NN: how many nearest project-linked memories vote.
    pub k: usize,
    /// The winning project's vote share must beat the runner-up's by at least
    /// this (of the top-k similarity mass), else Unattributed (ambiguous).
    pub vote_margin: f32,
}

impl Default for SemanticCfg {
    fn default() -> Self {
        Self {
            threshold: 0.55,
            high: 0.72,
            k: 12,
            vote_margin: 0.20,
        }
    }
}

/// How a single memory got (or didn't get) a project, with the evidence.
#[derive(Debug, Clone, PartialEq)]
pub struct Classification {
    /// The chosen project, or None when Unattributed.
    pub project_key: Option<String>,
    pub project_name: Option<String>,
    /// Best cosine seen.
    pub score: f32,
    /// Second-best cosine (for the margin check / explainability).
    pub runner_up: f32,
    /// "high"/"medium"/"low" when attributed; None when Unattributed.
    pub confidence: Option<&'static str>,
    /// Human-readable why, for the dry-run + debug.
    pub reason: String,
}

/// Collapse near-duplicate / tool-vs-project names into one canonical project,
/// so their memories form ONE centroid instead of overlapping ones (which the
/// margin check otherwise rejects as "ambiguous"). Conservative — only merges
/// names that are unambiguously the same project: mnemonic's own branch/tool
/// names built in, everything else from the user's `[graph.aliases]`. Unknown
/// names pass through unchanged (case preserved).
pub fn canonical_project(name: &str) -> String {
    let lower = name.trim().to_lowercase();
    // The user's private alias map wins, so real tool->project mappings stay
    // local (out of the public source) yet still drive attribution.
    if let Some(u) = user_aliases()
        && let Some(canon) = u.canonical_of.get(&lower)
    {
        return canon.clone();
    }
    match lower.as_str() {
        "fix-mnemonic" | "feat-mnemonic" | "mnemonic-eval-harness" => "mnemonic".into(),
        _ => name.to_string(),
    }
}

/// Inverse of `canonical_project`: the alias terms that should also be
/// recognised as a given canonical project when they appear in a memory title.
/// With `"example-app" = ["example-web"]` configured, "example-web search box
/// shipped" links to example-app even though the title never says
/// "example-app". Returns terms safe for whole-word matching.
pub fn aliases_for_project(canonical: &str) -> Vec<String> {
    let lower = canonical.trim().to_lowercase();
    let built_in: &[&str] = match lower.as_str() {
        "mnemonic" => &["fix-mnemonic", "feat-mnemonic"],
        _ => &[],
    };
    let mut out: Vec<String> = built_in.iter().map(|s| s.to_string()).collect();
    if let Some(u) = user_aliases()
        && let Some(extra) = u.extra_aliases.get(&lower)
    {
        out.extend(extra.iter().cloned());
    }
    out
}

/// One project-linked memory in the reference pool: which (canonical) project
/// it belongs to + its embedding.
#[derive(Debug, Clone)]
pub struct PoolItem {
    pub project_key: String,
    pub project_name: String,
    pub embedding: Embedding,
}

/// k-NN classification: instead of comparing to a project's *average*
/// (centroid), find the `cfg.k` nearest individual memories and let them vote,
/// weighted by similarity. This is far better for multi-topic projects: a
/// terse "Bumped the lockfile" lands next to *other* dependency chores
/// even when it's far from the project's docs-heavy mean.
///
/// Attributes only when the nearest neighbour clears `threshold` AND the
/// winning project's similarity-weighted vote share beats the runner-up by
/// `vote_margin`. Otherwise Unattributed — weak or genuinely between projects.
pub fn knn_classify(emb: &Embedding, pool: &[PoolItem], cfg: &SemanticCfg) -> Classification {
    use std::collections::HashMap;
    if emb.is_empty() || pool.is_empty() {
        return Classification {
            project_key: None,
            project_name: None,
            score: 0.0,
            runner_up: 0.0,
            confidence: None,
            reason: "no embedding / empty pool".into(),
        };
    }

    let mut sims: Vec<(usize, f32)> = pool
        .iter()
        .enumerate()
        .map(|(i, p)| (i, cosine_similarity(emb, &p.embedding)))
        .collect();
    sims.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let nearest = sims[0].1;
    if nearest < cfg.threshold {
        return Classification {
            project_key: None,
            project_name: None,
            score: nearest,
            runner_up: 0.0,
            confidence: None,
            reason: format!("unattributed: nearest {nearest:.2} < {:.2}", cfg.threshold),
        };
    }

    // Similarity-weighted votes among the k nearest neighbours.
    let k = cfg.k.max(1);
    let mut votes: HashMap<&str, (f32, &str)> = HashMap::new();
    let mut mass = 0.0f32;
    for (i, sim) in sims.iter().take(k) {
        if *sim <= 0.0 {
            continue;
        }
        let p = &pool[*i];
        let e = votes
            .entry(p.project_key.as_str())
            .or_insert((0.0, p.project_name.as_str()));
        e.0 += *sim;
        mass += *sim;
    }
    if votes.is_empty() || mass <= 0.0 {
        return Classification {
            project_key: None,
            project_name: None,
            score: nearest,
            runner_up: 0.0,
            confidence: None,
            reason: "unattributed: no positive neighbours".into(),
        };
    }

    let mut ranked: Vec<(&str, f32, &str)> = votes.iter().map(|(k, (s, n))| (*k, *s, *n)).collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let (best_key, best_sum, best_name) = ranked[0];
    let second_sum = ranked.get(1).map(|(_, s, _)| *s).unwrap_or(0.0);
    let best_share = best_sum / mass;
    let second_share = second_sum / mass;

    if best_share - second_share < cfg.vote_margin {
        return Classification {
            project_key: None,
            project_name: None,
            score: nearest,
            runner_up: second_share,
            confidence: None,
            reason: format!(
                "unattributed: ambiguous {best_name} {:.0}% vs runner {:.0}% (knn)",
                best_share * 100.0,
                second_share * 100.0
            ),
        };
    }

    let confidence = if nearest >= cfg.high && best_share >= 0.6 {
        "high"
    } else {
        "medium"
    };
    Classification {
        project_key: Some(best_key.to_string()),
        project_name: Some(best_name.to_string()),
        score: nearest,
        runner_up: second_share,
        confidence: Some(confidence),
        reason: format!(
            "knn→{best_name} {:.0}% (near {nearest:.2})",
            best_share * 100.0
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pi(key: &str, v: Vec<f32>) -> PoolItem {
        PoolItem {
            project_key: key.into(),
            project_name: key.into(),
            embedding: v,
        }
    }

    #[test]
    fn knn_matches_local_cluster_not_centroid() {
        // example-app pool is bimodal: docs-ish vs terse-chore-ish.
        // A new terse-chore memory is far from the *mean* but right next to
        // other chores, so k-NN attributes it where a centroid would not.
        let pool = vec![
            pi("example-app", vec![1.0, 0.0, 0.0]), // chore cluster
            pi("example-app", vec![0.98, 0.02, 0.0]),
            pi("example-app", vec![0.0, 1.0, 0.0]), // docs cluster
            pi("example-app", vec![0.0, 0.98, 0.0]),
            pi("mnemonic", vec![0.0, 0.0, 1.0]),
            pi("mnemonic", vec![0.0, 0.0, 0.98]),
        ];
        let cfg = SemanticCfg::default();
        let r = knn_classify(&vec![0.99, 0.01, 0.0], &pool, &cfg);
        assert_eq!(r.project_key.as_deref(), Some("example-app"), "{:?}", r);
    }

    #[test]
    fn knn_weak_when_nothing_near() {
        let pool = vec![pi("a", vec![1.0, 0.0]), pi("b", vec![0.0, 1.0])];
        // Orthogonal-ish to everything → nearest below threshold.
        let r = knn_classify(&vec![0.5, 0.5], &pool, &SemanticCfg::default());
        // cos to each is ~0.707; default threshold 0.55 → passes threshold but
        // votes split 50/50 → ambiguous (unattributed).
        assert!(r.project_key.is_none(), "{:?}", r);
    }

    #[test]
    fn canonical_collapses_known_aliases() {
        // Built-in mnemonic names, then configured aliases (the test seed).
        assert_eq!(canonical_project("fix-mnemonic"), "mnemonic");
        assert_eq!(canonical_project("example-cli"), "example-app");
        assert_eq!(canonical_project("sample-worker"), "sample-service");
        assert_eq!(canonical_project("unlisted-tool"), "unlisted-tool"); // untouched
        // The inverse lookup sees the same configured aliases.
        assert_eq!(
            aliases_for_project("example-app"),
            ["example-cli", "example-api", "example-web"]
        );
    }
}
