//! Validated-record payload extraction only; excludes flash I/O and validation.
#[path = "../.ci-src/record_buffer.rs"]
mod record_buffer;

#[global_allocator]
static ALLOC: divan::AllocProfiler = divan::AllocProfiler::system();

const PREFIX: usize = 5 + 32;

fn main() {
    divan::main();
}

fn fixture(bytes: usize) -> Vec<u8> {
    let mut record = Vec::with_capacity(PREFIX + bytes);
    record.extend_from_slice(b"WA1");
    record.extend_from_slice(&32_u16.to_le_bytes());
    record.resize(PREFIX, 0x57);
    record.extend((0..bytes).map(|i| (i % 256) as u8));
    record
}

// Both cases include acquiring one owned copy of the same already-read fixture.
// The old extraction then allocates its second payload; the production helper
// retains the first allocation. Verification stays outside the measured region.
#[divan::bench(args = [128, 3432, 65536], sample_count = 1000, sample_size = 1)]
fn legacy_payload_copy(bencher: divan::Bencher, bytes: usize) {
    let record = fixture(bytes);
    assert_eq!(record.len(), PREFIX + bytes);
    bencher.bench_local(|| {
        let owned_record = divan::black_box(record.clone());
        let payload = owned_record[PREFIX..].to_vec();
        divan::black_box(payload);
    });
}

#[divan::bench(args = [128, 3432, 65536], sample_count = 1000, sample_size = 1)]
fn production_in_place(bencher: divan::Bencher, bytes: usize) {
    let record = fixture(bytes);
    assert_eq!(
        record_buffer::remove_prefix(record.clone(), PREFIX),
        record[PREFIX..]
    );
    bencher.bench_local(|| {
        let owned_record = divan::black_box(record.clone());
        divan::black_box(record_buffer::remove_prefix(owned_record, PREFIX));
    });
}
