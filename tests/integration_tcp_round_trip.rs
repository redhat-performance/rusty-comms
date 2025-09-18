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

/// Minimal UDS server spawn smoke test on Unix
#[tokio::test]
#[cfg(unix)]
async fn uds_server_ready_smoke() -> Result<()> {
    let args = Args {
        mechanisms: vec![IpcMechanism::UnixDomainSocket],
        one_way: true,
        round_trip: false,
        warmup_iterations: 0,
        msg_count: 1,
        message_size: 64,
        include_first_message: true,
        ..Default::default()
    };
    let config = BenchmarkConfig::from_args(&args)?;
    let runner = BenchmarkRunner::new(config, IpcMechanism::UnixDomainSocket, args.clone());
    let transport_config = runner.create_transport_config_internal(&args)?;
    let (mut child, _reader) = runner.spawn_server_process(&transport_config)?;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let _ = child.kill();
    Ok(())
}
