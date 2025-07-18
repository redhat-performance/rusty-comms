use clap::Parser;
use ipc_benchmark::{
    benchmark::{BenchmarkConfig, BenchmarkRunner},
    cli::{Args, IpcMechanism},
    results::ResultsManager,
};
use tracing::{error, info};
// Removed redundant import
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();

    info!("Starting IPC Benchmark Suite");
    info!("Configuration: {:?}", args);

    // Create benchmark configuration
    let config = BenchmarkConfig::from_args(&args)?;

    // Initialize results manager
    let mut results_manager = ResultsManager::new(&args.output_file)?;

    // Enable streaming output if specified
    if let Some(ref streaming_file) = args.streaming_output {
        info!("Enabling streaming output to: {:?}", streaming_file);
        results_manager.enable_streaming(streaming_file)?;
    }

    // Get expanded mechanisms (handles 'all' expansion)
    let mechanisms = IpcMechanism::expand_all(args.mechanisms.clone());

    // Run benchmarks for each specified IPC mechanism
    for (i, mechanism) in mechanisms.iter().enumerate() {
        info!("Running benchmark for mechanism: {:?}", mechanism);

        // Add delay between mechanisms to allow proper resource cleanup
        if i > 0 {
            info!("Waiting for resource cleanup...");
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }

        match run_benchmark_for_mechanism(&config, mechanism, &mut results_manager).await {
            Ok(_) => info!("Benchmark completed successfully for {:?}", mechanism),
            Err(e) => {
                error!("Benchmark failed for {:?}: {}", mechanism, e);
                if !args.continue_on_error {
                    return Err(e);
                }
            }
        }
    }

    // Finalize results and output
    results_manager.finalize()?;

    info!("IPC Benchmark Suite completed successfully");
    Ok(())
}

async fn run_benchmark_for_mechanism(
    config: &BenchmarkConfig,
    mechanism: &IpcMechanism,
    results_manager: &mut ResultsManager,
) -> Result<()> {
    let runner = BenchmarkRunner::new(config.clone(), *mechanism);
    let results = runner.run().await?;
    results_manager.add_results(results).await?;
    Ok(())
}
