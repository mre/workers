//! Archive functionality example for the workers library.
//!
//! This example demonstrates how to configure job archiving to preserve
//! completed jobs for debugging, auditing, and analytics purposes.
//!
//! This example uses TestContainers to automatically start a PostgreSQL
//! database, so no manual setup is required. Just run:
//!
//! ```bash
//! cargo run --example archive
//! ```

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::time::Duration;
use testcontainers::ContainerAsync;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use workers::{BackgroundJob, Runner, archived_job_count, get_archived_jobs};

/// Example job that processes user notifications
#[derive(Serialize, Deserialize)]
struct NotificationJob {
    user_id: u64,
    message: String,
    notification_type: String,
}

impl BackgroundJob for NotificationJob {
    const JOB_NAME: &'static str = "notification";
    type Context = ();

    async fn run(&self, _ctx: Self::Context) -> Result<()> {
        println!(
            "Sending {} notification to user {}: {}",
            self.notification_type, self.user_id, self.message
        );

        // Simulate some processing time
        tokio::time::sleep(Duration::from_millis(100)).await;

        println!("Notification sent successfully");
        Ok(())
    }
}

/// Example job that processes payment transactions
#[derive(Serialize, Deserialize)]
struct PaymentJob {
    transaction_id: String,
    amount: f64,
    currency: String,
}

impl BackgroundJob for PaymentJob {
    const JOB_NAME: &'static str = "payment";
    type Context = ();

    async fn run(&self, _ctx: Self::Context) -> Result<()> {
        println!(
            "Processing payment: {} {} (ID: {})",
            self.amount, self.currency, self.transaction_id
        );

        // Simulate payment processing
        tokio::time::sleep(Duration::from_millis(200)).await;

        println!("Payment processed successfully");
        Ok(())
    }
}

/// Set up a PostgreSQL database using TestContainers
async fn setup_database() -> Result<(PgPool, ContainerAsync<Postgres>)> {
    println!("Starting PostgreSQL container...");
    let postgres_image = Postgres::default();
    let container = postgres_image.start().await?;

    let host = container.get_host().await?;
    let port = container.get_host_port_ipv4(5432).await?;
    let connection_string = format!("postgresql://postgres:postgres@{}:{}/postgres", host, port);

    println!("Connecting to database at {}:{}...", host, port);
    let pool = PgPool::connect(&connection_string).await?;

    println!("Running database migrations...");
    sqlx::migrate!("./migrations").run(&pool).await?;

    Ok((pool, container))
}

#[tokio::main]
async fn main() -> Result<()> {
    // Set up database using TestContainers
    let (pool, _container) = setup_database().await?;

    println!("Starting archive example...\n");

    // Create runner with archiving enabled for important jobs
    let runner = Runner::new(pool.clone(), ())
        .register_job_type::<NotificationJob>()
        .register_job_type::<PaymentJob>()
        .configure_queue("default", |queue| {
            queue
                .num_workers(2)
                .poll_interval(Duration::from_millis(100))
                .archive_completed_jobs(true) // Enable archiving for audit trail
        })
        .shutdown_when_queue_empty();

    // Enqueue some notification jobs
    let notifications = vec![
        NotificationJob {
            user_id: 123,
            message: "Welcome to our platform!".to_string(),
            notification_type: "welcome".to_string(),
        },
        NotificationJob {
            user_id: 456,
            message: "Your order has been shipped".to_string(),
            notification_type: "shipping".to_string(),
        },
    ];

    // Enqueue some payment jobs
    let payments = vec![
        PaymentJob {
            transaction_id: "txn_001".to_string(),
            amount: 99.99,
            currency: "USD".to_string(),
        },
        PaymentJob {
            transaction_id: "txn_002".to_string(),
            amount: 29.99,
            currency: "EUR".to_string(),
        },
    ];

    // Enqueue all jobs
    for notification in notifications {
        notification.enqueue(&pool).await?;
    }

    for payment in payments {
        payment.enqueue(&pool).await?;
    }

    println!("Enqueued 4 jobs total\n");

    // Start processing
    println!("Processing jobs...\n");
    let handle = runner.start();
    handle.wait_for_shutdown().await;

    println!("\nAll jobs completed!\n");

    // Now demonstrate archive functionality
    println!("Archive Analysis:");
    println!("=================");

    // Get total count of archived jobs
    let total_archived = archived_job_count(&pool).await?;
    println!("Total archived jobs: {}", total_archived);

    // Get archived notification jobs
    let notification_archives = get_archived_jobs(&pool, Some("notification"), None).await?;
    println!(
        "Notification jobs archived: {}",
        notification_archives.len()
    );

    // Get archived payment jobs
    let payment_archives = get_archived_jobs(&pool, Some("payment"), None).await?;
    println!("Payment jobs archived: {}", payment_archives.len());

    // Show details of archived jobs
    println!("\nArchived Job Details:");
    println!("====================");

    let all_archived = get_archived_jobs(&pool, None, Some(10)).await?;
    for job in all_archived {
        println!(
            "Job ID: {}, Type: {}, Archived: {}",
            job.job.id,
            job.job.job_type,
            job.archived_at.format("%Y-%m-%d %H:%M:%S")
        );

        // Pretty print the job data
        if let Ok(pretty_data) = serde_json::to_string_pretty(&job.job.data) {
            println!("   Data: {}", pretty_data.replace('\n', "\n         "));
        }
        println!();
    }

    println!("Archive example completed! Jobs are preserved for debugging and audit purposes.");

    // The PostgreSQL container will be automatically cleaned up when _container is dropped
    println!("\nCleaning up PostgreSQL container...");

    Ok(())
}
