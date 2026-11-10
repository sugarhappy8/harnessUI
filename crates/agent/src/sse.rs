//! A transport-agnostic, incremental server-sent-events decoder.
//!
//! Bytes go in as they arrive from any transport; complete events come out. The decoder
//! follows the WHATWG event-stream rules that matter for chat streams:
//!
//! - lines end in LF, CRLF or a lone CR, and may be split anywhere across chunks (including
//!   between the CR and LF of a CRLF, and inside a multi-byte UTF-8 character);
//! - lines starting with `:` are comments and are ignored (llama-server sends `:` as a
//!   keep-alive ping);
//! - several `data:` lines in one event are joined with `\n`;
//! - one optional space after the field colon is removed;
//! - a UTF-8 byte-order mark at the very start is skipped.
//!
//! `data: [DONE]` is delivered as an ordinary event; [`SseEvent::is_done`] recognises it.
//!
//! A server that never sends a newline cannot make the decoder grow without bound: a line or
//! an event longer than the configured limit is dropped and [`SseDecoder::overflowed`] turns
//! true, which the stream layer reports as a decode error.

/// One dispatched server-sent event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    /// The `event:` field, if the server sent one.
    pub event: Option<String>,
    /// The `data:` lines joined with `\n`.
    pub data: String,
    /// The last `id:` field seen on the stream, if any.
    pub id: Option<String>,
}

impl SseEvent {
    /// True for the OpenAI end-of-stream marker `data: [DONE]`.
    pub fn is_done(&self) -> bool {
        self.data.trim() == "[DONE]"
    }
}

const BOM: &[u8] = b"\xEF\xBB\xBF";

/// Incremental SSE decoder. See the module documentation for the rules it follows.
#[derive(Debug)]
pub struct SseDecoder {
    /// Bytes of the line being read.
    line: Vec<u8>,
    /// The previous chunk ended in CR, so a leading LF in the next chunk belongs to it.
    pending_cr: bool,
    /// The current line passed the limit and is being skipped until its end.
    discarding_line: bool,
    /// The current event passed the limit and is skipped until the blank line.
    discarding_event: bool,
    /// The first line has been seen (the BOM check is done).
    started: bool,
    data: String,
    event: Option<String>,
    last_id: Option<String>,
    max_bytes: usize,
    overflowed: bool,
}

impl Default for SseDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl SseDecoder {
    /// The default limit on one line and on one event's data: 16 MiB. Large enough for a
    /// tool call sent whole in one chunk, small enough to bound memory.
    pub const DEFAULT_MAX_BYTES: usize = 16 * 1024 * 1024;

    /// A decoder with the default size limit.
    pub fn new() -> Self {
        Self::with_max_bytes(Self::DEFAULT_MAX_BYTES)
    }

    /// A decoder that drops any line or event larger than `max_bytes`.
    pub fn with_max_bytes(max_bytes: usize) -> Self {
        Self {
            line: Vec::new(),
            pending_cr: false,
            discarding_line: false,
            discarding_event: false,
            started: false,
            data: String::new(),
            event: None,
            last_id: None,
            max_bytes: max_bytes.max(1),
            overflowed: false,
        }
    }

    /// True once a line or an event was dropped for exceeding the size limit.
    pub fn overflowed(&self) -> bool {
        self.overflowed
    }

    /// Feeds bytes and returns the events they complete, in order.
    pub fn push(&mut self, mut bytes: &[u8]) -> Vec<SseEvent> {
        let mut out = Vec::new();
        // An empty chunk (a transport may deliver one) must not consume the pending CR:
        // the LF that completes the CRLF can still arrive in the next non-empty chunk.
        if self.pending_cr && !bytes.is_empty() {
            self.pending_cr = false;
            if let Some(rest) = bytes.strip_prefix(b"\n") {
                bytes = rest;
            }
        }
        while !bytes.is_empty() {
            match bytes.iter().position(|&b| b == b'\n' || b == b'\r') {
                None => {
                    self.append_to_line(bytes);
                    break;
                }
                Some(end) => {
                    self.append_to_line(&bytes[..end]);
                    let terminator = bytes[end];
                    bytes = &bytes[end + 1..];
                    self.end_line(&mut out);
                    if terminator == b'\r' {
                        match bytes.first() {
                            Some(b'\n') => bytes = &bytes[1..],
                            Some(_) => {}
                            None => self.pending_cr = true,
                        }
                    }
                }
            }
        }
        out
    }

    /// Ends the stream. Unlike a strict SSE parser this also delivers an event whose final
    /// blank line never arrived, because a server that closes right after its last `data:`
    /// line (an error object, for example) still meant to send it.
    pub fn finish(&mut self) -> Vec<SseEvent> {
        let mut out = Vec::new();
        if !self.line.is_empty() || self.discarding_line {
            self.end_line(&mut out);
        }
        self.dispatch(&mut out);
        self.pending_cr = false;
        out
    }

    fn append_to_line(&mut self, bytes: &[u8]) {
        if self.discarding_line {
            return;
        }
        if self.line.len() + bytes.len() > self.max_bytes {
            self.line.clear();
            self.discarding_line = true;
            self.overflowed = true;
            return;
        }
        self.line.extend_from_slice(bytes);
    }

    fn end_line(&mut self, out: &mut Vec<SseEvent>) {
        let line = std::mem::take(&mut self.line);
        if std::mem::replace(&mut self.discarding_line, false) {
            // The oversized line may have been a data line: its event is incomplete.
            self.discarding_event = true;
            return;
        }
        let mut line: &[u8] = &line;
        if !self.started {
            self.started = true;
            if let Some(rest) = line.strip_prefix(BOM) {
                line = rest;
            }
        }
        if line.is_empty() {
            self.dispatch(out);
            return;
        }
        if line[0] == b':' {
            return;
        }
        let (field, value) = match line.iter().position(|&b| b == b':') {
            Some(colon) => {
                let value = &line[colon + 1..];
                (&line[..colon], value.strip_prefix(b" ").unwrap_or(value))
            }
            None => (line, &[][..]),
        };
        match field {
            b"data" => {
                if self.discarding_event {
                    return;
                }
                let value = String::from_utf8_lossy(value);
                if self.data.len() + value.len() + 1 > self.max_bytes {
                    self.data.clear();
                    self.discarding_event = true;
                    self.overflowed = true;
                    return;
                }
                self.data.push_str(&value);
                self.data.push('\n');
            }
            b"event" => self.event = Some(String::from_utf8_lossy(value).into_owned()),
            // An id containing NUL is ignored, as the SSE rules require.
            b"id" if !value.contains(&0) => {
                self.last_id = Some(String::from_utf8_lossy(value).into_owned());
            }
            // `retry` and unknown fields are ignored, as the SSE rules require.
            _ => {}
        }
    }

    fn dispatch(&mut self, out: &mut Vec<SseEvent>) {
        let event = self.event.take();
        if std::mem::replace(&mut self.discarding_event, false) {
            self.data.clear();
            return;
        }
        if self.data.is_empty() {
            return;
        }
        let mut data = std::mem::take(&mut self.data);
        if data.ends_with('\n') {
            data.pop();
        }
        out.push(SseEvent {
            event,
            data,
            id: self.last_id.clone(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(events: &[SseEvent]) -> Vec<&str> {
        events.iter().map(|e| e.data.as_str()).collect()
    }

    #[test]
    fn basic_events_and_done() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push(b"data: {\"a\":1}\n\ndata: [DONE]\n\n");
        assert_eq!(data(&events), vec!["{\"a\":1}", "[DONE]"]);
        assert!(!events[0].is_done());
        assert!(events[1].is_done());
    }

    #[test]
    fn comments_and_pings_are_ignored() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push(b":\n\n: keep-alive\ndata: x\n\n:\n\n");
        assert_eq!(data(&events), vec!["x"]);
    }

    #[test]
    fn crlf_and_lone_cr() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push(b"data: a\r\n\r\ndata: b\r\rdata: c\n\n");
        assert_eq!(data(&events), vec!["a", "b", "c"]);
    }

    #[test]
    fn crlf_split_across_chunks() {
        let mut decoder = SseDecoder::new();
        assert!(decoder.push(b"data: a\r").is_empty());
        assert!(decoder.push(b"\n\r").len() == 1);
        // The LF after the second CR belongs to it and must not produce a second blank line.
        assert!(decoder.push(b"\ndata: b\r\n\r\n").len() == 1);
    }

    #[test]
    fn empty_chunk_between_cr_and_lf_keeps_the_crlf_together() {
        // "data: a\r\n" + "data: b\r\n\r\n" is ONE event with two data lines. An empty chunk
        // delivered between the CR and the LF must not turn that LF into a blank line.
        let mut decoder = SseDecoder::new();
        let mut events = decoder.push(b"data: a\r");
        events.extend(decoder.push(b""));
        events.extend(decoder.push(b"\ndata: b\r\n\r\n"));
        assert_eq!(data(&events), vec!["a\nb"]);
    }

    #[test]
    fn empty_event_field_is_kept_as_empty() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push(b"event:\ndata: x\n\n");
        assert_eq!(events[0].event.as_deref(), Some(""));
    }

    #[test]
    fn multi_line_data_is_joined() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push(b"data: line one\ndata:line two\ndata\n\n");
        assert_eq!(data(&events), vec!["line one\nline two\n"]);
    }

    #[test]
    fn event_and_id_fields() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push(b"event: error\nid: 7\ndata: x\n\ndata: y\n\n");
        assert_eq!(events[0].event.as_deref(), Some("error"));
        assert_eq!(events[0].id.as_deref(), Some("7"));
        assert_eq!(events[1].event, None);
        assert_eq!(events[1].id.as_deref(), Some("7"));
    }

    #[test]
    fn utf8_split_inside_a_character() {
        let text = "data: héllo wörld ✓\n\n".as_bytes();
        let mut decoder = SseDecoder::new();
        let mut events = Vec::new();
        for byte in text {
            events.extend(decoder.push(std::slice::from_ref(byte)));
        }
        assert_eq!(data(&events), vec!["héllo wörld ✓"]);
    }

    #[test]
    fn bom_is_skipped_once() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push(b"\xEF\xBB\xBFdata: a\n\n");
        assert_eq!(data(&events), vec!["a"]);
    }

    #[test]
    fn finish_flushes_an_unterminated_event() {
        let mut decoder = SseDecoder::new();
        assert!(decoder.push(b"data: {\"error\":{}}").is_empty());
        assert_eq!(data(&decoder.finish()), vec!["{\"error\":{}}"]);
        assert!(decoder.finish().is_empty());
    }

    #[test]
    fn empty_data_field_dispatches_empty_event() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push(b"data:\n\n\n\n");
        assert_eq!(data(&events), vec![""]);
    }

    #[test]
    fn oversized_line_is_dropped_and_flagged() {
        let mut decoder = SseDecoder::with_max_bytes(16);
        let mut events = decoder.push(b"data: 0123456789");
        events.extend(decoder.push(b"0123456789\n\ndata: ok\n\n"));
        assert_eq!(data(&events), vec!["ok"]);
        assert!(decoder.overflowed());
    }

    #[test]
    fn oversized_event_is_dropped_and_flagged() {
        let mut decoder = SseDecoder::with_max_bytes(11);
        let events = decoder.push(b"data: 12345\ndata: 67890\n\ndata: ok\n\n");
        assert_eq!(data(&events), vec!["ok"]);
        assert!(decoder.overflowed());
    }
}
