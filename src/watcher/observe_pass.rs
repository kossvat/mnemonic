//! One transactional attempt of the observation pass.
use super::*;
use crate::plans::location::Location;
use crate::storage::observation::Batch;
use anyhow::ensure;

impl Observation {
    /// One attempt. `None` when the stored checkpoint was not `start` any
    /// more at the write.
    pub(super) fn once(
        &self,
        path: &Path,
        stream: &str,
        map: &ProjectMap,
        start: Option<&Checkpoint>,
    ) -> Result<Option<(Done, FileStamp)>> {
        // Stat, read and fingerprint the same descriptor: a path may be
        // replaced at any time.
        let mut file = File::open(path)?;
        let meta = file.metadata()?;
        let read = FileStamp::of(&meta);
        if start.is_none() && !self.history {
            let quiet = meta
                .modified()
                .ok()
                .and_then(|at| at.elapsed().ok())
                .is_some_and(|age| age.as_secs() >= self.idle_secs);
            if quiet {
                return Ok(Some((Done::Left, read)));
            }
        }
        let resumed = match start {
            Some(start) => unchanged(&mut file, &meta, &start.cursor)?.map(|hash| (start, hash)),
            None => None,
        };
        // A transcript that was replaced or rewritten starts from an empty
        // state, and what was kept of it is dropped with this write.
        let replaced = start.is_some() && resumed.is_none();
        let (generation, offset) = match (&resumed, start) {
            (Some((start, _)), _) => (start.cursor.generation, start.cursor.offset),
            (None, Some(start)) => (
                start
                    .cursor
                    .generation
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("generation exhausted"))?,
                0,
            ),
            (None, None) => (0, 0),
        };
        let held_location = if replaced {
            None
        } else {
            self.live.as_ref().and_then(|live| live.location(stream))
        };
        let recover = self.live.is_some()
            && held_location.is_none()
            && offset > 0
            && resumed.as_ref().is_some_and(|(start, _)| {
                start
                    .last_at
                    .is_some_and(|last| (0..=30 * 60).contains(&(Utc::now() - last).num_seconds()))
            });
        let mut location = self
            .live
            .as_ref()
            .map(|_| held_location.unwrap_or_default());
        if recover {
            location = Some(Location::from_prefix(&mut file, offset, self.format.agent)?);
        }
        let chunk = tail::read_snapshot(&mut file, meta.len(), offset)?;
        if resumed.is_some() && chunk.next_offset == offset {
            if recover && let (Some(live), Some(location)) = (&self.live, location) {
                let after = file.metadata()?;
                ensure!(
                    FileStamp::of(&after) == read,
                    "transcript changed while recovering its location"
                );
                ensure!(
                    unchanged(
                        &mut file,
                        &after,
                        &start.expect("resumed checkpoint").cursor
                    )?
                    .is_some(),
                    "transcript rewritten while recovering its location"
                );
                // Empty metadata also records that recovery was done, but
                // only if its session still matches this checkpoint.
                if let Some(session) =
                    self.live_session(stream, start.expect("resumed checkpoint"))?
                {
                    live.recover_location(stream, &session, location);
                }
            }
            return Ok(Some((Done::AtEnd, read)));
        }

        let (state, mut program, mut last_kind, mut last_at) = match &resumed {
            Some((start, _)) => (start.state, start.program, start.last_kind, start.last_at),
            None => (State::default(), None, Kind::Other, None),
        };
        let open = match &resumed {
            Some(_) => self.storage.open_segment(stream)?,
            None => None,
        };
        let mut walker = Walker::resume(self.idle_secs, state, open).merged(map.merges().clone());
        let mut prompts = Vec::new();
        for (_, line) in chunk.records(offset) {
            let Some(observed) =
                (self.format.read)(line.trim_end_matches('\n'), map, location.as_mut())
            else {
                continue;
            };
            // Who drives the session is decided once, by the first record
            // that says how it was started.
            if program.is_none() {
                program = observed.program;
            }
            walker.record(&observed, None);
            let Some(at) = observed.at else {
                continue;
            };
            if observed.kind == Kind::Prompt && program != Some(true) {
                prompts.push(at);
            }
            // A record copied by a compaction carries an earlier time. It
            // moves no time of a segment, and it is not the last thing the
            // session did.
            if last_at.is_none_or(|last| at >= last) {
                last_at = Some(at);
                last_kind = observed.kind;
            }
        }
        // A session a program ran has no prompt of the owner, whenever in
        // the pass the record came that says who ran it.
        if program == Some(true) {
            prompts.clear();
        }
        let state = walker.state();
        let (segments, _) = walker.finish();
        let cursor = checkpoint(
            &mut file,
            &meta,
            stream.to_string(),
            generation,
            chunk.next_offset,
            resumed
                .as_ref()
                .map(|(start, hash)| (start.cursor.prefix_len, hash.as_str())),
        )?;

        #[cfg(test)]
        if let Some(hook) = self.before_write.lock().unwrap().as_mut() {
            hook(path);
        }

        // What was read, now and in earlier passes, must still be what the
        // file holds: an append is not a rewrite, anything else is.
        let after = file.metadata()?;
        ensure!(after.len() >= meta.len(), "transcript shrank while reading");
        if after.len() != meta.len() || after.modified()? != meta.modified()? {
            let earlier_intact = match &resumed {
                Some((start, _)) => unchanged(&mut file, &after, &start.cursor)?.is_some(),
                None => true,
            };
            ensure!(
                earlier_intact && chunk.still_on_disk(&mut file, offset)?,
                "transcript rewritten while reading"
            );
        }
        let next = Checkpoint {
            cursor,
            program,
            state,
            last_kind,
            last_at,
        };
        let written = self.storage.observe(&Batch {
            expected: start,
            next: &next,
            replaced,
            agent: self.format.agent,
            segments: &segments,
            prompts: &prompts,
        })?;
        if written && let Some(live) = &self.live {
            // The session as it is after this write. What says where it
            // is stays in memory: only its project is named.
            match self.live_session(stream, &next)? {
                Some(session) => live.observed_with_location(stream, session, location),
                None => live.forget(stream),
            }
        }
        Ok(written.then_some((Done::Written, read)))
    }

    fn live_session(
        &self,
        stream: &str,
        checkpoint: &Checkpoint,
    ) -> Result<Option<crate::live::Session>> {
        checkpoint
            .last_at
            .map(|last_at| {
                Ok(crate::live::Session {
                    agent: self.format.agent.to_string(),
                    program: checkpoint.program == Some(true),
                    project: checkpoint.state.project,
                    last_kind: checkpoint.last_kind,
                    last_at,
                    started_at: self.storage.session_start(stream)?.unwrap_or(last_at),
                })
            })
            .transpose()
    }
}
