//! Bounded ring buffer of log lines.

use std::collections::VecDeque;

/// The most recent lines of a command's output. Oldest lines are dropped
/// once the capacity is reached so memory stays flat for chatty processes.
#[derive(Debug, Clone)]
pub struct LogBuffer {
    lines: VecDeque<String>,
    capacity: usize,
    dropped: u64,
}

impl LogBuffer {
    /// Creates an empty buffer holding at most `capacity` lines (min 1).
    pub fn new(capacity: usize) -> Self {
        Self {
            lines: VecDeque::new(),
            capacity: capacity.max(1),
            dropped: 0,
        }
    }

    /// Appends a line, evicting the oldest if full.
    pub fn push(&mut self, line: String) {
        if self.lines.len() == self.capacity {
            self.lines.pop_front();
            self.dropped += 1;
        }
        self.lines.push_back(line);
    }

    /// Number of lines currently held.
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// True when no lines are held.
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// The line at `index` (0 is the oldest held line).
    pub fn get(&self, index: usize) -> Option<&str> {
        self.lines.get(index).map(String::as_str)
    }

    /// How many older lines were evicted so far (so the UI can say the log
    /// is truncated).
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Removes all held lines.
    pub fn clear(&mut self) {
        self.lines.clear();
    }

    /// All held lines joined with newlines, for "copy log".
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for (i, line) in self.lines.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(line);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_oldest_beyond_capacity() {
        let mut log = LogBuffer::new(3);
        for i in 0..5 {
            log.push(format!("l{i}"));
        }
        assert_eq!(log.len(), 3);
        assert_eq!(log.get(0), Some("l2"));
        assert_eq!(log.dropped(), 2);
        assert_eq!(log.to_text(), "l2\nl3\nl4");
        log.clear();
        assert!(log.is_empty());
    }
}
