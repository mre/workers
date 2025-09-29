use std::{collections::HashMap, marker::PhantomData, time::Duration};
use tracing::{Instrument, debug, error, info};

use sqlx::PgPool;
use tokio::task::JoinSet;

use crate::BackgroundJob;

#[derive(Clone, Copy, Debug)]
pub struct Configured;

#[derive(Clone, Copy, Debug)]
pub struct Unconfigured;

type JobType = String;

/// The `ArchiveCleaner` spawns a thread that is in charge of cleaning up various archived jobs of given types
/// Uses typestate to ensure you cannot start a cleaner that will do nothing
#[derive(Debug)]
pub struct ArchiveCleaner<State = Unconfigured> {
    configurations: HashMap<JobType, CleanupConfiguration>,
    _state: PhantomData<State>,
}

impl Default for ArchiveCleaner<Unconfigured> {
    fn default() -> Self {
        Self::new()
    }
}

/// How to clean up archived entries
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CleanupPolicy {
    /// Keep all entries newer than `now - Duration`
    MaxAge(chrono::Duration),
    /// Keep at most n entries
    MaxCount(usize),
    /// Discard entries older than the `max_age` _and_ keep at most `max_count`
    Mixed {
        /// Maximum age of an entry to keep
        max_age: chrono::Duration,
        /// Maximum number of entries to keep
        max_count: usize,
    },
}

impl Default for CleanupPolicy {
    fn default() -> Self {
        Self::MaxAge(chrono::Duration::seconds(3600))
    }
}

/// Configuration for cleaning up archived entries
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CleanupConfiguration {
    /// Interval at which to run
    pub cleanup_every: Duration,
    /// How to go about cleaning the archived entries
    pub policy: CleanupPolicy,
}

impl Default for CleanupConfiguration {
    fn default() -> Self {
        Self {
            cleanup_every: Duration::from_secs(3600),
            policy: CleanupPolicy::default(),
        }
    }
}

impl ArchiveCleaner {
    /// Create a new, unconfigured, `ArchiveCleaner`
    pub fn new() -> Self {
        Self {
            configurations: HashMap::new(),
            _state: PhantomData,
        }
    }

    async fn spawn_cleaner(job_type: JobType, config: CleanupConfiguration, pool: PgPool) {
        let span = tracing::info_span!("archive_cleaner", job_type = %job_type);
        async move {
            info!(
                cleanup_every = ?config.cleanup_every,
                policy = ?config.policy,
                "Archive cleaner task started"
            );

            let mut ticker = tokio::time::interval(config.cleanup_every);

            loop {
                ticker.tick().await;

                debug!(
                    policy = ?config.policy,
                    "Starting cleanup cycle"
                );

                let result = match config.policy {
                    CleanupPolicy::MaxAge(max_age) => {
                        debug!(
                            max_age = ?max_age,
                            "Executing MaxAge cleanup policy"
                        );
                        sqlx::query(
                            "DELETE FROM archived_jobs WHERE job_type = $1 AND archived_at < (NOW() - $2)",
                        )
                        .bind(&job_type)
                        .bind(max_age)
                        .execute(&pool)
                        .await
                    }
                    CleanupPolicy::MaxCount(count) => {
                        debug!(
                            max_count = count,
                            "Executing MaxCount cleanup policy"
                        );
                        sqlx::query(&format!(
                            r"DELETE FROM archived_jobs WHERE job_type = $1
                             AND archived_at < (SELECT archived_at FROM archived_jobs WHERE job_type = $1
                                                ORDER BY archived_at DESC OFFSET {offset} LIMIT 1)",
                            offset = count - 1
                        ))
                        .bind(&job_type)
                        .execute(&pool)
                        .await
                    }
                    CleanupPolicy::Mixed { max_age, max_count } => {
                        debug!(
                            max_age = ?max_age,
                            max_count = max_count,
                            "Executing Mixed cleanup policy"
                        );
                        sqlx::query(&format!(
                            r"DELETE FROM archived_jobs WHERE job_type = $1 AND
                          (archived_at < (NOW() - $2) OR
                           archived_at < (SELECT archived_at FROM archived_jobs WHERE job_type = $1
                                          ORDER BY archived_at DESC OFFSET {offset} LIMIT 1))",
                            offset = max_count - 1
                        ))
                        .bind(&job_type)
                        .bind(max_age)
                        .execute(&pool)
                        .await
                    }
                };

                match result {
                    Ok(query_result) => {
                        let rows_affected = query_result.rows_affected();
                        if rows_affected > 0 {
                            info!(
                                rows_deleted = rows_affected,
                                policy = ?config.policy,
                                "Cleanup cycle completed successfully"
                            );
                        } else {
                            debug!("Cleanup cycle completed - no rows to delete");
                        }
                    }
                    Err(e) => {
                        error!(
                            error = %e,
                            policy = ?config.policy,
                            "Failed to clean archived jobs"
                        );
                        break;
                    }
                }
            }

            info!("Archive cleaner task exited");
        }
        .instrument(span)
        .await;
    }
}

impl<State> ArchiveCleaner<State> {
    /// Configure the cleaner for a specific job type
    pub fn configure<J: BackgroundJob<Context: Clone + Send + Sync + 'static>>(
        mut self,
        configuration: CleanupConfiguration,
    ) -> ArchiveCleaner<Configured> {
        debug!(
            job_type = J::JOB_NAME,
            cleanup_every = ?configuration.cleanup_every,
            policy = ?configuration.policy,
            "Configuring archive cleaner for job type"
        );

        self.configurations
            .insert(J::JOB_NAME.to_owned(), configuration);

        ArchiveCleaner {
            configurations: self.configurations,
            _state: PhantomData,
        }
    }
}

impl ArchiveCleaner<Configured> {
    /// Start the cleaner, spawning a `tokio::task::Task` for each configured job type
    /// Returns a `JoinSet` containing all spawned tasks for easy cancellation
    pub fn run(self, pool: &PgPool) -> JoinSet<()> {
        info!(
            job_types = ?self.configurations.keys().collect::<Vec<_>>(),
            num_configured = self.configurations.len(),
            "Starting archive cleaner with configured job types"
        );

        let mut set = JoinSet::new();
        for (job_type, config) in self.configurations {
            set.spawn(ArchiveCleaner::spawn_cleaner(
                job_type,
                config,
                pool.clone(),
            ));
        }
        set
    }
}
