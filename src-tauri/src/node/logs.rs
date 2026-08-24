use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde::Serialize;

/// Enough output to explain a failed start without growing without bound.
const CAPACITY: usize = 2000;

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Stream {
    Stdout,
    Stderr,
    Desktop,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub seq: u64,
    pub stream: Stream,
    pub text: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogPage {
    pub lines: Vec<LogLine>,
    pub cursor: u64,
    /// True when the requested cursor had already been dropped from the ring.
    pub truncated: bool,
}

/// Ring buffer of the node's output, polled by the status view.
#[derive(Clone, Default)]
pub struct LogBuffer {
    inner: Arc<Mutex<VecDeque<LogLine>>>,
    next: Arc<Mutex<u64>>,
}

impl LogBuffer {
    pub fn push(&self, stream: Stream, text: String) {
        let mut seq = self.next.lock().expect("log sequence poisoned");
        let line = LogLine {
            seq: *seq,
            stream,
            text,
        };
        *seq += 1;
        drop(seq);

        let mut lines = self.inner.lock().expect("log buffer poisoned");
        if lines.len() == CAPACITY {
            lines.pop_front();
        }
        lines.push_back(line);
    }

    /// Returns every line after `cursor`; `0` starts from the oldest retained line.
    pub fn page(&self, cursor: u64) -> LogPage {
        let lines = self.inner.lock().expect("log buffer poisoned");
        let oldest = lines.front().map(|line| line.seq).unwrap_or(cursor);
        let selected: Vec<LogLine> = lines
            .iter()
            .filter(|line| line.seq >= cursor)
            .cloned()
            .collect();
        let next = selected
            .last()
            .map(|line| line.seq + 1)
            .unwrap_or_else(|| lines.back().map(|line| line.seq + 1).unwrap_or(cursor));
        LogPage {
            lines: selected,
            cursor: next,
            truncated: oldest > cursor,
        }
    }

    pub fn clear(&self) {
        self.inner.lock().expect("log buffer poisoned").clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_after_cursor() {
        let buffer = LogBuffer::default();
        buffer.push(Stream::Stdout, "one".into());
        buffer.push(Stream::Stderr, "two".into());

        let first = buffer.page(0);
        assert_eq!(first.lines.len(), 2);
        assert!(!first.truncated);

        let second = buffer.page(first.cursor);
        assert!(second.lines.is_empty());
        assert_eq!(second.cursor, first.cursor);
    }

    #[test]
    fn drops_oldest_lines() {
        // The cursor keeps counting past dropped lines so gaps stay visible.
        let buffer = LogBuffer::default();
        for index in 0..CAPACITY + 5 {
            buffer.push(Stream::Stdout, index.to_string());
        }
        let page = buffer.page(0);
        assert_eq!(page.lines.len(), CAPACITY);
        assert_eq!(page.lines[0].seq, 5);
        assert!(page.truncated);
    }

    #[test]
    fn clears_on_restart() {
        let buffer = LogBuffer::default();
        buffer.push(Stream::Stdout, "one".into());
        buffer.clear();
        assert!(buffer.page(0).lines.is_empty());
    }
}
