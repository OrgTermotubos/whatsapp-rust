# Runtime regression probes

`runtime_latency` and `runtime_footprint` share one workload implementation in
`examples/runtime_probe/`. Both execute production APIs and fail on download,
decryption, delivery, patch-MAC or persistence errors. They require no network
access. The connected fixture's service limitations and certificate policy are
documented in `connected_idle.md`.

`appstate_prefetch` registers the same prefetch fixture with Divan and the
existing CodSpeed client shard, with one-client and eight-client rows. The latter
uses four runtime workers and four blobs per client. It reports verified bytes per batch. Its native
allocation profiler and CodSpeed instruments provide ongoing allocation/CPU
coverage. Use the separate diagnostic executables for uninstrumented walltime
and requested-heap comparisons.
Native Divan allocation columns exclude executor and blocking threads outside
Divan's control. Use the global DHAT probe for concurrent heap measurements.
CodSpeed uses its own instruments; compare matching instruments and environments.

Build each revision with the same toolchain, lockfile, profile and features.
When comparing older revisions, apply exactly the same additive probe sources
and Cargo example registrations to both trees. Preserve executables before
rebuilding, record their hashes and run them directly, outside Cargo.

```sh
cargo build --locked -p whatsapp-rust --profile bench \
  --features bench-harness,sqlite-storage-bundled \
  --example runtime_latency --example runtime_footprint --example connected_idle
target/release/examples/runtime_latency dm-pn-5 0 1 400
target/release/examples/runtime_latency patch-100 4 8 100
target/release/examples/runtime_footprint patch-100 4 8 5
```

Arguments are scenario, runtime worker count, simultaneous clients and samples.
Zero workers selects Tokio's current-thread runtime. Nonzero workers select a
multi-thread runtime. Warm send/receive fixtures own their current-thread runtime
and accept only zero workers and one client. Their sample limit prevents crossing
the group sender-key rotation threshold.

| Scenario | Timed work | Operation denominator |
| --- | --- | --- |
| `dm-lid-1` | Warm direct send to one LID device | Sends |
| `dm-pn-5` | Warm PN send with LID mappings and five peer devices | Sends |
| `group-64` | Warm send to a 64-member PN group | Sends |
| `receive-dm` | Decrypt, dispatch and receipt submission | Delivered messages |
| `patch-1`, `patch-100` | Apply one authenticated patch per client, commit its cursor and return decoded mutations | Mutations |
| `download-64k`, `download-1m` | Streaming encrypted media download, worker queue, decryption and verification | Downloads |
| `prefetch-64k`, `prefetch-1m` | Production app-state prefetch, four retained blobs per client, default shared budget and concurrency | Blobs |
| `connected-memory`, `connected-sqlite` | Connected activity, encrypted inbound delivery, history parsing, pending-row cleanup and receipt fence | Inbound messages, 256 per client |

Patch construction, media encryption, fixture setup and ciphertext generation are
outside measurement. Patches use synthetic push-name records and an in-memory
backend. Each application checks the persisted version and hash, and leaves MAC
validation enabled. Connected SQLite uses real file-backed databases and checks
shutdown/cleanup failures. Each connected sample also processes 256 historical
messages per client, so its throughput denominator does not describe a pure
receive workload. Connected and patch clients run as separate Tokio tasks.

The media probe uses `MediaDownloader`, not the client's shared app-state quota
or CDN discovery. HTTP returns an immutable, pre-encrypted in-memory body. These
measurements exclude TCP/TLS, production servers, real credentials and first-contact
Signal session establishment. The prefetch fixture additionally exercises the
client's production admission, quota accounting, job scheduling and result
retention. It seeds a valid cached CDN route, verifies every plaintext body, and
drops each batch after verification. The blobs are synthetic bytes, so protobuf
application is measured separately in the patch scenarios. The bench-only payload
adapter accepts both the old byte map and the new fallible byte map, preserving
download errors while letting identical sources compile against both revisions.
Worker and client arguments describe the executor topology. Record the actual
desktop, virtual machine or server hardware separately.
For standalone media, the client argument counts simultaneous downloads sharing
the immutable HTTP input. Send fixtures check success through the encrypted sink;
remote delivery is outside their scope.

## Timing and memory boundaries

Each sample is one JSON line. Timing uses System and reports `elapsed_ns`.
The footprint executable installs DHAT and reports allocation count, cumulative
requested bytes, peak tracked live bytes and tracked bytes still live at the end
of the measured region. Timing is null under DHAT. No walltime measured with DHAT
is a performance result.

DHAT starts after fixture/input creation and excludes pre-existing allocations.
Its retained count covers allocations born in that region, not the client's
entire resident heap. The complementary `connected_idle` example measures whole
Rust live bytes and Linux RssAnon across real elapsed idle checkpoints. C heap,
allocator slack and thread stacks are absent from the DHAT byte counts.

Downloads are sampled before dropping results. Patch outputs are checked after
sampling. Warm receive checks the delivered-event increment. Connected work
waits for its fixture's receipt/commit fence. Deferred housekeeping can overlap
the region and should remain part of the reported variability.

Use fresh processes, warm up, alternate revision order within each pair and keep
raw samples. Calculate throughput from total operations divided by summed timed
durations. Calculate latency quantiles on individual samples. For concurrent
patch/download/prefetch/connected cases, those quantiles are batch completion latency,
not per-message latency. Use process-level repeats as statistical replicates,
not thousands of correlated messages as independent experiments. Report tail
quantiles with their sample counts and paired variation. A noisy result or an
interval spanning no change is inconclusive.

Keep builds and other benchmark processes out of timing runs. Record CPU,
governor, affinity, background load, allocator environment, revision hashes and
exact commands. Never infer a universal throughput gain from smaller futures.
