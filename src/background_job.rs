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
    /// Higher values indicate higher priority and will be processed first.
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

    /// Enqueue multiple jobs of the same type for background execution in a single transaction.
    ///
    /// This is more efficient than calling `enqueue()` multiple times as it uses a single
    /// database transaction and bulk insert operations with `PostgreSQL` arrays.
    ///
    /// Returns a vector of job IDs. For deduplicated jobs, `None` is returned for jobs
    /// that were skipped due to deduplication.
    ///
    /// # Implementation Notes
    ///
    /// - Uses `PostgreSQL` arrays with UNNEST for bulk insert
    /// - Handles deduplication with conditional inserts
    /// - Single transaction for atomicity
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let jobs = vec![
    ///     SendEmailJob { to: "a@example.com", subject: "Hello" },
    ///     SendEmailJob { to: "b@example.com", subject: "Hi" },
    /// ];
    /// let ids = SendEmailJob::enqueue_batch(&jobs, &pool).await?;
    /// ```
    #[instrument(name = "workers.enqueue_batch", skip(jobs, pool), fields(message = Self::JOB_NAME, count = jobs.len()))]
    fn enqueue_batch<'a>(
        jobs: &'a [Self],
        pool: &'a PgPool,
    ) -> BoxFuture<'a, Result<Vec<Option<i64>>, EnqueueError>> {
        if jobs.is_empty() {
            return async move { Ok(Vec::new()) }.boxed();
        }

        // Serialize all jobs first
        let mut job_data = Vec::with_capacity(jobs.len());
        for job in jobs {
            match serde_json::to_value(job) {
                Ok(data) => job_data.push(data),
                Err(err) => {
                    return async move { Err(EnqueueError::SerializationError(err)) }.boxed();
                }
            }
        }

        let job_type = Self::JOB_NAME;
        let priority = Self::PRIORITY;
        let is_deduplicated = Self::DEDUPLICATED;

        if is_deduplicated {
            let future = enqueue_batch_deduplicated(pool, job_type, job_data, priority);
            future.boxed()
        } else {
            let future = enqueue_batch_simple(pool, job_type, job_data, priority);
            async move {
                let ids = future.await?;
                Ok(ids.into_iter().map(Some).collect())
            }
            .boxed()
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

fn enqueue_batch_simple<'a>(
    pool: &'a PgPool,
    job_type: &'a str,
    job_data: Vec<Value>,
    priority: i16,
) -> BoxFuture<'a, Result<Vec<i64>, EnqueueError>> {
    async move {
        // Use PostgreSQL arrays with UNNEST for bulk insert
        let ids = sqlx::query_scalar::<_, i64>(
            r"
            INSERT INTO background_jobs (job_type, data, priority)
            SELECT $1, unnest($2::jsonb[]), $3
            RETURNING id
            ",
        )
        .bind(job_type)
        .bind(job_data)
        .bind(priority)
        .fetch_all(pool)
        .await?;

        Ok(ids)
    }
    .boxed()
}

fn enqueue_batch_deduplicated<'a>(
    pool: &'a PgPool,
    job_type: &'a str,
    job_data: Vec<Value>,
    priority: i16,
) -> BoxFuture<'a, Result<Vec<Option<i64>>, EnqueueError>> {
    async move {
        // For deduplication, we need to handle each job individually within a transaction
        // to check for existing jobs and return the correct Option<i64> for each position
        let mut tx = pool.begin().await?;
        let mut result_ids = Vec::with_capacity(job_data.len());

        for data in job_data {
            let id = sqlx::query_scalar::<_, Option<i64>>(
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
            .bind(&data)
            .bind(priority)
            .fetch_optional(&mut *tx)
            .await?;

            result_ids.push(id.flatten());
        }

        tx.commit().await?;
        Ok(result_ids)
    }
    .boxed()
}
