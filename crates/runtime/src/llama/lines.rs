//! Reading a child's output line by line without trusting it.
//!
//! Lines are decoded lossily (llama.cpp echoes raw bytes from model files, for example in
//! "invalid magic characters"), trailing `\r\n` is removed, and a line longer than
//! [`MAX_LINE_BYTES`] is cut so a chatty or broken process cannot exhaust memory.
//!
//! [`LineReader::splitting_cr`] also ends a line at a lone `\r`, for the Studio engine,
//! which redraws its progress bar in place with `\r` and prints `\n` only after the last
//! step: read by newlines alone, a whole sampling run would be one cut-off line.

use std::collections::VecDeque;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

/// Longest line kept, in bytes; the rest of a longer line is discarded.
pub(crate) const MAX_LINE_BYTES: usize = 16 * 1024;

/// Reads lines from a child's pipe.
pub(crate) struct LineReader<R> {
    reader: BufReader<R>,
    buf: Vec<u8>,
    /// Whether a lone `\r` ends a line too ([`Self::splitting_cr`]).
    split_cr: bool,
    /// The last line ended at a `\r`, so a `\n` right after it is part of that end.
    after_cr: bool,
}

impl<R: AsyncRead + Unpin> LineReader<R> {
    pub(crate) fn new(inner: R) -> Self {
        Self {
            reader: BufReader::new(inner),
            buf: Vec::new(),
            split_cr: false,
            after_cr: false,
        }
    }

    /// A reader that ends a line at `\r` as well as at `\n` (a `\r\n` is one end), for a
    /// child that redraws one line with `\r`: the Studio engine's progress bar.
    pub(crate) fn splitting_cr(inner: R) -> Self {
        Self {
            split_cr: true,
            ..Self::new(inner)
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
            if std::mem::take(&mut self.after_cr) && available[0] == b'\n' {
                self.reader.consume(1);
                continue;
            }
            consumed_any = true;
            let split_cr = self.split_cr;
            let ends_line = |b: &u8| *b == b'\n' || (split_cr && *b == b'\r');
            let (chunk, done) = match available.iter().position(ends_line) {
                Some(end) => {
                    self.after_cr = available[end] == b'\r';
                    (&available[..end], Some(end + 1))
                }
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

    use std::pin::Pin;
    use std::task::{Context, Poll};

    use tokio::io::ReadBuf;

    async fn lines<R: AsyncRead + Unpin>(mut reader: LineReader<R>) -> Vec<String> {
        let mut out = Vec::new();
        while let Some(line) = reader.next_line().await {
            out.push(line);
        }
        out
    }

    async fn collect(input: &[u8]) -> Vec<String> {
        lines(LineReader::new(input)).await
    }

    /// Hands out one byte per read, so a `\r\n` is split across two reads.
    struct OneByteAtATime<'a>(&'a [u8]);

    impl AsyncRead for OneByteAtATime<'_> {
        fn poll_read(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            if let Some((&first, rest)) = self.0.split_first() {
                buf.put_slice(&[first]);
                self.0 = rest;
            }
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn a_carriage_return_ends_a_line() {
        assert_eq!(
            lines(LineReader::splitting_cr(&b"a\rb\r\nc\n"[..])).await,
            ["a", "b", "c"]
        );
        // A redraw after a newline leaves an empty line between them, as the engine prints it.
        assert_eq!(
            lines(LineReader::splitting_cr(&b"log\n\rbar 1\rbar 2\n"[..])).await,
            ["log", "", "bar 1", "bar 2"]
        );
    }

    #[tokio::test]
    async fn llamas_reader_still_splits_on_newlines_only() {
        assert_eq!(lines(LineReader::new(&b"a\rb\n"[..])).await, ["a\rb"]);
    }

    #[tokio::test]
    async fn a_crlf_split_across_reads_is_one_end() {
        assert_eq!(
            lines(LineReader::splitting_cr(OneByteAtATime(b"a\r\nb\rc\r\n"))).await,
            ["a", "b", "c"]
        );
    }

    #[tokio::test]
    async fn a_line_ended_by_a_carriage_return_is_still_cut() {
        let mut input = vec![b'x'; MAX_LINE_BYTES * 2];
        input.extend_from_slice(b"\rnext\r");
        let lines = lines(LineReader::splitting_cr(&input[..])).await;
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].len(), MAX_LINE_BYTES);
        assert_eq!(lines[1], "next");
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
