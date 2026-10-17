//! Same-binary wrapper comparison over the real warm, encrypted DM send path.
//! The sink omits network/TLS; the backend is in memory. Setup and content
//! equivalence checks happen before measurement. Both futures are boxed like
//! callback tasks, then polled through completion and dropped.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use whatsapp_rust::bench_support::{ContextReactionHarness, ContextReplyHarness};

#[global_allocator]
static ALLOC: divan::AllocProfiler = divan::AllocProfiler::system();

fn main() {
    divan::main();
}

fn shared(label: &'static str, devices: usize) -> &'static ContextReplyHarness {
    type Key = (&'static str, usize);
    static FIXTURES: OnceLock<Mutex<HashMap<Key, &'static ContextReplyHarness>>> = OnceLock::new();
    let mut fixtures = FIXTURES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("fixture registry");
    fixtures
        .entry((label, devices))
        .or_insert_with(|| Box::leak(Box::new(ContextReplyHarness::new(devices))))
}

#[divan::bench(args = [1, 5], sample_count = 20, sample_size = 20)]
fn legacy_wrapper(bencher: divan::Bencher, devices: usize) {
    let fixture = shared("legacy", devices);
    bencher.bench(|| fixture.legacy_quoted_reply());
}

#[divan::bench(args = [1, 5], sample_count = 20, sample_size = 20)]
fn shared_request_wrapper(bencher: divan::Bencher, devices: usize) {
    let fixture = shared("shared-request", devices);
    bencher.bench(|| fixture.quoted_reply());
}

fn reaction_fixture(label: &'static str, group: bool) -> &'static ContextReactionHarness {
    type Key = (&'static str, bool);
    static FIXTURES: OnceLock<Mutex<HashMap<Key, &'static ContextReactionHarness>>> =
        OnceLock::new();
    let mut fixtures = FIXTURES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("reaction fixture registry");
    fixtures
        .entry((label, group))
        .or_insert_with(|| Box::leak(Box::new(ContextReactionHarness::new(group))))
}

#[divan::bench(args = [false, true], sample_count = 20, sample_size = 20)]
fn reaction_unboxed_control(bencher: divan::Bencher, group: bool) {
    let fixture = reaction_fixture("unboxed", group);
    bencher.bench(|| fixture.unboxed());
}

#[divan::bench(args = [false, true], sample_count = 20, sample_size = 20)]
fn reaction_bounded_routing(bencher: divan::Bencher, group: bool) {
    let fixture = reaction_fixture("bounded", group);
    bencher.bench(|| fixture.bounded());
}
