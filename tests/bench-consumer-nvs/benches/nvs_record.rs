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

// Steady-state writes: the SAME 8 KiB input allocation is prepared outside both
// measurements. It remains resident; zero temporary allocation is not zero RAM.
// These controls model buffer work only, excluding NVS I/O and key validation.
fn reserved_payload(bytes: usize) -> Vec<u8> {
    let mut buffer = Vec::with_capacity(8192);
    buffer.extend((0..bytes).map(|i| (i % 256) as u8));
    buffer
}

fn owned_write(buffer: &mut Vec<u8>, previous: &[u8]) {
    record_buffer::with_suffix(buffer, previous.len(), |scratch| {
        scratch.copy_from_slice(divan::black_box(previous));
        divan::black_box(scratch);
    })
    .expect("reserved read scratch");
    let prefix = record_buffer::prepend_parts(buffer, &previous[..5], &previous[5..PREFIX])
        .expect("reserved framing scratch");
    divan::black_box(buffer.as_slice());
    record_buffer::remove_prefix_in_place(buffer, prefix);
}

#[divan::bench(args = [128, 3432, 4433], sample_count = 1000, sample_size = 1)]
fn legacy_write_temporaries(bencher: divan::Bencher, bytes: usize) {
    let previous = fixture(2969 - PREFIX);
    let expected = fixture(bytes);
    let payload = reserved_payload(bytes);
    let mut framed = Vec::new();
    framed.extend_from_slice(&previous[..PREFIX]);
    framed.extend_from_slice(&payload);
    assert_eq!(framed, expected);
    bencher
        .with_inputs(|| reserved_payload(bytes))
        .bench_local_refs(|payload| {
            let mut read = Vec::new();
            read.try_reserve_exact(previous.len()).expect("legacy read");
            read.resize(previous.len(), 0);
            read.copy_from_slice(divan::black_box(previous.as_slice()));
            divan::black_box(read.as_slice());
            drop(read); // Production validation releases the old record first.
            let mut framed = Vec::new();
            framed
                .try_reserve_exact(PREFIX + payload.len())
                .expect("legacy frame");
            framed.extend_from_slice(&previous[..PREFIX]);
            framed.extend_from_slice(payload);
            divan::black_box(framed);
        });
}

#[divan::bench(args = [128, 3432, 4433], sample_count = 1000, sample_size = 1)]
fn owned_write_reserved_scratch(bencher: divan::Bencher, bytes: usize) {
    let previous = fixture(2969 - PREFIX);
    let mut buffer = reserved_payload(bytes);
    let expected = buffer.clone();
    let pointer = buffer.as_ptr();
    owned_write(&mut buffer, &previous);
    assert_eq!(buffer, expected);
    assert_eq!(buffer.as_ptr(), pointer);
    assert_eq!(buffer.capacity(), 8192);
    bencher
        .with_inputs(|| reserved_payload(bytes))
        .bench_local_refs(|buffer| {
            owned_write(buffer, &previous);
        });
}
