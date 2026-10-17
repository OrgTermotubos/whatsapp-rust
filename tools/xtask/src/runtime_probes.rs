use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Output};
use xtask_support::{capture, write, write_json};

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct ProcessStats {
    user_seconds: f64,
    system_seconds: f64,
    max_rss_kib: u64,
    minor_faults: u64,
    major_faults: u64,
    exit_code: i32,
}

fn measured_command(executable: &str, stats: &Path) -> Command {
    let mut command = Command::new("/usr/bin/time");
    command.env("LC_ALL", "C").args([
        "--quiet",
        "--format",
        r#"{"user_seconds":%U,"system_seconds":%S,"max_rss_kib":%M,"minor_faults":%R,"major_faults":%F,"exit_code":%x}"#,
        "--output",
    ]).arg(stats).args(["--", executable]);
    command
}

fn prepare_output_dir(out_dir: &Path) -> Result<std::path::PathBuf> {
    // Children run at the workspace root, while the caller may be in a member.
    // Resolve before creating files so both processes use the same directory.
    let out_dir = std::path::absolute(out_dir)?;
    if out_dir.exists() {
        ensure!(
            std::fs::read_dir(&out_dir)?.next().is_none(),
            "probe output directory must be empty"
        );
    }
    std::fs::create_dir_all(&out_dir)?;
    Ok(out_dir)
}

pub fn run(root: &Path, out_dir: &Path) -> Result<()> {
    let out_dir = prepare_output_dir(out_dir)?;
    let out_dir = out_dir.as_path();
    eprintln!("building native runtime probe executables");
    let build = capture_build(
        Command::new("cargo").current_dir(root).args([
            "build",
            "--locked",
            "--profile",
            "bench",
            "-p",
            "whatsapp-rust",
            "--features",
            "bench-harness,sqlite-storage-bundled",
            "--example",
            "runtime_latency",
            "--example",
            "runtime_footprint",
            "--message-format=json-render-diagnostics",
        ]),
        out_dir,
    )?;
    let mut executables = BTreeMap::new();
    for line in build
        .stdout
        .split(|&byte| byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let item: Value = serde_json::from_slice(line)?;
        if item["reason"] == "compiler-artifact"
            && let (Some(name), Some(path)) =
                (item["target"]["name"].as_str(), item["executable"].as_str())
            && matches!(name, "runtime_latency" | "runtime_footprint")
        {
            executables.insert(name.to_owned(), path.to_owned());
        }
    }
    ensure!(
        executables.len() == 2,
        "both runtime probe executables are required"
    );
    let output = |program: &str, args: &[&str]| -> Result<String> {
        Ok(String::from_utf8(
            capture(Command::new(program).current_dir(root).args(args))?.stdout,
        )?)
    };
    let hashes = executables
        .iter()
        .map(|(name, path)| Ok((name.clone(), xtask_support::sha256(&std::fs::read(path)?))))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let allocator_environment = [
        "MALLOC_ARENA_MAX",
        "MALLOC_MMAP_THRESHOLD_",
        "MALLOC_TRIM_THRESHOLD_",
        "MALLOC_TOP_PAD_",
        "GLIBC_TUNABLES",
    ]
    .into_iter()
    .filter_map(|name| std::env::var(name).ok().map(|value| (name, value)))
    .collect::<BTreeMap<_, _>>();
    write_json(
        &out_dir.join("metadata.json"),
        &json!({
            "revision": output("git", &["rev-parse", "HEAD"] )?.trim(),
            "working_tree_status": output("git", &["status", "--porcelain"] )?,
            "rustc": output("rustc", &["-Vv"] )?,
            "host": output("uname", &["-a"] )?,
            "cpu": output("lscpu", &[] )?,
            "executable_sha256": hashes,
            "rounds": 3,
            "profile": "bench",
            "features": "default,bench-harness,sqlite-storage-bundled",
            "allocator_environment": allocator_environment,
            "process_stats": "GNU time whole-process CPU, page faults and peak RSS; LC_ALL=C",
            "scope": "synthetic warm fixtures; System timing and separate global DHAT Rust heap; no RSS or TCP/TLS; shared runner timing is observational",
        }),
    )?;
    let cases = [
        ("dm-lid-1", "0", "1"),
        ("dm-pn-5", "0", "1"),
        ("group-64", "0", "1"),
        ("receive-dm", "0", "1"),
        ("patch-1", "0", "1"),
        ("patch-100", "4", "8"),
        ("download-64k", "0", "1"),
        ("download-1m", "4", "8"),
        ("prefetch-64k", "0", "1"),
        ("prefetch-64k", "4", "8"),
        ("prefetch-1m", "0", "1"),
        ("prefetch-1m", "4", "8"),
        ("connected-memory", "0", "1"),
        ("connected-sqlite", "0", "1"),
    ];
    for round in 0..3 {
        for (scenario, workers, clients) in cases {
            for (kind, samples) in [("latency", "60"), ("footprint", "3")] {
                let stem = format!("{scenario}-w{workers}-c{clients}-r{round}-{kind}");
                eprintln!("runtime probe {stem}");
                let stdout = std::fs::File::create(out_dir.join(format!("{stem}.jsonl")))?;
                let stderr = std::fs::File::create(out_dir.join(format!("{stem}.stderr")))?;
                let name = format!("runtime_{kind}");
                let stats_path = out_dir.join(format!("{stem}.process.json"));
                let status = measured_command(&executables[&name], &stats_path)
                    .current_dir(root)
                    .args([scenario, workers, clients, samples])
                    .stdout(stdout)
                    .stderr(stderr)
                    .status()
                    .with_context(|| format!("running {stem}"))?;
                write_json(
                    &out_dir.join(format!("{stem}.status.json")),
                    &json!({
                        "success": status.success(),
                        "code": status.code(),
                    }),
                )?;
                ensure!(
                    status.success(),
                    "{stem} failed with {status}; see its stderr artifact"
                );
                let stats: ProcessStats = serde_json::from_slice(&std::fs::read(&stats_path)?)?;
                ensure!(
                    stats.exit_code == 0
                        && stats.user_seconds.is_finite()
                        && stats.user_seconds >= 0.0
                        && stats.system_seconds.is_finite()
                        && stats.system_seconds >= 0.0,
                    "invalid process statistics for {stem}"
                );
                write_json(&stats_path, &stats)?;
                let rows = std::fs::read_to_string(out_dir.join(format!("{stem}.jsonl")))?;
                let rows = rows
                    .lines()
                    .map(serde_json::from_str::<Value>)
                    .collect::<Result<Vec<_>, _>>()?;
                ensure!(
                    rows.len() == samples.parse::<usize>()?,
                    "incomplete output for {stem}"
                );
            }
        }
    }
    Ok(())
}

fn capture_build(command: &mut Command, out_dir: &Path) -> Result<Output> {
    let output = command.output().context("building runtime probes")?;
    write(&out_dir.join("build.stdout.jsonl"), &output.stdout)?;
    write(&out_dir.join("build.stderr"), &output.stderr)?;
    write_json(
        &out_dir.join("build.status.json"),
        &json!({
            "success": output.status.success(),
            "code": output.status.code(),
        }),
    )?;
    ensure!(
        output.status.success(),
        "native probe build failed with {}; diagnostics saved in {}",
        output.status,
        out_dir.display(),
    );
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "linux")]
    fn relative_output_from_member_survives_child_working_directory_change() {
        let root = tempfile::tempdir().unwrap();
        let member = root.path().join("member");
        std::fs::create_dir(&member).unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .current_dir(&member)
            .env("XTASK_RELATIVE_OUTPUT_ROOT", root.path())
            .args([
                "--ignored",
                "--exact",
                "runtime_probes::tests::relative_output_child",
                "--nocapture",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stats: ProcessStats =
            serde_json::from_slice(&std::fs::read(member.join("results/process.json")).unwrap())
                .unwrap();
        assert_eq!(stats.exit_code, 23);
        assert!(!root.path().join("results").exists());
    }

    #[test]
    #[ignore = "subprocess fixture"]
    #[cfg(target_os = "linux")]
    fn relative_output_child() {
        let Some(root) = std::env::var_os("XTASK_RELATIVE_OUTPUT_ROOT") else {
            return;
        };
        let output_dir = prepare_output_dir(Path::new("results")).unwrap();
        assert!(output_dir.is_absolute());
        let output = measured_command(
            std::env::current_exe().unwrap().to_str().unwrap(),
            &output_dir.join("process.json"),
        )
        .current_dir(root)
        .env("XTASK_COMPILER_FAILURE_FIXTURE", "1")
        .args([
            "--ignored",
            "--exact",
            "runtime_probes::tests::compiler_error_child",
            "--nocapture",
        ])
        .output()
        .unwrap();
        assert_eq!(output.status.code(), Some(23));
        assert!(prepare_output_dir(Path::new("results")).is_err());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn process_measurement_preserves_failed_child_status_and_diagnostics() {
        let output_dir = tempfile::tempdir().unwrap();
        let stats_path = output_dir.path().join("process.json");
        let output = measured_command(
            std::env::current_exe().unwrap().to_str().unwrap(),
            &stats_path,
        )
        .env("XTASK_COMPILER_FAILURE_FIXTURE", "1")
        .args([
            "--ignored",
            "--exact",
            "runtime_probes::tests::compiler_error_child",
            "--nocapture",
        ])
        .output()
        .unwrap();
        assert_eq!(output.status.code(), Some(23));
        assert!(
            String::from_utf8(output.stdout)
                .unwrap()
                .contains("synthetic compiler error")
        );
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains("synthetic cargo failure summary")
        );
        let stats: ProcessStats =
            serde_json::from_slice(&std::fs::read(stats_path).unwrap()).unwrap();
        assert_eq!(stats.exit_code, 23);
        assert!(stats.user_seconds >= 0.0 && stats.system_seconds >= 0.0);
    }

    #[test]
    #[ignore = "subprocess fixture"]
    fn compiler_error_child() {
        if std::env::var_os("XTASK_COMPILER_FAILURE_FIXTURE").is_none() {
            return;
        }
        println!(
            r#"{{"reason":"compiler-message","message":{{"rendered":"synthetic compiler error"}}}}"#
        );
        eprintln!("synthetic cargo failure summary");
        std::process::exit(23);
    }

    #[test]
    fn failed_build_preserves_compiler_diagnostics_and_exit_status() {
        let output = tempfile::tempdir().unwrap();
        let error = capture_build(
            Command::new(std::env::current_exe().unwrap())
                .env("XTASK_COMPILER_FAILURE_FIXTURE", "1")
                .args([
                    "--ignored",
                    "--exact",
                    "runtime_probes::tests::compiler_error_child",
                    "--nocapture",
                ]),
            output.path(),
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("native probe build failed"),
            "{error:#}"
        );
        let stdout = std::fs::read_to_string(output.path().join("build.stdout.jsonl")).unwrap();
        let stderr = std::fs::read_to_string(output.path().join("build.stderr")).unwrap();
        assert!(stdout.contains("synthetic compiler error"));
        assert!(stderr.contains("synthetic cargo failure summary"));
        let status: Value = serde_json::from_slice(
            &std::fs::read(output.path().join("build.status.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(status["success"], false);
        assert_eq!(status["code"], 23);
    }
}
