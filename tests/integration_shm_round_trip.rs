use anyhow::Result;
use ipc_benchmark::{cli::Args, BenchmarkConfig, BenchmarkRunner, IpcMechanism};

/// Verify SHM round-trip works end-to-end with a spawned server process.
#[tokio::test]
async fn shm_round_trip_process_smoke() -> Result<()> {
    // Ensure a deterministic shared memory name
    let args = Args {
        mechanisms: vec![IpcMechanism::SharedMemory],
        one_way: false,
        round_trip: true,
        warmup_iterations: 0,
        concurrency: 1,
        msg_count: 32,
        shared_memory_name: Some("ipc_benchmark_integration_shm".to_string()),
        ..Default::default()
    };

    let config = BenchmarkConfig::from_args(&args)?;
    let runner = BenchmarkRunner::new(config, IpcMechanism::SharedMemory, args.clone());

    let _results = runner.run(None).await?;
    Ok(())
}
