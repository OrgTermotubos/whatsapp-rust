//! Retained app-state blobs through production admission, streaming and verification.

use whatsapp_rust::bench_support::PrefetchHarness;

#[global_allocator]
static ALLOC: divan::AllocProfiler = divan::AllocProfiler::system();

fn main() {
    divan::main();
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
