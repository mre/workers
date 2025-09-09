#![allow(missing_docs)]
#![allow(clippy::expect_used)]
#![allow(clippy::panic)]
#![allow(clippy::unwrap_used)]
#![allow(clippy::indexing_slicing)]

use claims::{assert_none, assert_some};
use diesel::prelude::*;
use diesel_async::pooled_connection::AsyncDieselConnectionManager;
use diesel_async::pooled_connection::deadpool::Pool;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use insta::assert_compact_json_snapshot;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use tokio::sync::Barrier;
use workers::schema::background_jobs;
use workers::{BackgroundJob, Runner};

/// Test utilities and common setup
mod test_utils {
    use super::*;

    /// Create a connection pool for testing
    pub(super) fn create_pool(database_url: &str) -> anyhow::Result<Pool<AsyncPgConnection>> {
        let manager = AsyncDieselConnectionManager::<AsyncPgConnection>::new(database_url);
        Ok(Pool::builder(manager).max_size(4).build()?)
    }

    /// Create a test runner with common configuration
    pub(super) fn create_test_runner<Context: Clone + Send + Sync + 'static>(
        pool: Pool<AsyncPgConnection>,
        context: Context,
    ) -> Runner<Context> {
        Runner::new(pool, context)
            .configure_default_queue(|queue| queue.num_workers(2))
            .shutdown_when_queue_empty()
    }

    /// Get the database URL from environment, panicking with helpful message if not found
    pub(super) fn get_test_database_url() -> String {
        std::env::var("DATABASE_URL")
            .expect("DATABASE_URL environment variable must be set to run integration tests")
    }
}

async fn all_jobs(conn: &mut AsyncPgConnection) -> QueryResult<Vec<(String, Value)>> {
    background_jobs::table
        .select((background_jobs::job_type, background_jobs::data))
        .get_results(conn)
        .await
}

async fn job_exists(id: i64, conn: &mut AsyncPgConnection) -> QueryResult<bool> {
    Ok(background_jobs::table
        .find(id)
        .select(background_jobs::id)
        .get_result::<i64>(conn)
        .await
        .optional()?
        .is_some())
}

async fn job_is_locked(id: i64, conn: &mut AsyncPgConnection) -> QueryResult<bool> {
    Ok(background_jobs::table
        .find(id)
        .select(background_jobs::id)
        .for_update()
        .skip_locked()
        .get_result::<i64>(conn)
        .await
        .optional()?
        .is_none())
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
        const JOB_NAME: &'static str = "test";
        type Context = TestContext;

        async fn run(&self, ctx: Self::Context) -> anyhow::Result<()> {
            ctx.job_started_barrier.wait().await;
            ctx.assertions_finished_barrier.wait().await;
            Ok(())
        }
    }

    let database_url = test_utils::get_test_database_url();

    let test_context = TestContext {
        job_started_barrier: Arc::new(Barrier::new(2)),
        assertions_finished_barrier: Arc::new(Barrier::new(2)),
    };

    let pool = test_utils::create_pool(&database_url)?;
    let mut conn = pool.get().await?;

    let runner =
        test_utils::create_test_runner(pool, test_context.clone()).register_job_type::<TestJob>();

    let job_id = assert_some!(TestJob.enqueue(&mut conn).await?);

    assert!(job_exists(job_id, &mut conn).await?);
    assert!(!job_is_locked(job_id, &mut conn).await?);

    let runner = runner.start();
    test_context.job_started_barrier.wait().await;

    assert!(job_exists(job_id, &mut conn).await?);
    assert!(job_is_locked(job_id, &mut conn).await?);

    test_context.assertions_finished_barrier.wait().await;
    runner.wait_for_shutdown().await;

    assert!(!job_exists(job_id, &mut conn).await?);

    Ok(())
}

#[tokio::test]
async fn jobs_are_deleted_when_successfully_run() -> anyhow::Result<()> {
    #[derive(Serialize, Deserialize)]
    struct TestJob;

    impl BackgroundJob for TestJob {
        const JOB_NAME: &'static str = "test";
        type Context = ();

        async fn run(&self, _ctx: Self::Context) -> anyhow::Result<()> {
            Ok(())
        }
    }

    async fn remaining_jobs(conn: &mut AsyncPgConnection) -> QueryResult<i64> {
        background_jobs::table.count().get_result(conn).await
    }

    let database_url = test_utils::get_test_database_url();

    let pool = test_utils::create_pool(&database_url)?;
    let mut conn = pool.get().await?;

    let runner = test_utils::create_test_runner(pool, ()).register_job_type::<TestJob>();

    assert_eq!(remaining_jobs(&mut conn).await?, 0);

    TestJob.enqueue(&mut conn).await?;
    assert_eq!(remaining_jobs(&mut conn).await?, 1);

    let runner = runner.start();
    runner.wait_for_shutdown().await;
    assert_eq!(remaining_jobs(&mut conn).await?, 0);

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
        const JOB_NAME: &'static str = "test";
        type Context = TestContext;

        async fn run(&self, ctx: Self::Context) -> anyhow::Result<()> {
            ctx.job_started_barrier.wait().await;
            panic!();
        }
    }

    let database_url = test_utils::get_test_database_url();

    let test_context = TestContext {
        job_started_barrier: Arc::new(Barrier::new(2)),
    };

    let pool = test_utils::create_pool(&database_url)?;
    let mut conn = pool.get().await?;

    let runner =
        test_utils::create_test_runner(pool, test_context.clone()).register_job_type::<TestJob>();

    TestJob.enqueue(&mut conn).await?;

    let runner = runner.start();
    test_context.job_started_barrier.wait().await;

    // `SKIP LOCKED` is intentionally omitted here, so we block until
    // the lock on the first job is released.
    // If there is any point where the row is unlocked, but the retry
    // count is not updated, we will get a row here.
    let available_jobs = background_jobs::table
        .select(background_jobs::id)
        .filter(background_jobs::retries.eq(0))
        .for_update()
        .load::<i64>(&mut conn)
        .await?;
    assert_eq!(available_jobs.len(), 0);

    // Sanity check to make sure the job actually is there
    let total_jobs_including_failed = background_jobs::table
        .select(background_jobs::id)
        .for_update()
        .load::<i64>(&mut conn)
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
        const JOB_NAME: &'static str = "test";
        type Context = ();

        async fn run(&self, _ctx: Self::Context) -> anyhow::Result<()> {
            panic!()
        }
    }

    let database_url = test_utils::get_test_database_url();

    let pool = test_utils::create_pool(&database_url)?;
    let mut conn = pool.get().await?;

    let runner = test_utils::create_test_runner(pool, ()).register_job_type::<TestJob>();

    let job_id = assert_some!(TestJob.enqueue(&mut conn).await?);

    let runner = runner.start();
    runner.wait_for_shutdown().await;

    let tries = background_jobs::table
        .find(job_id)
        .select(background_jobs::retries)
        .for_update()
        .first::<i32>(&mut conn)
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
        const JOB_NAME: &'static str = "test";
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

    let database_url = test_utils::get_test_database_url();

    let test_context = TestContext {
        runs: Arc::new(AtomicU8::new(0)),
        job_started_barrier: Arc::new(Barrier::new(2)),
        assertions_finished_barrier: Arc::new(Barrier::new(2)),
    };

    let pool = test_utils::create_pool(&database_url)?;
    let mut conn = pool.get().await?;

    let runner = Runner::new(pool, test_context.clone())
        .register_job_type::<TestJob>()
        .shutdown_when_queue_empty();

    // Enqueue first job
    assert_some!(TestJob::new("foo").enqueue(&mut conn).await?);
    assert_compact_json_snapshot!(all_jobs(&mut conn).await?, @r#"[["test", {"value": "foo"}]]"#);

    // Try to enqueue the same job again, which should be deduplicated
    assert_none!(TestJob::new("foo").enqueue(&mut conn).await?);
    assert_compact_json_snapshot!(all_jobs(&mut conn).await?, @r#"[["test", {"value": "foo"}]]"#);

    // Start processing the first job
    let runner = runner.start();
    test_context.job_started_barrier.wait().await;

    // Enqueue the same job again, which should NOT be deduplicated,
    // since the first job already still running
    assert_some!(TestJob::new("foo").enqueue(&mut conn).await?);
    assert_compact_json_snapshot!(all_jobs(&mut conn).await?, @r#"[["test", {"value": "foo"}], ["test", {"value": "foo"}]]"#);

    // Try to enqueue the same job again, which should be deduplicated again
    assert_none!(TestJob::new("foo").enqueue(&mut conn).await?);
    assert_compact_json_snapshot!(all_jobs(&mut conn).await?, @r#"[["test", {"value": "foo"}], ["test", {"value": "foo"}]]"#);

    // Enqueue the same job but with different data, which should
    // NOT be deduplicated
    assert_some!(TestJob::new("bar").enqueue(&mut conn).await?);
    assert_compact_json_snapshot!(all_jobs(&mut conn).await?, @r#"[["test", {"value": "foo"}], ["test", {"value": "foo"}], ["test", {"value": "bar"}]]"#);

    // Resolve the final barrier to finish the test
    test_context.assertions_finished_barrier.wait().await;
    runner.wait_for_shutdown().await;

    Ok(())
}
