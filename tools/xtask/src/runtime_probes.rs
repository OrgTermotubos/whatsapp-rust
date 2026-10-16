use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Output};
use xtask_support::{capture, write, write_json};

pub fn run(root: &Path, out_dir: &Path) -> Result<()> {
    if out_dir.exists() {
        ensure!(
            std::fs::read_dir(out_dir)?.next().is_none(),
            "probe output directory must be empty"
        );
    }
    std::fs::create_dir_all(out_dir)?;
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
                let status = Command::new(&executables[&name])
                    .current_dir(root)
                    .args([scenario, workers, clients, samples])
                    .stdout(stdout)
                    .stderr(stderr)
                    .status()
                    .with_context(|| format!("running {stem}"))?;
                ensure!(
                    status.success(),
                    "{stem} failed with {status}; see its stderr artifact"
                );
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
    #[ignore = "subprocess fixture"]
    fn compiler_error_child() {
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
            Command::new(std::env::current_exe().unwrap()).args([
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
