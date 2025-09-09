use crate::errors::EnqueueError;
use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sqlx::PgPool;
use std::future::Future;
use tracing::instrument;

/// The default queue name used when no specific queue is specified.
pub const DEFAULT_QUEUE: &str = "default";

/// Trait for defining background jobs that can be enqueued and executed asynchronously.
pub trait BackgroundJob: Serialize + DeserializeOwned + Send + Sync + 'static {
    /// Unique name of the task.
    ///
    /// This MUST be unique for the whole application.
    const JOB_NAME: &'static str;

    /// Default priority of the task.
    ///
    /// [`Self::enqueue_with_priority`] can be used to override the priority value.
    const PRIORITY: i16 = 0;

    /// Whether the job should be deduplicated.
    ///
    /// If true, the job will not be enqueued if there is already an unstarted
    /// job with the same data.
    const DEDUPLICATED: bool = false;

    /// Job queue where this job will be executed.
    const QUEUE: &'static str = DEFAULT_QUEUE;

    /// The application data provided to this job at runtime.
    type Context: Clone + Send + 'static;

    /// Execute the task. This method should define its logic.
    fn run(&self, ctx: Self::Context) -> impl Future<Output = anyhow::Result<()>> + Send;

    /// Enqueue this job for background execution.
    ///
    /// Returns the job ID if successfully enqueued, or None if deduplicated.
    #[instrument(name = "workers.enqueue", skip(self, pool), fields(message = Self::JOB_NAME))]
    fn enqueue<'a>(&'a self, pool: &'a PgPool) -> BoxFuture<'a, Result<Option<i64>, EnqueueError>> {
        let data = match serde_json::to_value(self) {
            Ok(data) => data,
            Err(err) => return async move { Err(EnqueueError::SerializationError(err)) }.boxed(),
        };
        let priority = Self::PRIORITY;

        if Self::DEDUPLICATED {
            let future = enqueue_deduplicated(pool, Self::JOB_NAME, data, priority);
            future.boxed()
        } else {
            let future = enqueue_simple(pool, Self::JOB_NAME, data, priority);
            async move { Ok(Some(future.await?)) }.boxed()
        }
    }
}

fn enqueue_deduplicated<'a>(
    pool: &'a PgPool,
    job_type: &'a str,
    data: Value,
    priority: i16,
) -> BoxFuture<'a, Result<Option<i64>, EnqueueError>> {
    async move {
        // Try to insert only if no similar job exists (not locked)
        let result = sqlx::query_scalar::<_, Option<i64>>(
            r"
            INSERT INTO background_jobs (job_type, data, priority)
            SELECT $1, $2, $3
            WHERE NOT EXISTS (
                SELECT 1 FROM background_jobs
                WHERE job_type = $1 AND data = $2 AND priority = $3
                FOR UPDATE SKIP LOCKED
            )
            RETURNING id
            ",
        )
        .bind(job_type)
        .bind(data)
        .bind(priority)
        .fetch_optional(pool)
        .await?;

        Ok(result.flatten())
    }
    .boxed()
}

fn enqueue_simple<'a>(
    pool: &'a PgPool,
    job_type: &'a str,
    data: Value,
    priority: i16,
) -> BoxFuture<'a, Result<i64, EnqueueError>> {
    async move {
        let id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO background_jobs (job_type, data, priority) VALUES ($1, $2, $3) RETURNING id"
        )
        .bind(job_type)
        .bind(data)
        .bind(priority)
        .fetch_one(pool)
        .await?;

        Ok(id)
    }
    .boxed()
}
