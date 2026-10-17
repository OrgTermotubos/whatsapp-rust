//! Retained app-state blobs through production admission, streaming and verification.

use std::sync::Arc;
use whatsapp_rust::bench_support::{PrefetchHarness, StreamingDecryptHarness};

#[global_allocator]
static ALLOC: divan::AllocProfiler = divan::AllocProfiler::system();

fn main() {
    divan::main();
}

// These synchronous controls expose crypto/output costs without the runtime
// internals that prevent useful callgraphs for the end-to-end cases.
#[divan::bench(args = [65536, 1048576], sample_count = 30, sample_size = 1)]
fn streaming_vec(bencher: divan::Bencher, bytes: usize) {
    let fixture = StreamingDecryptHarness::new(bytes).expect("streaming fixture");
    bencher.bench_local(|| fixture.decrypt_vec().expect("verified streaming decrypt"));
}

#[divan::bench(args = [65536, 1048576], sample_count = 30, sample_size = 1)]
fn streaming_accounted(bencher: divan::Bencher, bytes: usize) {
    let fixture = StreamingDecryptHarness::new(bytes).expect("streaming fixture");
    bencher.bench_local(|| {
        fixture
            .decrypt_accounted()
            .expect("verified accounted decrypt")
    });
}

#[divan::bench(args = [65536, 1048576], sample_count = 10, sample_size = 1)]
fn four_external_blobs(bencher: divan::Bencher, bytes: usize) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("prefetch runtime");
    bencher
        .counter(divan::counter::BytesCount::new((bytes * 4) as u64))
        .with_inputs(|| {
            runtime
                .block_on(PrefetchHarness::new(bytes))
                .expect("fixture")
        })
        .bench_local_refs(|fixture| {
            runtime
                .block_on(fixture.prefetch())
                .expect("verified prefetch")
        });
}

#[divan::bench(args = [65536, 1048576], sample_count = 10, sample_size = 1)]
fn eight_clients(bencher: divan::Bencher, bytes: usize) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("concurrent prefetch runtime");
    bencher
        .counter(divan::counter::BytesCount::new((bytes * 32) as u64))
        .with_inputs(|| {
            runtime.block_on(async {
                let mut fixtures = Vec::with_capacity(8);
                for _ in 0..8 {
                    fixtures.push(Arc::new(
                        PrefetchHarness::new(bytes).await.expect("fixture"),
                    ));
                }
                fixtures
            })
        })
        .bench_local_refs(|fixtures| {
            runtime.block_on(async {
                let tasks = fixtures.iter().map(|fixture| {
                    let fixture = Arc::clone(fixture);
                    tokio::spawn(async move { fixture.prefetch().await })
                });
                for result in futures::future::join_all(tasks).await {
                    result.expect("prefetch task").expect("verified prefetch");
                }
            })
        });
}
