//! Reading a child's output line by line without trusting it.
//!
//! Lines are decoded lossily (llama.cpp echoes raw bytes from model files, for example in
//! "invalid magic characters"), trailing `\r\n` is removed, and a line longer than
//! [`MAX_LINE_BYTES`] is cut so a chatty or broken process cannot exhaust memory.

use std::collections::VecDeque;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

/// Longest line kept, in bytes; the rest of a longer line is discarded.
pub(crate) const MAX_LINE_BYTES: usize = 16 * 1024;

/// Reads lines from a child's pipe.
pub(crate) struct LineReader<R> {
    reader: BufReader<R>,
    buf: Vec<u8>,
}

impl<R: AsyncRead + Unpin> LineReader<R> {
    pub(crate) fn new(inner: R) -> Self {
        Self {
            reader: BufReader::new(inner),
            buf: Vec::new(),
        }
    }

    /// The next line, or `None` at end-of-file or on a read error.
    pub(crate) async fn next_line(&mut self) -> Option<String> {
        self.buf.clear();
        let mut consumed_any = false;
        loop {
            let available = match self.reader.fill_buf().await {
                Ok(bytes) => bytes,
                Err(_) => return None,
            };
            if available.is_empty() {
                return consumed_any.then(|| finish(&self.buf));
            }
            consumed_any = true;
            let (chunk, done) = match available.iter().position(|&b| b == b'\n') {
                Some(end) => (&available[..end], Some(end + 1)),
                None => (available, None),
            };
            let room = MAX_LINE_BYTES.saturating_sub(self.buf.len());
            self.buf.extend_from_slice(&chunk[..chunk.len().min(room)]);
            match done {
                Some(used) => {
                    self.reader.consume(used);
                    return Some(finish(&self.buf));
                }
                None => {
                    let used = chunk.len();
                    self.reader.consume(used);
                }
            }
        }
    }
}

fn finish(bytes: &[u8]) -> String {
    let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
    String::from_utf8_lossy(bytes).into_owned()
}

/// The last `capacity` lines seen.
#[derive(Debug, Clone)]
pub(crate) struct Tail {
    lines: VecDeque<String>,
    capacity: usize,
}

impl Tail {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            lines: VecDeque::with_capacity(capacity.min(1024)),
            capacity: capacity.max(1),
        }
    }

    pub(crate) fn push(&mut self, line: String) {
        if self.lines.len() == self.capacity {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }

    pub(crate) fn to_vec(&self) -> Vec<String> {
        self.lines.iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn collect(input: &[u8]) -> Vec<String> {
        let mut reader = LineReader::new(input);
        let mut out = Vec::new();
        while let Some(line) = reader.next_line().await {
            out.push(line);
        }
        out
    }

    #[tokio::test]
    async fn splits_lines_and_strips_crlf() {
        assert_eq!(collect(b"a\r\nb\nc").await, vec!["a", "b", "c"]);
        assert_eq!(collect(b"").await, Vec::<String>::new());
        assert_eq!(collect(b"\n\n").await, vec!["", ""]);
    }

    #[tokio::test]
    async fn decodes_invalid_utf8_lossily() {
        let lines = collect(b"magic '\xff\xfe9\xff'\nok\n").await;
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("magic '"));
        assert_eq!(lines[1], "ok");
    }

    #[tokio::test]
    async fn truncates_long_lines() {
        let mut input = vec![b'x'; MAX_LINE_BYTES * 3];
        input.extend_from_slice(b"\nnext\n");
        let lines = collect(&input).await;
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].len(), MAX_LINE_BYTES);
        assert_eq!(lines[1], "next");
    }

    #[test]
    fn tail_keeps_the_last_lines() {
        let mut tail = Tail::new(3);
        for i in 0..5 {
            tail.push(i.to_string());
        }
        assert_eq!(tail.to_vec(), vec!["2", "3", "4"]);
        let mut tiny = Tail::new(0);
        tiny.push("a".into());
        tiny.push("b".into());
        assert_eq!(tiny.to_vec(), vec!["b"]);
    }
}
