//! Example of using the `ArchiveCleaner`
//!
//! This example demonstrates how to set up and use the
//! `ArchiveCleaner` to periodically clean up archived jobs from a `PostgreSQL` database.
//!
//! The cleaner can be configured with different policies to manage the retention of archived jobs.
//!
//! In this example, we will configure the cleaner to remove archived jobs older than a specified duration.
//!
//! ```bash
//! cargo run --example archive_cleanup
//! ```
#![allow(clippy::cast_possible_wrap)]

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::time::Duration;
use testcontainers::ContainerAsync;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use tracing::info;
use workers::{
    ArchiveCleanerBuilder, BackgroundJob, CleanupConfiguration, CleanupPolicy, Runner,
    archived_job_count,
};

#[derive(Debug, Serialize, Deserialize)]
struct ReticulateSplineJob;

impl BackgroundJob for ReticulateSplineJob {
    const JOB_TYPE: &'static str = "reticulate_splines";
    type Context = ();

    async fn run(&self, _ctx: Self::Context) -> Result<()> {
        info!("Reticulating spline");
        tokio::time::sleep(Duration::from_millis(100)).await;
        info!("Spline reticulated");
        Ok(())
    }
}

/// Set up a `PostgreSQL` database using `TestContainers`
#[allow(clippy::cognitive_complexity)]
async fn setup_database() -> Result<(PgPool, ContainerAsync<Postgres>)> {
    info!("Starting PostgreSQL container...");
    let postgres_image = Postgres::default();
    let container = postgres_image.start().await?;

    let host = container.get_host().await?;
    let port = container.get_host_port_ipv4(5432).await?;
    let connection_string = format!("postgresql://postgres:postgres@{host}:{port}/postgres");

    info!("Connecting to database at {}:{}...", host, port);
    let pool = PgPool::connect(&connection_string).await?;

    info!("Running database migrations...");
    sqlx::migrate!("./migrations").run(&pool).await?;

    Ok((pool, container))
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<()> {
    const KEEP_JOBS: usize = 5;

    // Initialize tracing with compact formatting
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,archive=info,workers=info".into()),
        )
        .compact()
        .init();
    let (pool, _container) = setup_database().await?;

    let runner = Runner::new(pool.clone(), ())
        .register_job_type::<ReticulateSplineJob>()
        .configure_default_queue(|queue| queue.archive_completed_jobs(true))
        .shutdown_when_queue_empty();

    ArchiveCleanerBuilder::new()
        .configure::<ReticulateSplineJob>(CleanupConfiguration {
            cleanup_every: Duration::from_secs(1),
            policy: CleanupPolicy::MaxCount(KEEP_JOBS),
        })
        .run(&pool.clone());

    for _ in 0..10 {
        ReticulateSplineJob.enqueue(&pool).await?;
    }

    info!("Enqueued 10 ReticulateSplineJob jobs");

    info!("Processing jobs...");
    let handle = runner.start();
    handle.wait_for_shutdown().await;

    info!("All jobs processed, waiting for cleanup!");

    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(archived_job_count(&pool.clone()).await?, KEEP_JOBS as i64);

    info!("The cleaner has only kept the last 5 archived jobs as per the MaxCount policy.");
    Ok(())
}
