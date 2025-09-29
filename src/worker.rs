use crate::job_registry::JobRegistry;
use crate::storage;
use crate::util::{try_to_extract_panic_info, with_sentry_transaction};
use anyhow::anyhow;
use futures_util::FutureExt;
use rand::Rng;
use sentry_core::{Hub, SentryFutureExt};
use sqlx::PgPool;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{Instrument, debug, error, info_span, trace, warn};

pub(crate) struct Worker<Context> {
    pub(crate) connection_pool: PgPool,
    pub(crate) context: Context,
    pub(crate) job_registry: Arc<JobRegistry<Context>>,
    pub(crate) shutdown_when_queue_empty: bool,
    pub(crate) poll_interval: Duration,
    pub(crate) jitter: Duration,
    pub(crate) archive_completed_jobs: bool,
}

impl<Context: Clone + Send + Sync + 'static> Worker<Context> {
    /// Calculate the sleep duration with random jitter applied.
    fn sleep_duration_with_jitter(&self) -> Duration {
        if self.jitter.is_zero() {
            return self.poll_interval;
        }

        let jitter_millis = u64::try_from(self.jitter.as_millis()).unwrap_or(u64::MAX);
        let random_jitter = rand::thread_rng().gen_range(0..=jitter_millis);
        self.poll_interval + Duration::from_millis(random_jitter)
    }

    /// Run background jobs forever, or until the queue is empty if `shutdown_when_queue_empty` is set.
    #[allow(clippy::cognitive_complexity)]
    pub(crate) async fn run(&self) {
        loop {
            match self.run_next_job().await {
                Ok(Some(_)) => {}
                Ok(None) if self.shutdown_when_queue_empty => {
                    debug!("No pending background worker jobs found. Shutting down the worker…");
                    break;
                }
                Ok(None) => {
                    let sleep_duration = self.sleep_duration_with_jitter();
                    trace!(
                        "No pending background worker jobs found. Polling again in {sleep_duration:?}…",
                    );
                    sleep(sleep_duration).await;
                }
                Err(error) => {
                    error!("Failed to run job: {error}");
                    sleep(self.sleep_duration_with_jitter()).await;
                }
            }
        }
    }

    /// Run the next job in the queue, if there is one.
    ///
    /// Returns:
    /// - `Ok(Some(job_id))` if a job was run
    /// - `Ok(None)` if no jobs were waiting
    /// - `Err(...)` if there was an error retrieving the job
    #[allow(clippy::cognitive_complexity)]
    async fn run_next_job(&self) -> anyhow::Result<Option<i64>> {
        let context = self.context.clone();
        let job_registry = self.job_registry.clone();
        let pool = &self.connection_pool;

        let job_types = job_registry.job_types();

        trace!("Looking for next background worker job…");

        // Start a transaction to hold the job lock during execution
        let mut tx = pool.begin().await?;

        let job = match storage::find_next_unlocked_job_tx(&mut tx, &job_types).await {
            Ok(job) => job,
            Err(sqlx::Error::RowNotFound) => {
                tx.rollback().await?;
                return Ok(None);
            }
            Err(e) => {
                tx.rollback().await?;
                return Err(e.into());
            }
        };

        let span = info_span!("job", job.id = %job.id, job.type = %job.job_type);

        let job_id = job.id;
        debug!("Running job…");

        let future = with_sentry_transaction(&job.job_type, async || {
            let run_task_fn = job_registry
                .get(&job.job_type)
                .ok_or_else(|| anyhow!("Unknown job type {}", job.job_type))?;

            AssertUnwindSafe(run_task_fn(context, job.data))
                .catch_unwind()
                .await
                .map_err(|e| try_to_extract_panic_info(&*e))
                // TODO: Replace with flatten() once that stabilizes
                .and_then(std::convert::identity)
        });

        let result = future
            .instrument(span.clone())
            .bind_hub(Hub::current())
            .await;

        let _enter = span.enter();
        match result {
            Ok(()) => {
                if self.archive_completed_jobs {
                    debug!("Archiving successful job…");
                    storage::archive_successful_job(&mut tx, job_id).await?;
                } else {
                    debug!("Deleting successful job…");
                    storage::delete_successful_job(&mut tx, job_id).await?;
                }
                tx.commit().await?;
            }
            Err(error) => {
                warn!("Failed to run job: {error}");
                storage::update_failed_job(&mut tx, job_id).await?;
                tx.commit().await?;
            }
        }

        Ok(Some(job_id))
    }
}
