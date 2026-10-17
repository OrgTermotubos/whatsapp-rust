//! Frozen legacy dashboard serialization versus the actual consumer fix.

use std::collections::VecDeque;
use whatsapp_rust::{serde, serde_json};

#[allow(unused_imports)] // The imported production module contains host-only tests.
#[path = "../consumer/src/supervisor/message_log.rs"]
mod message_log;
use message_log::MessageLogEntry;

#[global_allocator]
static ALLOC: divan::AllocProfiler = divan::AllocProfiler::system();

fn main() {
    divan::main();
}

// Exact Value-tree implementation from consumer 5e42634.
fn legacy_json(log: &VecDeque<MessageLogEntry>) -> String {
    let messages: Vec<serde_json::Value> = log
        .iter()
        .map(|m| {
            serde_json::json!({
                "id": m.id,
                "chat": m.chat,
                "sender": m.sender,
                "text": m.text,
                "timestamp": m.timestamp,
                "from_me": m.from_me,
            })
        })
        .collect();
    serde_json::json!({"count": messages.len(), "messages": messages}).to_string()
}

fn fixture(count: usize) -> VecDeque<MessageLogEntry> {
    let log = (0..count)
        .map(|index| MessageLogEntry {
            id: format!("SYNTHETIC-MESSAGE-{index}"),
            chat: "100000000001@s.whatsapp.net".into(),
            sender: "100000000002@s.whatsapp.net".into(),
            text: Some("x".repeat(256)),
            timestamp: 1_000_000,
            from_me: false,
        })
        .collect();
    let old: serde_json::Value = serde_json::from_str(&legacy_json(&log)).expect("legacy JSON");
    let new: serde_json::Value =
        serde_json::from_str(&message_log::to_json(&log)).expect("borrowed JSON");
    assert_eq!(old, new, "dashboard JSON contract changed");
    log
}

#[divan::bench(args = [1, 16], sample_count = 1000, sample_size = 1)]
fn legacy_value_tree(bencher: divan::Bencher, count: usize) {
    let log = fixture(count);
    bencher.bench_local(|| legacy_json(&log));
}

#[divan::bench(args = [1, 16], sample_count = 1000, sample_size = 1)]
fn borrowed_log(bencher: divan::Bencher, count: usize) {
    let log = fixture(count);
    bencher.bench_local(|| message_log::to_json(&log));
}
