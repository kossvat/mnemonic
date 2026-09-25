//! Byte-accurate JSONL tailing shared by the Claude and Codex watchers.
//! Only a newline commits a record: an incomplete JSON/UTF-8 suffix stays on
//! disk and is read again on the next poll (including after a restart).

#[cfg(test)]
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
#[cfg(test)]
use std::path::Path;

pub(super) struct TailChunk {
    bytes: Vec<u8>,
    pub next_offset: u64,
}

impl TailChunk {
    /// Whether the bytes this chunk was parsed from are still what the file
    /// holds at `start`. An append past the snapshot leaves them alone; a
    /// rewrite does not.
    pub(super) fn still_on_disk(
        &self,
        reader: &mut (impl Read + Seek),
        start: u64,
    ) -> io::Result<bool> {
        reader.seek(SeekFrom::Start(start))?;
        let mut current = Vec::with_capacity(self.bytes.len());
        reader
            .take(self.bytes.len() as u64)
            .read_to_end(&mut current)?;
        Ok(current == self.bytes)
    }

    pub(super) fn records(&self, start: u64) -> impl Iterator<Item = (u64, &str)> {
        let mut offset = start;
        self.bytes
            .split_inclusive(|b| *b == b'\n')
            .filter_map(move |line| {
                let position = offset;
                offset += line.len() as u64;
                std::str::from_utf8(line).ok().map(|text| (position, text))
            })
    }

    #[cfg(test)]
    pub fn lines(&self) -> impl Iterator<Item = &str> {
        // A malformed complete UTF-8 record is skipped, never replaced with
        // fabricated text. Partial UTF-8 cannot enter bytes: it is after the
        // last newline and was excluded before advancing the offset.
        self.bytes
            .split(|b| *b == b'\n')
            .filter_map(|line| std::str::from_utf8(line).ok())
    }
}

#[cfg(test)]
pub(super) fn read_complete_tail(path: &Path, offset: u64) -> io::Result<TailChunk> {
    let mut file = File::open(path)?;
    let snapshot_len = file.metadata()?.len();
    read_snapshot(&mut file, snapshot_len, offset)
}

/// Align old persisted offsets too: older watchers sometimes checkpointed in
/// the middle of an unfinished record. On first run, skip complete history
/// but retain any unfinished record at EOF so its later completion is read.
#[cfg(test)]
pub(super) fn resume_offset(path: &Path, saved: Option<u64>) -> io::Result<u64> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    last_complete_offset(&mut file, saved.unwrap_or(len).min(len))
}

pub(super) fn read_snapshot(
    reader: &mut (impl Read + Seek),
    len: u64,
    offset: u64,
) -> io::Result<TailChunk> {
    if offset > len {
        // Preserve the existing compaction policy: skip rewritten history,
        // while leaving an unfinished new tail available for a later poll.
        return Ok(TailChunk {
            bytes: Vec::new(),
            next_offset: last_complete_offset(reader, len)?,
        });
    }
    reader.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    // An append between metadata() and read() belongs to the NEXT snapshot.
    // Reading beyond len then checkpointing len used to replay a suffix.
    reader.take(len - offset).read_to_end(&mut bytes)?;
    let consumed = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    bytes.truncate(consumed);
    Ok(TailChunk {
        bytes,
        next_offset: offset + consumed as u64,
    })
}

pub(super) fn last_complete_offset(
    reader: &mut (impl Read + Seek),
    mut end: u64,
) -> io::Result<u64> {
    let mut buf = [0u8; 4096];
    while end > 0 {
        let size = end.min(buf.len() as u64) as usize;
        let start = end - size as u64;
        reader.seek(SeekFrom::Start(start))?;
        reader.read_exact(&mut buf[..size])?;
        if let Some(i) = buf[..size].iter().rposition(|b| *b == b'\n') {
            return Ok(start + i as u64 + 1);
        }
        end = start;
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    fn records(chunk: &TailChunk) -> Vec<serde_json::Value> {
        chunk
            .lines()
            .filter(|s| !s.is_empty())
            .map(|s| serde_json::from_str(s).unwrap())
            .collect()
    }

    #[test]
    fn split_json_retries_the_unfinished_record_exactly_once() {
        let first = b"{\"n\":1}\n";
        let second = b"{\"n\":2}\n";
        let mut data = first.to_vec();
        data.extend_from_slice(&second[..4]);
        let mut reader = Cursor::new(data.clone());
        let chunk = read_snapshot(&mut reader, data.len() as u64, 0).unwrap();
        assert_eq!(records(&chunk), vec![serde_json::json!({"n":1})]);
        assert_eq!(chunk.next_offset, first.len() as u64);
        data.extend_from_slice(&second[4..]);
        let len = data.len() as u64;
        let mut reader = Cursor::new(data);
        let next = read_snapshot(&mut reader, len, chunk.next_offset).unwrap();
        assert_eq!(records(&next), vec![serde_json::json!({"n":2})]);
        assert_eq!(next.next_offset, len);
        assert!(
            read_snapshot(&mut reader, len, next.next_offset)
                .unwrap()
                .bytes
                .is_empty()
        );
    }

    #[test]
    fn split_utf8_tail_waits_for_the_complete_record() {
        let text = "{\"text\":\"решение 🚀\"}\n";
        let split = text.find('🚀').unwrap() + 2; // middle of a four-byte codepoint
        let mut reader = Cursor::new(text.as_bytes());
        let first = read_snapshot(&mut reader, split as u64, 0).unwrap();
        assert!(first.bytes.is_empty());
        assert_eq!(first.next_offset, 0);
        let next = read_snapshot(&mut reader, text.len() as u64, first.next_offset).unwrap();
        assert_eq!(
            records(&next),
            vec![serde_json::json!({"text":"решение 🚀"})]
        );
        assert_eq!(next.next_offset, text.len() as u64);
    }

    #[test]
    fn append_after_stat_is_not_consumed_or_replayed_in_the_wrong_snapshot() {
        let first = "{\"n\":1}\n";
        let second = "{\"n\":2}\n";
        let third = "{\"n\":3}\n";
        let all = format!("{first}{second}{third}");
        let mut reader = Cursor::new(all.as_bytes());
        // metadata observed half of record 2; read sees records 2 and 3
        // already completed. The snapshot limit must still be respected.
        let observed = first.len() + 4;
        let early = read_snapshot(&mut reader, observed as u64, 0).unwrap();
        assert_eq!(records(&early), vec![serde_json::json!({"n":1})]);
        assert_eq!(early.next_offset, first.len() as u64);
        let later = read_snapshot(&mut reader, all.len() as u64, early.next_offset).unwrap();
        assert_eq!(
            records(&later),
            vec![serde_json::json!({"n":2}), serde_json::json!({"n":3})]
        );
        assert_eq!(later.next_offset, all.len() as u64);
    }

    #[test]
    fn restart_and_first_run_resume_at_complete_byte_boundaries() {
        let path = crate::test_support::temp_path("mnemonic-tail-", "transcript.jsonl");
        let first = "{\"n\":1}\n";
        let second = "{\"text\":\"привет\"}\n";
        let split = second.find('п').unwrap() + 1;
        std::fs::write(
            &path,
            [first.as_bytes(), &second.as_bytes()[..split]].concat(),
        )
        .unwrap();
        let chunk = read_complete_tail(&path, 0).unwrap();
        let saved = chunk.next_offset;
        assert_eq!(saved, first.len() as u64);
        assert_eq!(resume_offset(&path, None).unwrap(), saved);
        // Repair a checkpoint made by the older watcher inside UTF-8/JSON.
        assert_eq!(
            resume_offset(&path, Some((first.len() + split) as u64)).unwrap(),
            saved
        );
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(&second.as_bytes()[split..]).unwrap();
        drop(file);
        let resumed = resume_offset(&path, Some(saved)).unwrap();
        let next = read_complete_tail(&path, resumed).unwrap();
        assert_eq!(records(&next), vec![serde_json::json!({"text":"привет"})]);
        assert_eq!(next.next_offset, (first.len() + second.len()) as u64);
    }

    #[test]
    fn saved_zero_keeps_a_new_sessions_first_unfinished_record() {
        let path = crate::test_support::temp_path("mnemonic-tail-", "transcript.jsonl");
        let record = b"{\"n\":1}\n";
        std::fs::write(&path, &record[..4]).unwrap();
        let saved = read_complete_tail(&path, 0).unwrap().next_offset;
        assert_eq!(saved, 0);
        // Completion happens while the watcher is down. Some(0) must stay
        // distinguishable from None (first-ever run, skip complete history).
        std::fs::write(&path, record).unwrap();
        let resumed = resume_offset(&path, Some(saved)).unwrap();
        assert_eq!(resumed, 0);
        assert_eq!(
            records(&read_complete_tail(&path, resumed).unwrap()),
            vec![serde_json::json!({"n":1})]
        );
    }

    #[test]
    fn compaction_skips_complete_history_but_keeps_an_unfinished_tail() {
        let rewritten = b"{\"n\":1}\n{\"n\":";
        let mut reader = Cursor::new(rewritten);
        let chunk = read_snapshot(&mut reader, rewritten.len() as u64, 1000).unwrap();
        assert!(chunk.bytes.is_empty());
        assert_eq!(chunk.next_offset, 8);
    }

    #[test]
    fn malformed_complete_utf8_does_not_corrupt_or_block_the_next_record() {
        let mut reader = Cursor::new(b"\xff\n{\"n\":2}\n");
        let len = reader.get_ref().len() as u64;
        let chunk = read_snapshot(&mut reader, len, 0).unwrap();
        assert_eq!(records(&chunk), vec![serde_json::json!({"n":2})]);
        assert_eq!(chunk.next_offset, len);
    }
}
