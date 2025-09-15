#[cfg(target_os = "linux")]
use anyhow::Result;
#[cfg(target_os = "linux")]
use ipc_benchmark::{cli::Args, BenchmarkConfig, BenchmarkRunner, IpcMechanism};

/// Verify PMQ round-trip works end-to-end with a spawned server process.
#[cfg(target_os = "linux")]
/// Note: PMQ may require specific kernel config and permissions (e.g.,
/// mounted /dev/mqueue, SELinux policy, RLIMIT_MSGQUEUE). This test auto-skips
/// at runtime unless explicitly enabled via the environment and basic
/// directory checks pass.
#[tokio::test]
async fn pmq_round_trip_process_smoke() -> Result<()> {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    // Require explicit opt-in to run PMQ test in this environment
    let allow_env = std::env::var("IPC_BENCHMARK_RUN_PMQ").unwrap_or_default();
    if allow_env != "1" {
        eprintln!("Skipping PMQ test: set IPC_BENCHMARK_RUN_PMQ=1 to enable in this env");
        return Ok(());
    }

    let mq_dir = Path::new("/dev/mqueue");
    if !mq_dir.exists() || !mq_dir.is_dir() {
        eprintln!("Skipping PMQ test: /dev/mqueue not present");
        return Ok(());
    }
    if let Ok(md) = fs::metadata(mq_dir) {
        let mode = md.permissions().mode();
        // Require write and execute on directory (create/unlink entries)
        if (mode & 0o200) == 0 || (mode & 0o100) == 0 {
            eprintln!("Skipping PMQ test: /dev/mqueue not writable/executable by user");
            return Ok(());
        }
    }
    // Use a unique queue name to avoid permission conflicts with existing queues
    let unique = format!("/ipc_benchmark_integration_pmq_{}", std::process::id());
    let args = Args {
        mechanisms: vec![IpcMechanism::PosixMessageQueue],
        one_way: false,
        round_trip: true,
        warmup_iterations: 0,
        concurrency: 1,
        msg_count: 32,
        message_queue_name: Some(unique),
        ..Default::default()
    };

    let config = BenchmarkConfig::from_args(&args)?;
    let runner = BenchmarkRunner::new(config, IpcMechanism::PosixMessageQueue, args.clone());

    let _results = runner.run(None).await?;
    Ok(())
}
