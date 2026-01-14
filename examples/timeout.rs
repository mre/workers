//! Job timeout example for the workers library.
//!
//! This example demonstrates queue-level timeouts.
//!
//! ## Cancel Safety
//!
//! When a job times out, it's cancelled at the next `.await` point.
//! Jobs should be idempotent and avoid leaving shared state inconsistent.
//!
//! Run with: `cargo run --example timeout`

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::time::Duration;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use workers::{BackgroundJob, Queue, Runner};

#[derive(Serialize, Deserialize)]
struct Job {
    duration_ms: u64,
}

impl BackgroundJob for Job {
    const JOB_TYPE: &'static str = "job";
    type Context = ();

    async fn run(&self, _ctx: Self::Context) -> Result<()> {
        tokio::time::sleep(Duration::from_millis(self.duration_ms)).await;
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Setup database
    let container = Postgres::default().start().await?;
    let port = container.get_host_port_ipv4(5432).await?;
    let pool = PgPool::connect(&format!(
        "postgresql://postgres:postgres@127.0.0.1:{port}/postgres"
    ))
    .await?;
    sqlx::migrate!("./migrations").run(&pool).await?;

    // Create queue with 1 second timeout
    let runner = Runner::new(pool.clone(), ())
        .add_queue(
            Queue::default()
                .register::<Job>()
                .timeout(Duration::from_secs(1)),
        )
        .shutdown_when_queue_empty();

    // Enqueue two jobs: one fast, one slow
    Job { duration_ms: 100 }.enqueue(&pool).await?; // Will complete (100ms < 1s)
    Job {
        duration_ms: 10_000,
    }
    .enqueue(&pool)
    .await?; // Will timeout (10s > 1s)

    // Run and verify: fast job completes, slow job times out
    runner.start().wait_for_shutdown().await;

    let failed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM background_jobs WHERE retries > 0")
        .fetch_one(&pool)
        .await?;

    assert_eq!(failed, 1, "Expected 1 failed job (the slow one)");
    Ok(())
}
