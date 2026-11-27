//! A session carried to another account of a pool has the turns a limit ended cleaned, and what is
//! left is a history the format accepts, so the next account can import it. The cleaner is
//! `coqui-acp-pool`'s; this holds the pinned revision to that for a session with an earlier limit
//! turn, the case that once left a prompt inside an open turn.

use ai6::DEFAULT_LIMIT_PHRASES;
use coqui_acp_history::{Body, Document, Line, Stop, StopReason};
use coqui_acp_pool::{Cleaning, clean};
use serde_json::json;

const HEADER: &str = r#"{"format":"coqui-acp-history","version":1,"id":"5f0f6d6e-3f3c-4f1a-8a52-1c1e6f0b9a77","origin":{"harness":"claude"}}"#;

fn ask(text: &str) -> Line {
    Line::prompt(vec![json!({"type": "text", "text": text})])
}

fn say(text: &str) -> Line {
    Line::update(json!({
        "sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}
    }))
}

/// A turn a limit ended: some work, a call left open, the limit's words and a failed ending.
fn limit_turn(question: &str, said: &str) -> Vec<Line> {
    vec![
        ask(question),
        say("Working on it."),
        Line::update(
            json!({"sessionUpdate": "tool_call", "toolCallId": question, "status": "in_progress", "title": "run"}),
        ),
        say(said),
        Line::turn_end(Stop::Error),
    ]
}

fn history(lines: Vec<Line>) -> Document {
    let mut document: Document = format!("{HEADER}\n")
        .parse()
        .expect("a header is a history");
    document.lines = lines;
    document
}

fn prompts(document: &Document) -> usize {
    document
        .lines
        .iter()
        .filter(|line| matches!(line.body, Body::Prompt { .. }))
        .count()
}

#[test]
fn a_session_with_an_earlier_limit_turn_moves_and_its_history_imports() {
    let mut lines = limit_turn("first", "You have hit your usage limit. Please wait");
    lines.extend([
        ask("second"),
        say("done"),
        Line::turn_end(Stop::Acp(StopReason::EndTurn)),
    ]);
    lines.extend(limit_turn("third", "usage limit reached, try again later"));
    let before = history(lines);
    let cleaned = clean(&before, &Cleaning::new(DEFAULT_LIMIT_PHRASES));
    // What the next account imports: every prompt the person sent is still there, none sits inside
    // an open turn, and no limit words or open calls are carried along.
    cleaned
        .document
        .to_vec()
        .expect("the cleaned history is one the format accepts");
    cleaned.document.check().expect("it is still a history");
    assert_eq!(prompts(&cleaned.document), prompts(&before));
    assert!(cleaned.removed.limit_text >= 2);
    assert_eq!(cleaned.removed.open_calls, 2);
    let said: Vec<String> = cleaned
        .document
        .lines
        .iter()
        .filter_map(|line| match &line.body {
            Body::Update { update } if update["sessionUpdate"] == "agent_message_chunk" => {
                update["content"]["text"].as_str().map(str::to_owned)
            }
            _ => None,
        })
        .collect();
    assert!(said.iter().all(|text| !text.contains("limit")), "{said:?}");
    assert!(said.contains(&"done".to_owned()), "{said:?}");
}
