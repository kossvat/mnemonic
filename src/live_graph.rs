//! Cached curated graph. Only explicit/background refreshes read evidence
//! and allocate durable slots; GET renders from the last owned input.
use super::Live;
use crate::activity::ActivityStore;
use crate::config::Config;
use crate::curated_graph::{self, Hours, Input, PlanUnit, Snapshot};
use crate::owner_time_day::{Calendar, LocalCalendar};
use crate::storage::Storage;
use crate::storage::curated_graph::{EdgePage, Evidence, GraphSlot, LinkKind, unit_key};
use anyhow::Result;
use chrono::{DateTime, Utc};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[path = "live_graph_data.rs"]
mod data;

const EXPANSION_CACHE: usize = 8;

struct Source {
    storage: Arc<Storage>,
    tech: Vec<String>,
    people: Vec<String>,
    /// Serialize preparation, including plan reads, without holding `graph`.
    refresh: Mutex<()>,
}

struct Prepared {
    evidence: Evidence,
    units: Vec<PlanUnit>,
    slots: Vec<GraphSlot>,
    hours: BTreeMap<i64, Hours>,
    now: i64,
}

impl Prepared {
    fn input(&self) -> Input<'_> {
        Input {
            evidence: &self.evidence,
            units: &self.units,
            slots: &self.slots,
            hours_30d: &self.hours,
            now: self.now,
        }
    }
}

#[derive(Default)]
pub(super) struct Graph {
    source: Option<Arc<Source>>,
    activity: Option<Arc<ActivityStore>>,
    prepared: Option<Prepared>,
    material: Option<String>,
    overview: Option<Arc<Snapshot>>,
    expansions: BTreeMap<String, Arc<Snapshot>>,
}

fn vocabulary(config: &Config) -> (Vec<String>, Vec<String>) {
    let canonical = crate::graph::canonical::canonicalize_name;
    let deny: BTreeSet<_> = config.graph.deny.iter().map(|s| canonical(s)).collect();
    let clean = |names: Vec<&str>| {
        names
            .into_iter()
            .filter_map(|raw| {
                let name = canonical(raw);
                let uncapped = crate::graph::canonical::canonicalize_name_uncapped(raw);
                // Slugging can remove assignment syntax or create a token
                // shape; the length cap must not conceal that shape either.
                crate::redaction::state::check_identities([raw, name.as_str(), uncapped.as_str()])
                    .ok()?;
                (!name.is_empty() && !deny.contains(&name)).then_some(name)
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    };
    (
        clean(
            crate::graph::extractor::KNOWN_TECH
                .iter()
                .copied()
                .chain(config.graph.tech.iter().map(String::as_str))
                .collect(),
        ),
        clean(config.graph.people.iter().map(String::as_str).collect()),
    )
}

impl Live {
    /// Called once during startup after plans are configured. An absent
    /// workspace disables this projection without opening another store.
    pub fn configure_graph(&self, storage: Arc<Storage>, config: &Config) -> Result<()> {
        let source = config.projects.root().map(|_| {
            let (tech, people) = vocabulary(config);
            Arc::new(Source {
                storage,
                tech,
                people,
                refresh: Mutex::new(()),
            })
        });
        {
            let mut graph = self.graph.lock().unwrap_or_else(|held| held.into_inner());
            *graph = Graph {
                source,
                ..Graph::default()
            };
        }
        self.refresh_graph()?;
        Ok(())
    }

    /// Attach the daemon's already-open activity handle. Missing/disabled
    /// activity stays unknown; graph reads never create an activity database.
    pub fn attach_graph_activity(&self, activity: Arc<ActivityStore>) -> Result<bool> {
        self.graph
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .activity = Some(activity);
        self.refresh_graph()
    }

    pub fn refresh_graph(&self) -> Result<bool> {
        self.refresh_graph_at(&LocalCalendar, Utc::now(), Instant::now())
    }

    fn refresh_graph_at(
        &self,
        calendar: &dyn Calendar,
        now: DateTime<Utc>,
        clock: Instant,
    ) -> Result<bool> {
        let source = self
            .graph
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .source
            .clone();
        let Some(source) = source else {
            return Ok(false);
        };
        let _refresh = source
            .refresh
            .lock()
            .unwrap_or_else(|held| held.into_inner());
        // No graph lock is held while plans read files or publish live lines.
        self.refresh_plans_at(now, clock, true);
        let plans = self.project_plans();
        let activity = self
            .graph
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .activity
            .clone();
        let names = plans
            .iter()
            .flat_map(|(id, p)| p.units.iter().map(move |u| (*id, u.id.clone())))
            .collect::<Vec<_>>();
        let named = source.storage.project_unit_memory_activity(&names)?;
        let units = plans
            .into_iter()
            .flat_map(|(project_id, plan)| {
                plan.units
                    .into_iter()
                    .map(|unit| {
                        let (memory_count, last_memory) = named
                            .get(&(project_id, unit.id.clone()))
                            .copied()
                            .unwrap_or_default();
                        PlanUnit {
                            project_id,
                            id: unit.id,
                            title: unit.title,
                            closed_at: unit.closed_at.or_else(|| {
                                (unit.stage == crate::plans::Stage::Done).then_some(unit.as_of)
                            }),
                            last_memory,
                            memory_count,
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let evidence = source
            .storage
            .curated_graph_evidence(&source.tech, &source.people)?;
        let hours = data::hours(&source.storage, activity.as_deref(), calendar, now)?;
        // The only graph-state write; skip the transaction entirely on a
        // no-op refresh. Storage arbitrates concurrent additions atomically.
        let mut slots = source.storage.graph_slots()?;
        let known: BTreeSet<_> = slots.iter().map(|s| s.key.as_str()).collect();
        let missing = curated_graph::slot_requests(&evidence, &units)
            .into_iter()
            .filter(|r| !known.contains(r.key.as_str()))
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            source.storage.ensure_graph_slots(&missing)?;
            slots = source.storage.graph_slots()?;
        }
        let prepared = Prepared {
            evidence,
            units,
            slots,
            hours,
            now: now.timestamp(),
        };
        let material = data::material(&prepared)?;
        let overview = Arc::new(curated_graph::render(prepared.input(), None)?);
        let changed = {
            let mut graph = self.graph.lock().unwrap_or_else(|held| held.into_inner());
            // An old preparation can never overwrite a new configuration.
            if !graph
                .source
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &source))
            {
                return Ok(false);
            }
            let changed = graph.material.as_ref() != Some(&material);
            graph.material = Some(material);
            graph.prepared = Some(prepared);
            graph.overview = Some(overview);
            graph.expansions.clear();
            changed
        };
        if changed {
            self.signal(true, clock);
            self.wake.notify_one();
        }
        Ok(changed)
    }

    /// Entirely in-memory, including a first expansion. No refresh, file read,
    /// database access or signal occurs here. Unknown focuses return `None`.
    pub fn graph_snapshot(&self, expand: Option<&str>) -> Result<Option<Arc<Snapshot>>> {
        let mut graph = self.graph.lock().unwrap_or_else(|held| held.into_inner());
        let Some(key) = expand else {
            return Ok(graph.overview.clone());
        };
        if let Some(snapshot) = graph.expansions.get(key) {
            return Ok(Some(snapshot.clone()));
        }
        let Some(prepared) = &graph.prepared else {
            return Ok(None);
        };
        if !curated_graph::contains(&prepared.input(), key) {
            return Ok(None);
        }
        let snapshot = Arc::new(curated_graph::render(prepared.input(), Some(key))?);
        if graph.expansions.len() >= EXPANSION_CACHE {
            graph.expansions.pop_first();
        }
        graph.expansions.insert(key.into(), snapshot.clone());
        Ok(Some(snapshot))
    }

    pub fn graph_edge_memories(
        &self,
        source_key: &str,
        target_key: &str,
        kind: LinkKind,
        page: EdgePage,
    ) -> Result<Option<Vec<String>>> {
        let (source, unit) = {
            let graph = self.graph.lock().unwrap_or_else(|held| held.into_inner());
            let unit = (kind == LinkKind::Contains && target_key.starts_with("unit:")).then(|| {
                graph.prepared.as_ref().and_then(|prepared| {
                    prepared
                        .units
                        .iter()
                        .find(|unit| {
                            source_key == format!("project:{}", unit.project_id)
                                && unit_key(unit.project_id, &unit.id).as_deref()
                                    == Some(target_key)
                                && curated_graph::contains(&prepared.input(), target_key)
                        })
                        .map(|unit| {
                            let known = prepared
                                .units
                                .iter()
                                .map(|unit| (unit.project_id, unit.id.clone()))
                                .collect::<Vec<_>>();
                            (unit.project_id, unit.id.clone(), known)
                        })
                })
            });
            (graph.source.clone(), unit)
        };
        source
            .map(|source| {
                if let Some(unit) = unit {
                    return unit.map_or_else(
                        || Ok(Vec::new()),
                        |(project, unit, known)| {
                            source
                                .storage
                                .curated_graph_unit_memories(project, &unit, &known, page)
                        },
                    );
                }
                source.storage.curated_graph_edge_memories(
                    source_key,
                    target_key,
                    kind,
                    &source.tech,
                    &source.people,
                    page,
                )
            })
            .transpose()
    }

    #[cfg(test)]
    #[allow(dead_code)] // The binary HTTP tests use this fixture-only hook.
    pub(crate) fn set_graph_snapshot(&self, expand: Option<&str>, snapshot: Arc<Snapshot>) {
        let mut graph = self.graph.lock().unwrap_or_else(|held| held.into_inner());
        if let Some(key) = expand {
            if graph.expansions.len() >= EXPANSION_CACHE {
                graph.expansions.pop_first();
            }
            graph.expansions.insert(key.into(), snapshot);
        } else {
            graph.overview = Some(snapshot);
        }
    }
}

#[cfg(test)]
#[path = "live_graph_tests.rs"]
mod tests;
