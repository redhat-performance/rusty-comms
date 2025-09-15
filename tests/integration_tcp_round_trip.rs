use anyhow::Result;
use ipc_benchmark::{cli::Args, BenchmarkConfig, BenchmarkRunner, IpcMechanism};

/// Verify TCP round-trip works end-to-end with a spawned server process.
///
/// This is a lightweight smoke test that ensures the process-based
/// client/server separation functions correctly for TCP.
#[tokio::test]
async fn tcp_round_trip_process_smoke() -> Result<()> {
    // Keep the run short and deterministic. Avoid relying on Default for Args,
    // since derive(Default) does not apply clap defaults.
    let args = Args {
        mechanisms: vec![IpcMechanism::TcpSocket],
        one_way: false,
        round_trip: true,
        warmup_iterations: 0,
        // Explicit network parameters
        host: "127.0.0.1".to_string(),
        port: 20000, // base; runner will offset to a unique high port
        // Ensure non-zero concurrency and finite iteration mode
        concurrency: 1,
        msg_count: 32,
        ..Default::default()
    };

    let config = BenchmarkConfig::from_args(&args)?;
    let runner = BenchmarkRunner::new(config, IpcMechanism::TcpSocket, args.clone());

    let _results = runner.run(None).await?;
    Ok(())
}
