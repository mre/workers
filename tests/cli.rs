#![allow(missing_docs)]
#![allow(clippy::expect_used)]
#![allow(clippy::panic)]
#![allow(clippy::unwrap_used)]
#![allow(clippy::indexing_slicing)]

use claims::{assert_none, assert_some};
use insta::assert_compact_json_snapshot;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{PgPool, Row};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use testcontainers::ContainerAsync;
use testcontainers_modules::postgres::Postgres;
use tokio::sync::Barrier;
use workers::{
    ArchiveQuery, BackgroundJob, Runner, archived_job_count, get_archived_jobs, setup_database,
};

/// Test utilities and common setup
mod test_utils {
    use super::*;
    use testcontainers::runners::AsyncRunner;

    /// Set up a test database with `TestContainers` and return the pool and container
    pub(super) async fn setup_test_db() -> anyhow::Result<(PgPool, ContainerAsync<Postgres>)> {
        let postgres_image = Postgres::default();
        let container = postgres_image.start().await?;

        // Get the connection parameters from the container
        let host = container.get_host().await?;
        let port = container.get_host_port_ipv4(5432).await?;

        // Use the standard postgres/postgres credentials for testcontainers
        let connection_string = format!("postgresql://postgres:postgres@{host}:{port}/postgres");

        let pool = PgPool::connect(&connection_string).await?;

        // Run migrations
        sqlx::migrate!("./migrations").run(&pool).await?;

        Ok((pool, container))
    }

    /// Set up a test database using the public `setup_database` function
    pub(super) async fn setup_test_db_with_public_api()
    -> anyhow::Result<(PgPool, ContainerAsync<Postgres>)> {
        let postgres_image = Postgres::default();
        let container = postgres_image.start().await?;

        let host = container.get_host().await?;
        let port = container.get_host_port_ipv4(5432).await?;
        let connection_string = format!("postgresql://postgres:postgres@{host}:{port}/postgres");

        let pool = PgPool::connect(&connection_string).await?;

        // Use the public API instead of direct migrations
        setup_database(&pool).await?;

        Ok((pool, container))
    }

    /// Create a test runner with common configuration
    pub(super) fn create_test_runner<Context: Clone + Send + Sync + 'static>(
        pool: PgPool,
        context: Context,
    ) -> Runner<Context> {
        Runner::new(pool, context)
            .configure_default_queue(|queue| queue.num_workers(2))
            .shutdown_when_queue_empty()
    }
}

async fn all_jobs(pool: &PgPool) -> anyhow::Result<Vec<(String, Value)>> {
    let jobs = sqlx::query("SELECT job_type, data FROM background_jobs")
        .fetch_all(pool)
        .await?;

    Ok(jobs
        .into_iter()
        .map(|row| {
            let job_type: String = row.get("job_type");
            let data: Value = row.get("data");
            (job_type, data)
        })
        .collect())
}

async fn job_exists(id: i64, pool: &PgPool) -> anyhow::Result<bool> {
    let result =
        sqlx::query_scalar::<_, Option<i64>>("SELECT id FROM background_jobs WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?;

    Ok(result.is_some())
}

async fn job_is_locked(id: i64, pool: &PgPool) -> anyhow::Result<bool> {
    let result = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT id FROM background_jobs WHERE id = $1 FOR UPDATE SKIP LOCKED",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(result.is_none())
}

// Database setup tests
#[tokio::test]
async fn test_setup_database_creates_tables() {
    let (pool, _container) = test_utils::setup_test_db_with_public_api().await.unwrap();

    // Verify basic tables exist
    let table_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.tables
         WHERE table_name IN ('background_jobs', 'archived_jobs')
         AND table_schema = 'public'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(
        table_count, 2,
        "Expected background_jobs and archived_jobs tables"
    );
}

#[tokio::test]
async fn jobs_are_locked_when_fetched() -> anyhow::Result<()> {
    #[derive(Clone)]
    struct TestContext {
        job_started_barrier: Arc<Barrier>,
        assertions_finished_barrier: Arc<Barrier>,
    }

    #[derive(Serialize, Deserialize)]
    struct TestJob;

    impl BackgroundJob for TestJob {
        const JOB_TYPE: &'static str = "test";
        type Context = TestContext;

        async fn run(&self, ctx: Self::Context) -> anyhow::Result<()> {
            ctx.job_started_barrier.wait().await;
            ctx.assertions_finished_barrier.wait().await;
            Ok(())
        }
    }

    let test_context = TestContext {
        job_started_barrier: Arc::new(Barrier::new(2)),
        assertions_finished_barrier: Arc::new(Barrier::new(2)),
    };

    let (pool, _container) = test_utils::setup_test_db().await?;

    let runner =
        test_utils::create_test_runner(pool.clone(), test_context.clone()).register::<TestJob>();

    let job_id = assert_some!(TestJob.enqueue(&pool).await?);

    assert!(job_exists(job_id, &pool).await?);
    assert!(!job_is_locked(job_id, &pool).await?);

    let runner = runner.start();
    test_context.job_started_barrier.wait().await;

    assert!(job_exists(job_id, &pool).await?);
    assert!(job_is_locked(job_id, &pool).await?);

    test_context.assertions_finished_barrier.wait().await;
    runner.wait_for_shutdown().await;

    assert!(!job_exists(job_id, &pool).await?);

    Ok(())
}

#[tokio::test]
async fn jobs_are_deleted_when_successfully_run() -> anyhow::Result<()> {
    #[derive(Serialize, Deserialize)]
    struct TestJob;

    impl BackgroundJob for TestJob {
        const JOB_TYPE: &'static str = "test";
        type Context = ();

        async fn run(&self, _ctx: Self::Context) -> anyhow::Result<()> {
            Ok(())
        }
    }

    async fn remaining_jobs(pool: &PgPool) -> anyhow::Result<i64> {
        let count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM background_jobs")
            .fetch_one(pool)
            .await?;
        Ok(count)
    }

    let (pool, _container) = test_utils::setup_test_db().await?;

    let runner = test_utils::create_test_runner(pool.clone(), ()).register::<TestJob>();

    assert_eq!(remaining_jobs(&pool).await?, 0);

    TestJob.enqueue(&pool).await?;
    assert_eq!(remaining_jobs(&pool).await?, 1);

    let runner = runner.start();
    runner.wait_for_shutdown().await;
    assert_eq!(remaining_jobs(&pool).await?, 0);

    Ok(())
}

#[tokio::test]
async fn failed_jobs_do_not_release_lock_before_updating_retry_time() -> anyhow::Result<()> {
    #[derive(Clone)]
    struct TestContext {
        job_started_barrier: Arc<Barrier>,
    }

    #[derive(Serialize, Deserialize)]
    struct TestJob;

    impl BackgroundJob for TestJob {
        const JOB_TYPE: &'static str = "test";
        type Context = TestContext;

        async fn run(&self, ctx: Self::Context) -> anyhow::Result<()> {
            ctx.job_started_barrier.wait().await;
            panic!();
        }
    }

    let test_context = TestContext {
        job_started_barrier: Arc::new(Barrier::new(2)),
    };

    let (pool, _container) = test_utils::setup_test_db().await?;

    let runner =
        test_utils::create_test_runner(pool.clone(), test_context.clone()).register::<TestJob>();

    TestJob.enqueue(&pool).await?;

    let runner = runner.start();
    test_context.job_started_barrier.wait().await;

    // `SKIP LOCKED` is intentionally omitted here, so we block until
    // the lock on the first job is released.
    // If there is any point where the row is unlocked, but the retry
    // count is not updated, we will get a row here.
    let available_jobs =
        sqlx::query_scalar::<_, i64>("SELECT id FROM background_jobs WHERE retries = 0 FOR UPDATE")
            .fetch_all(&pool)
            .await?;
    assert_eq!(available_jobs.len(), 0);

    // Sanity check to make sure the job actually is there
    let total_jobs_including_failed =
        sqlx::query_scalar::<_, i64>("SELECT id FROM background_jobs FOR UPDATE")
            .fetch_all(&pool)
            .await?;
    assert_eq!(total_jobs_including_failed.len(), 1);

    runner.wait_for_shutdown().await;

    Ok(())
}

#[tokio::test]
async fn panicking_in_jobs_updates_retry_counter() -> anyhow::Result<()> {
    #[derive(Serialize, Deserialize)]
    struct TestJob;

    impl BackgroundJob for TestJob {
        const JOB_TYPE: &'static str = "test";
        type Context = ();

        async fn run(&self, _ctx: Self::Context) -> anyhow::Result<()> {
            panic!()
        }
    }

    let (pool, _container) = test_utils::setup_test_db().await?;

    let runner = test_utils::create_test_runner(pool.clone(), ()).register::<TestJob>();

    let job_id = assert_some!(TestJob.enqueue(&pool).await?);

    let runner = runner.start();
    runner.wait_for_shutdown().await;

    let tries = sqlx::query_scalar::<_, i32>(
        "SELECT retries FROM background_jobs WHERE id = $1 FOR UPDATE",
    )
    .bind(job_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(tries, 1);

    Ok(())
}

#[tokio::test]
async fn jobs_can_be_deduplicated() -> anyhow::Result<()> {
    #[derive(Clone)]
    struct TestContext {
        runs: Arc<AtomicU8>,
        job_started_barrier: Arc<Barrier>,
        assertions_finished_barrier: Arc<Barrier>,
    }

    #[derive(Serialize, Deserialize)]
    struct TestJob {
        value: String,
    }

    impl TestJob {
        fn new(value: impl Into<String>) -> Self {
            let value = value.into();
            Self { value }
        }
    }

    impl BackgroundJob for TestJob {
        const JOB_TYPE: &'static str = "test";
        const DEDUPLICATED: bool = true;
        type Context = TestContext;

        async fn run(&self, ctx: Self::Context) -> anyhow::Result<()> {
            let runs = ctx.runs.fetch_add(1, Ordering::SeqCst);
            if runs == 0 {
                ctx.job_started_barrier.wait().await;
                ctx.assertions_finished_barrier.wait().await;
            }
            Ok(())
        }
    }

    let test_context = TestContext {
        runs: Arc::new(AtomicU8::new(0)),
        job_started_barrier: Arc::new(Barrier::new(2)),
        assertions_finished_barrier: Arc::new(Barrier::new(2)),
    };

    let (pool, _container) = test_utils::setup_test_db().await?;

    let runner = Runner::new(pool.clone(), test_context.clone())
        .register::<TestJob>()
        .shutdown_when_queue_empty();

    // Enqueue first job
    assert_some!(TestJob::new("foo").enqueue(&pool).await?);
    assert_compact_json_snapshot!(all_jobs(&pool).await?, @r#"[["test", {"value": "foo"}]]"#);

    // Try to enqueue the same job again, which should be deduplicated
    assert_none!(TestJob::new("foo").enqueue(&pool).await?);
    assert_compact_json_snapshot!(all_jobs(&pool).await?, @r#"[["test", {"value": "foo"}]]"#);

    // Start processing the first job
    let runner = runner.start();
    test_context.job_started_barrier.wait().await;

    // Enqueue the same job again, which should NOT be deduplicated,
    // since the first job already still running
    assert_some!(TestJob::new("foo").enqueue(&pool).await?);
    assert_compact_json_snapshot!(all_jobs(&pool).await?, @r#"[["test", {"value": "foo"}], ["test", {"value": "foo"}]]"#);

    // Try to enqueue the same job again, which should be deduplicated again
    assert_none!(TestJob::new("foo").enqueue(&pool).await?);
    assert_compact_json_snapshot!(all_jobs(&pool).await?, @r#"[["test", {"value": "foo"}], ["test", {"value": "foo"}]]"#);

    // Enqueue the same job but with different data, which should
    // NOT be deduplicated
    assert_some!(TestJob::new("bar").enqueue(&pool).await?);
    assert_compact_json_snapshot!(all_jobs(&pool).await?, @r#"[["test", {"value": "foo"}], ["test", {"value": "foo"}], ["test", {"value": "bar"}]]"#);

    // Resolve the final barrier to finish the test
    test_context.assertions_finished_barrier.wait().await;
    runner.wait_for_shutdown().await;

    Ok(())
}

#[tokio::test]
async fn jitter_configuration_affects_polling() -> anyhow::Result<()> {
    use std::time::Duration;
    use workers::Runner;

    #[derive(Serialize, Deserialize)]
    struct TestJob;

    impl BackgroundJob for TestJob {
        const JOB_TYPE: &'static str = "jitter_test";
        type Context = ();

        async fn run(&self, _ctx: Self::Context) -> anyhow::Result<()> {
            Ok(())
        }
    }

    let (pool, _container) = test_utils::setup_test_db().await?;

    // Test that jitter configuration is accepted and compiles
    let runner = Runner::new(pool.clone(), ())
        .register_with::<TestJob, _>(|queue| {
            queue
                .num_workers(1)
                .poll_interval(Duration::from_millis(100))
                .jitter(Duration::from_millis(50)) // Add up to 50ms jitter
        })
        .shutdown_when_queue_empty();

    // No jobs in queue, so the worker will immediately shut down
    let runner_handle = runner.start();
    runner_handle.wait_for_shutdown().await;

    // Test passes if jitter configuration is accepted and runs without error
    Ok(())
}

#[tokio::test]
async fn archive_functionality_works() -> anyhow::Result<()> {
    #[derive(Serialize, Deserialize)]
    struct TestJob {
        message: String,
    }

    impl BackgroundJob for TestJob {
        const JOB_TYPE: &'static str = "test";
        type Context = ();

        async fn run(&self, _ctx: Self::Context) -> anyhow::Result<()> {
            // Job does nothing but succeed
            Ok(())
        }
    }

    async fn remaining_jobs(pool: &PgPool) -> anyhow::Result<i64> {
        let count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM background_jobs")
            .fetch_one(pool)
            .await?;
        Ok(count)
    }

    let (pool, _container) = test_utils::setup_test_db().await?;

    // Configure runner with archiving enabled
    let runner = Runner::new(pool.clone(), ())
        .register::<TestJob>()
        .configure_default_queue(|queue| {
            queue.num_workers(1).archive_completed_jobs(true) // Enable archiving
        })
        .shutdown_when_queue_empty();

    // Enqueue a test job
    let job = TestJob {
        message: "test message".to_string(),
    };
    job.enqueue(&pool).await?;

    // Check that we have 1 job in the queue and 0 in archive initially
    assert_eq!(remaining_jobs(&pool).await?, 1);
    assert_eq!(archived_job_count(&pool).await?, 0);

    // Run the workers
    let runner_handle = runner.start();
    runner_handle.wait_for_shutdown().await;

    // After processing, job should be archived (not deleted)
    assert_eq!(remaining_jobs(&pool).await?, 0); // No jobs left in queue
    assert_eq!(archived_job_count(&pool).await?, 1); // 1 job in archive

    // Verify we can retrieve the archived job
    let archived_jobs = get_archived_jobs(
        &pool,
        ArchiveQuery::Filter {
            job_filter: Some("test".to_string()),
            limit: None,
        },
    )
    .await?;
    assert_eq!(archived_jobs.len(), 1);
    assert_eq!(archived_jobs[0].job.job_type, "test");

    Ok(())
}

#[tokio::test]
async fn test_enqueue_batch_simple_jobs() -> anyhow::Result<()> {
    #[derive(Serialize, Deserialize)]
    struct TestJob {
        message: String,
        number: u32,
    }

    impl BackgroundJob for TestJob {
        const JOB_TYPE: &'static str = "test_batch";
        type Context = ();

        async fn run(&self, _ctx: Self::Context) -> anyhow::Result<()> {
            Ok(())
        }
    }

    let (pool, _container) = test_utils::setup_test_db().await?;

    // Create batch of jobs
    let jobs = vec![
        TestJob {
            message: "first".to_string(),
            number: 1,
        },
        TestJob {
            message: "second".to_string(),
            number: 2,
        },
        TestJob {
            message: "third".to_string(),
            number: 3,
        },
    ];

    // Enqueue batch
    let ids = TestJob::enqueue_batch(&jobs, &pool).await?;

    // Should get back 3 job IDs
    assert_eq!(ids.len(), 3);
    assert!(ids.iter().all(Option::is_some));

    // Verify jobs are in database
    let job_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM background_jobs WHERE job_type = $1")
            .bind("test_batch")
            .fetch_one(&pool)
            .await?;
    assert_eq!(job_count, 3);

    Ok(())
}

#[tokio::test]
async fn test_enqueue_batch_empty_jobs() -> anyhow::Result<()> {
    #[derive(Serialize, Deserialize)]
    struct TestJob {
        message: String,
    }

    impl BackgroundJob for TestJob {
        const JOB_TYPE: &'static str = "test_empty_batch";
        type Context = ();

        async fn run(&self, _ctx: Self::Context) -> anyhow::Result<()> {
            Ok(())
        }
    }

    let (pool, _container) = test_utils::setup_test_db().await?;

    // Enqueue empty batch
    let ids = TestJob::enqueue_batch(&[], &pool).await?;

    // Should get back empty vector
    assert_eq!(ids.len(), 0);

    Ok(())
}

#[tokio::test]
async fn test_enqueue_batch_deduplicated_jobs() -> anyhow::Result<()> {
    #[derive(Serialize, Deserialize)]
    struct TestJob {
        message: String,
    }

    impl BackgroundJob for TestJob {
        const JOB_TYPE: &'static str = "test_dedup_batch";
        const DEDUPLICATED: bool = true;
        type Context = ();

        async fn run(&self, _ctx: Self::Context) -> anyhow::Result<()> {
            Ok(())
        }
    }

    let (pool, _container) = test_utils::setup_test_db().await?;

    // Create batch with duplicate jobs
    let jobs = vec![
        TestJob {
            message: "unique1".to_string(),
        },
        TestJob {
            message: "unique2".to_string(),
        },
        TestJob {
            message: "unique1".to_string(),
        }, // Duplicate
        TestJob {
            message: "unique3".to_string(),
        },
        TestJob {
            message: "unique2".to_string(),
        }, // Duplicate
    ];

    // Enqueue batch
    let ids = TestJob::enqueue_batch(&jobs, &pool).await?;

    // Should get back 5 results (some None for duplicates)
    assert_eq!(ids.len(), 5);

    // First occurrence of each unique job should be enqueued
    assert!(ids[0].is_some()); // unique1
    assert!(ids[1].is_some()); // unique2
    assert!(ids[2].is_none()); // duplicate unique1
    assert!(ids[3].is_some()); // unique3
    assert!(ids[4].is_none()); // duplicate unique2

    // Verify only 3 unique jobs are in database
    let job_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM background_jobs WHERE job_type = $1")
            .bind("test_dedup_batch")
            .fetch_one(&pool)
            .await?;
    assert_eq!(job_count, 3);

    Ok(())
}

#[tokio::test]
async fn test_batch_enqueue_with_different_priorities() -> anyhow::Result<()> {
    #[derive(Serialize, Deserialize)]
    struct HighPriorityJob {
        message: String,
    }

    #[derive(Serialize, Deserialize)]
    struct LowPriorityJob {
        message: String,
    }

    impl BackgroundJob for HighPriorityJob {
        const JOB_TYPE: &'static str = "high_priority_batch";
        const PRIORITY: i16 = 100;
        type Context = ();

        async fn run(&self, _ctx: Self::Context) -> anyhow::Result<()> {
            Ok(())
        }
    }

    impl BackgroundJob for LowPriorityJob {
        const JOB_TYPE: &'static str = "low_priority_batch";
        const PRIORITY: i16 = 1;
        type Context = ();

        async fn run(&self, _ctx: Self::Context) -> anyhow::Result<()> {
            Ok(())
        }
    }

    let (pool, _container) = test_utils::setup_test_db().await?;

    // Enqueue high priority jobs
    let high_jobs = vec![
        HighPriorityJob {
            message: "urgent1".to_string(),
        },
        HighPriorityJob {
            message: "urgent2".to_string(),
        },
    ];
    let high_ids = HighPriorityJob::enqueue_batch(&high_jobs, &pool).await?;

    // Enqueue low priority jobs
    let low_jobs = vec![
        LowPriorityJob {
            message: "normal1".to_string(),
        },
        LowPriorityJob {
            message: "normal2".to_string(),
        },
    ];
    let low_ids = LowPriorityJob::enqueue_batch(&low_jobs, &pool).await?;

    assert_eq!(high_ids.len(), 2);
    assert_eq!(low_ids.len(), 2);

    // Verify priorities are set correctly
    let high_priorities: Vec<i16> =
        sqlx::query_scalar("SELECT priority FROM background_jobs WHERE job_type = $1 ORDER BY id")
            .bind("high_priority_batch")
            .fetch_all(&pool)
            .await?;
    assert!(high_priorities.iter().all(|&p| p == 100));

    let low_priorities: Vec<i16> =
        sqlx::query_scalar("SELECT priority FROM background_jobs WHERE job_type = $1 ORDER BY id")
            .bind("low_priority_batch")
            .fetch_all(&pool)
            .await?;
    assert!(low_priorities.iter().all(|&p| p == 1));

    Ok(())
}
