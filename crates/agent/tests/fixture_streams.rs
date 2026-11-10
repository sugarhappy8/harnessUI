//! Decoding real streams captured from the pinned llama-server (b12110) and checking that
//! the result does not depend on where the transport splits the bytes.

use agent::{
    ArgumentsStatus, FinishReason, SseDecoder, SseEvent, StreamDecoder, StreamEvent, Timings,
    ToolCallAccumulator, Usage,
};
use proptest::prelude::*;

const TOOL_CALL: &[u8] = include_bytes!("fixtures/stream_tool_call.sse");
const REASONING: &[u8] = include_bytes!("fixtures/stream_reasoning.sse");

fn decode_in_pieces(bytes: &[u8], cuts: &[usize]) -> Vec<StreamEvent> {
    let mut decoder = StreamDecoder::new();
    let mut out = Vec::new();
    let mut start = 0;
    let mut cuts: Vec<usize> = cuts.iter().map(|c| c % (bytes.len() + 1)).collect();
    cuts.sort_unstable();
    for cut in cuts {
        decoder.push(&bytes[start..cut], &mut out).unwrap();
        // A transport may deliver empty chunks; they must change nothing (not even between
        // the CR and LF of a CRLF).
        decoder.push(b"", &mut out).unwrap();
        start = cut;
    }
    decoder.push(&bytes[start..], &mut out).unwrap();
    decoder.finish(&mut out).unwrap();
    out
}

fn sse_in_pieces(bytes: &[u8], cuts: &[usize]) -> Vec<SseEvent> {
    let mut decoder = SseDecoder::new();
    let mut out = Vec::new();
    let mut start = 0;
    let mut cuts: Vec<usize> = cuts.iter().map(|c| c % (bytes.len() + 1)).collect();
    cuts.sort_unstable();
    for cut in cuts {
        out.extend(decoder.push(&bytes[start..cut]));
        out.extend(decoder.push(b""));
        start = cut;
    }
    out.extend(decoder.push(&bytes[start..]));
    out.extend(decoder.finish());
    out
}

fn to_crlf(bytes: &[u8]) -> Vec<u8> {
    let text = std::str::from_utf8(bytes).unwrap();
    text.replace('\n', "\r\n").into_bytes()
}

/// Inserts llama-server keep-alive pings (`:` and a blank line) between events.
fn with_pings(bytes: &[u8]) -> Vec<u8> {
    let text = std::str::from_utf8(bytes).unwrap();
    text.replace("\n\ndata:", "\n\n:\n\n: ping\n\ndata:")
        .into_bytes()
}

#[test]
fn tool_call_fixture_events() {
    let events = decode_in_pieces(TOOL_CALL, &[]);
    let expected_args = ["{", "\"", "city", "\":", " \"", "Paris", "\"}"];
    let mut expected = vec![StreamEvent::ToolCallStarted {
        index: 0,
        id: "YHuw9eQT5XNqbiybdpC1BdgvZFVAvnjT".into(),
        name: "get_weather".into(),
    }];
    expected.extend(
        expected_args
            .iter()
            .map(|fragment| StreamEvent::ToolCallArgs {
                index: 0,
                fragment: (*fragment).to_owned(),
            }),
    );
    expected.push(StreamEvent::Finished {
        finish_reason: FinishReason::ToolCalls,
    });
    expected.push(StreamEvent::Usage(Usage {
        prompt_tokens: 168,
        completion_tokens: 20,
        cached_tokens: Some(0),
    }));
    expected.push(StreamEvent::Timings(Timings {
        prompt_n: Some(168),
        prompt_ms: Some(584.753),
        prompt_per_second: Some(287.300_791_958_314),
        predicted_n: Some(20),
        predicted_ms: Some(510.218),
        predicted_per_second: Some(37.238_984_120_513_19),
        cache_n: Some(0),
    }));
    expected.push(StreamEvent::Done);
    assert_eq!(events, expected);

    let mut accumulator = ToolCallAccumulator::new();
    for event in &events {
        accumulator.push(event);
    }
    let calls = accumulator.finish();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].call.id, "YHuw9eQT5XNqbiybdpC1BdgvZFVAvnjT");
    assert_eq!(calls[0].call.kind, "function");
    assert_eq!(calls[0].call.function.name, "get_weather");
    assert_eq!(calls[0].call.function.arguments, r#"{"city": "Paris"}"#);
    assert_eq!(calls[0].status, ArgumentsStatus::Object);
    assert_eq!(
        calls[0].arguments_object().unwrap()["city"],
        serde_json::json!("Paris")
    );
}

#[test]
fn reasoning_fixture_events() {
    // The captured stream is a thinking model cut off by max_tokens while still reasoning:
    // reasoning deltas only, finish_reason "length", then usage and timings.
    let events = decode_in_pieces(REASONING, &[]);
    let reasoning: String = events
        .iter()
        .filter_map(|event| match event {
            StreamEvent::Reasoning(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(reasoning.starts_with("Okay, the user wants to say \"say hi"));
    assert!(reasoning.ends_with("\"Hello, how are you"));
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, StreamEvent::Reasoning(_)))
            .count(),
        118
    );
    assert!(!events.iter().any(|e| matches!(e, StreamEvent::Content(_))));
    let tail = &events[events.len() - 4..];
    assert_eq!(
        tail[0],
        StreamEvent::Finished {
            finish_reason: FinishReason::Length
        }
    );
    assert_eq!(
        tail[1],
        StreamEvent::Usage(Usage {
            prompt_tokens: 14,
            completion_tokens: 120,
            cached_tokens: Some(1)
        })
    );
    let StreamEvent::Timings(timings) = tail[2] else {
        panic!("expected timings, got {:?}", tail[2]);
    };
    assert_eq!(timings.context_used(), Some(134));
    assert_eq!(tail[3], StreamEvent::Done);
}

#[test]
fn crlf_and_pings_do_not_change_events() {
    for fixture in [TOOL_CALL, REASONING] {
        let plain = decode_in_pieces(fixture, &[]);
        assert_eq!(decode_in_pieces(&to_crlf(fixture), &[]), plain);
        assert_eq!(decode_in_pieces(&with_pings(fixture), &[]), plain);
    }
}

#[test]
fn byte_at_a_time() {
    let cuts: Vec<usize> = (0..TOOL_CALL.len()).collect();
    assert_eq!(
        decode_in_pieces(TOOL_CALL, &cuts),
        decode_in_pieces(TOOL_CALL, &[])
    );
}

#[test]
fn input_after_done_is_ignored() {
    let mut bytes = TOOL_CALL.to_vec();
    bytes.extend_from_slice(b"data: {\"choices\":[{\"delta\":{\"content\":\"late\"}}]}\n\n");
    assert_eq!(
        decode_in_pieces(&bytes, &[]),
        decode_in_pieces(TOOL_CALL, &[])
    );
}

#[test]
fn input_tokens_fixture_shape() {
    let value: serde_json::Value =
        serde_json::from_slice(include_bytes!("fixtures/input_tokens.json")).unwrap();
    assert_eq!(value["input_tokens"], 168);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn tool_call_split_anywhere(cuts in proptest::collection::vec(any::<usize>(), 0..40)) {
        prop_assert_eq!(decode_in_pieces(TOOL_CALL, &cuts), decode_in_pieces(TOOL_CALL, &[]));
    }

    #[test]
    fn reasoning_split_anywhere(cuts in proptest::collection::vec(any::<usize>(), 0..80)) {
        prop_assert_eq!(decode_in_pieces(REASONING, &cuts), decode_in_pieces(REASONING, &[]));
    }

    #[test]
    fn crlf_split_anywhere(cuts in proptest::collection::vec(any::<usize>(), 0..40)) {
        let crlf = to_crlf(TOOL_CALL);
        prop_assert_eq!(sse_in_pieces(&crlf, &cuts), sse_in_pieces(TOOL_CALL, &[]));
        prop_assert_eq!(decode_in_pieces(&crlf, &cuts), decode_in_pieces(TOOL_CALL, &[]));
    }

    #[test]
    fn multi_line_crlf_split_anywhere(cuts in proptest::collection::vec(any::<usize>(), 0..40)) {
        // Multi-line `data:` fields with CRLF endings, where a lost CR/LF pairing would split
        // one event into two. The JSON is split across lines, which stays valid JSON once the
        // lines are joined with `\n`.
        let stream = concat!(
            "data: {\"choices\":[{\"index\":0,\r\n",
            "data: \"delta\":{\"content\":\"Hel\"}}]}\r\n\r\n",
            ":\r\n\r\n",
            "data: {\"choices\":[{\"index\":0,\r\n",
            "data: \"delta\":{\"content\":\"lo\"}}]}\r\n\r\n",
            "data: [DONE]\r\n\r\n",
        )
        .as_bytes();
        let whole = sse_in_pieces(stream, &[]);
        prop_assert_eq!(whole.len(), 3);
        prop_assert_eq!(&sse_in_pieces(stream, &cuts), &whole);
        prop_assert_eq!(
            decode_in_pieces(stream, &cuts),
            vec![
                StreamEvent::Content("Hel".into()),
                StreamEvent::Content("lo".into()),
                StreamEvent::Done
            ]
        );
    }

    #[test]
    fn pings_split_anywhere(cuts in proptest::collection::vec(any::<usize>(), 0..40)) {
        let pinged = with_pings(REASONING);
        prop_assert_eq!(sse_in_pieces(&pinged, &cuts), sse_in_pieces(REASONING, &[]));
    }

    #[test]
    fn arbitrary_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..512),
                                   cuts in proptest::collection::vec(any::<usize>(), 0..8)) {
        let mut decoder = StreamDecoder::new();
        let mut out = Vec::new();
        let mut start = 0;
        let mut cuts: Vec<usize> = cuts.iter().map(|c| c % (bytes.len() + 1)).collect();
        cuts.sort_unstable();
        for cut in cuts {
            if decoder.push(&bytes[start..cut], &mut out).is_err() {
                return Ok(());
            }
            start = cut;
        }
        let _ = decoder.push(&bytes[start..], &mut out);
        let _ = decoder.finish(&mut out);
    }
}
