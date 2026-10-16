use anyhow::{Result, bail, ensure};
use serde::Serialize;
use std::sync::Arc;
use std::time::Instant;
use wacore::appstate::patch_decode::{PatchList, WAPatchName};
use wacore::appstate_sync::AppStateProcessor;
use wacore::store::traits::AppStateSyncKey;
use waproto::whatsapp as wa;
use whatsapp_rust::TokioRuntime;
use whatsapp_rust::bench_support::connected_idle::{BackendFixture, MESSAGES, Session};
use whatsapp_rust::bench_support::{
    DmAddressing, DmSendHarness, GroupSendHarness, PrefetchHarness, ReceiveHarness,
    StreamingBodyHttp,
};
use whatsapp_rust::download::{DownloadLimits, DownloadParams, MediaDownloader, MediaType};

#[derive(Serialize)]
struct Sample {
    scenario: String,
    workers: usize,
    clients: usize,
    sample: usize,
    operations: usize,
    elapsed_ns: Option<u128>,
    allocated_bytes: Option<u64>,
    allocations: Option<u64>,
    peak_bytes: Option<usize>,
    retained_bytes: Option<usize>,
}

struct Measurement {
    profiler: Option<dhat::Profiler>,
    start: Instant,
}

impl Measurement {
    // Performance probes require walltime even when the library's test clock is replaced.
    #[allow(clippy::disallowed_methods)]
    fn start(memory: bool) -> Self {
        let profiler = memory.then(|| dhat::Profiler::builder().testing().build());
        Self {
            profiler,
            start: Instant::now(),
        }
    }

    fn finish(
        self,
        scenario: &str,
        workers: usize,
        clients: usize,
        sample: usize,
        operations: usize,
    ) -> Result<()> {
        let elapsed = self.start.elapsed().as_nanos();
        let stats = self.profiler.as_ref().map(|_| dhat::HeapStats::get());
        let output = Sample {
            scenario: scenario.to_owned(),
            workers,
            clients,
            sample,
            operations,
            elapsed_ns: stats.is_none().then_some(elapsed),
            allocated_bytes: stats.as_ref().map(|s| s.total_bytes),
            allocations: stats.as_ref().map(|s| s.total_blocks),
            peak_bytes: stats.as_ref().map(|s| s.max_bytes),
            retained_bytes: stats.as_ref().map(|s| s.curr_bytes),
        };
        drop(self.profiler);
        serde_json::to_writer(std::io::stdout().lock(), &output)?;
        // JSONL is the diagnostic's output contract.
        #[allow(clippy::print_stdout)]
        {
            println!();
        }
        Ok(())
    }
}

fn runtime(workers: usize) -> Result<tokio::runtime::Runtime> {
    let mut builder = if workers == 0 {
        tokio::runtime::Builder::new_current_thread()
    } else {
        let mut builder = tokio::runtime::Builder::new_multi_thread();
        builder.worker_threads(workers);
        builder
    };
    Ok(builder.enable_all().build()?)
}

pub fn run(memory: bool) -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    ensure!(
        args.len() == 4,
        "usage: <scenario> <workers: 0=current-thread> <clients> <samples>"
    );
    let scenario = &args[0];
    let workers = args[1].parse()?;
    let clients = args[2].parse()?;
    let samples = args[3].parse()?;
    ensure!(
        (1..=64).contains(&clients) && (1..=10000).contains(&samples) && workers <= 16,
        "out-of-range workload"
    );
    match scenario.as_str() {
        "dm-lid-1" | "dm-pn-5" | "group-64" | "receive-dm" => {
            ensure!(
                workers == 0 && clients == 1 && samples <= 800,
                "warm fixtures use one current-thread client and stay below ratchet rotation"
            );
            warm(scenario, samples, memory)
        }
        "connected-memory" | "connected-sqlite" => {
            connected(scenario, workers, clients, samples, memory)
        }
        "patch-1" | "patch-100" => patches(scenario, workers, clients, samples, memory),
        "download-64k" | "download-1m" => downloads(scenario, workers, clients, samples, memory),
        "prefetch-64k" | "prefetch-1m" => prefetch(scenario, workers, clients, samples, memory),
        _ => bail!("unknown scenario {scenario}"),
    }
}

fn warm(scenario: &str, samples: usize, memory: bool) -> Result<()> {
    let dm = match scenario {
        "dm-lid-1" => Some(DmSendHarness::new(1, DmAddressing::Lid)),
        "dm-pn-5" => Some(DmSendHarness::new(5, DmAddressing::PnWithKnownLid)),
        _ => None,
    };
    let group = (scenario == "group-64").then(|| GroupSendHarness::new(64));
    let receiver = (scenario == "receive-dm").then(ReceiveHarness::new);
    for index in 0..samples + 20 {
        let node = receiver.as_ref().map(ReceiveHarness::dm_stanza);
        let before = receiver.as_ref().map(ReceiveHarness::messages_delivered);
        let measurement = (index >= 20).then(|| Measurement::start(memory));
        if let Some(dm) = &dm {
            dm.warm_send();
        }
        if let Some(group) = &group {
            group.warm_send();
        }
        if let Some(receiver) = &receiver {
            receiver.receive(node.expect("prepared ciphertext"));
        }
        if let Some(measurement) = measurement {
            measurement.finish(scenario, 0, 1, index - 20, 1)?;
        }
        if let Some(receiver) = &receiver {
            ensure!(
                receiver.messages_delivered() == before.expect("receiver count") + 1,
                "decrypt/event failure"
            );
        }
    }
    Ok(())
}

fn connected(
    scenario: &str,
    workers: usize,
    clients: usize,
    samples: usize,
    memory: bool,
) -> Result<()> {
    let rt = runtime(workers)?;
    for index in 0..samples + 1 {
        rt.block_on(async {
            let mut fixtures = Vec::with_capacity(clients);
            for _ in 0..clients {
                let backend = if scenario == "connected-sqlite" {
                    BackendFixture::sqlite().await?
                } else {
                    BackendFixture::memory()
                };
                let session = Session::connect(backend.backend()).await?;
                let activity = session.prepare_activity().await?;
                fixtures.push((backend, session, activity));
            }
            let measurement = (index > 0).then(|| Measurement::start(memory));
            let results = futures::future::join_all(fixtures.into_iter().map(
                |(backend, session, activity)| {
                    tokio::spawn(async move {
                        session.receive_activity(activity).await?;
                        Ok::<_, anyhow::Error>((backend, session))
                    })
                },
            ))
            .await;
            if let Some(measurement) = measurement {
                measurement.finish(scenario, workers, clients, index - 1, clients * MESSAGES)?;
            }
            for result in results {
                let (backend, session) = result??;
                session.shutdown().await?;
                backend.cleanup()?;
            }
            Ok::<_, anyhow::Error>(())
        })?;
    }
    Ok(())
}

fn downloads(
    scenario: &str,
    workers: usize,
    clients: usize,
    samples: usize,
    memory: bool,
) -> Result<()> {
    let size = if scenario == "download-64k" {
        65536
    } else {
        1048576
    };
    let body = vec![7; size];
    let encrypted = wacore::upload::encrypt_media(&body, MediaType::AppState)?;
    let params = DownloadParams::encrypted(
        "/synthetic-runtime-probe",
        &encrypted.media_key,
        &encrypted.file_sha256,
        &encrypted.file_enc_sha256,
        size as u64,
        MediaType::AppState,
    )?;
    let downloader = MediaDownloader::with_default_hosts(
        Arc::new(StreamingBodyHttp::new(encrypted.data_to_upload.into())),
        Arc::new(TokioRuntime),
    );
    let rt = runtime(workers)?;
    for index in 0..samples + 5 {
        let measurement = (index >= 5).then(|| Measurement::start(memory));
        let results = rt.block_on(futures::future::join_all(
            (0..clients).map(|_| downloader.download(&params, DownloadLimits::default())),
        ));
        if let Some(measurement) = measurement {
            measurement.finish(scenario, workers, clients, index - 5, clients)?;
        }
        for result in results {
            ensure!(result? == body, "media validation/output failure");
        }
    }
    Ok(())
}

fn prefetch(
    scenario: &str,
    workers: usize,
    clients: usize,
    samples: usize,
    memory: bool,
) -> Result<()> {
    let size = if scenario == "prefetch-64k" {
        65536
    } else {
        1048576
    };
    let rt = runtime(workers)?;
    rt.block_on(async {
        let mut fixtures = Vec::with_capacity(clients);
        for _ in 0..clients {
            fixtures.push(Arc::new(PrefetchHarness::new(size).await?));
        }
        for index in 0..samples + 5 {
            let measurement = (index >= 5).then(|| Measurement::start(memory));
            let results = futures::future::join_all(fixtures.iter().map(|fixture| {
                let fixture = Arc::clone(fixture);
                tokio::spawn(async move { fixture.prefetch().await })
            }))
            .await;
            if let Some(measurement) = measurement {
                measurement.finish(scenario, workers, clients, index - 5, clients * 4)?;
            }
            for result in results {
                result??;
            }
        }
        Ok(())
    })
}

fn patches(
    scenario: &str,
    workers: usize,
    clients: usize,
    samples: usize,
    memory: bool,
) -> Result<()> {
    let width = if scenario == "patch-1" { 1 } else { 100 };
    let rt = runtime(workers)?;
    rt.block_on(async {
        let mut fixtures = Vec::with_capacity(clients);
        let key_id = b"synthetic-runtime-probe";
        let key = [7; 32];
        let keys = wacore::appstate::keys::expand_app_state_keys(&key);
        let mut value = wa::SyncActionValue::default();
        value.push_name_setting.get_or_insert_default().name = Some("Synthetic".into());
        value.timestamp = Some(1234567890);
        let mutations: Vec<_> = (0..width)
            .map(|index| {
                let path = format!("[\"setting_pushName\",\"synthetic-{index}\"]");
                wacore::appstate::encode::encode_record(
                    wa::syncd_mutation::SyncdOperation::SET,
                    path.as_bytes(),
                    &value,
                    &keys,
                    key_id,
                    &[0; 16],
                    1,
                )
                .0
            })
            .collect();
        for _ in 0..clients {
            let backend = BackendFixture::memory();
            backend
                .backend()
                .set_sync_key(
                    key_id,
                    AppStateSyncKey {
                        key_data: key.to_vec(),
                        fingerprint: vec![],
                        timestamp: 1,
                    },
                )
                .await?;
            let processor = Arc::new(AppStateProcessor::new(
                backend.backend(),
                Arc::new(TokioRuntime),
            ));
            fixtures.push((backend, processor));
        }
        for index in 0..samples + 5 {
            let mut lists = Vec::with_capacity(clients);
            for (_, processor) in &fixtures {
                let (encoded, version) =
                    processor.build_patch("regular", mutations.clone()).await?;
                let mut patch = waproto::codec::syncd_patch_decode(&encoded)?;
                patch.version.get_or_insert_default().version = Some(version + 1);
                lists.push(PatchList {
                    name: WAPatchName::Regular,
                    has_more_patches: false,
                    patches: vec![patch],
                    snapshot: None,
                    snapshot_ref: None,
                    error: None,
                });
            }
            let measurement = (index >= 5).then(|| Measurement::start(memory));
            let results = futures::future::join_all(fixtures.iter().zip(lists).map(
                |((_, processor), list)| {
                    let processor = Arc::clone(processor);
                    tokio::spawn(async move { processor.process_patch_list(list, true).await })
                },
            ))
            .await;
            if let Some(measurement) = measurement {
                measurement.finish(scenario, workers, clients, index - 5, clients * width)?;
            }
            for ((backend, _), result) in fixtures.iter().zip(results) {
                let (decoded, state, _) = result??;
                ensure!(
                    decoded.len() == width && state.version == (index + 1) as u64,
                    "patch decode/version mismatch"
                );
                ensure!(
                    backend
                        .backend()
                        .get_version("regular")
                        .await?
                        .is_some_and(|persisted| persisted.version == state.version
                            && persisted.hash == state.hash),
                    "patch persistence mismatch"
                );
            }
        }
        for (backend, _) in fixtures {
            backend.cleanup()?;
        }
        Ok(())
    })
}
