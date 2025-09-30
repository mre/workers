use std::{collections::HashMap, marker::PhantomData, time::Duration};
use tracing::{debug, error, info, instrument};

use sqlx::PgPool;
use tokio::task::AbortHandle;

use crate::BackgroundJob;

#[derive(Clone, Copy, Debug)]
pub struct Configured;

#[derive(Clone, Copy, Debug)]
pub struct Unconfigured;

type JobType = String;

/// The `ArchiveCleanerBuilder` prepares an `ArchiveCleaner` that is in charge of cleaning up various archived jobs of given types
/// Uses typestate to ensure you cannot start a cleaner that will do nothing
#[derive(Debug)]
pub struct ArchiveCleanerBuilder<State = Unconfigured> {
    configurations: HashMap<JobType, CleanupConfiguration>,
    _state: PhantomData<State>,
}

impl Default for ArchiveCleanerBuilder<Unconfigured> {
    fn default() -> Self {
        Self::new()
    }
}

/// How to clean up archived entries
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CleanupPolicy {
    /// Keep all entries newer than `now - Duration`
    MaxAge(chrono::Duration),
    /// Keep at most N entries
    MaxCount(usize),
    /// Discard old entries based on age, but keep at least some
    Retain {
        /// Maximum age of an entry to keep
        max_age: chrono::Duration,
        /// Minimum number of entries to always preserve
        keep_at_least: usize,
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

impl ArchiveCleanerBuilder {
    /// Create a new, unconfigured, `ArchiveCleaner`
    pub fn new() -> Self {
        Self {
            configurations: HashMap::new(),
            _state: PhantomData,
        }
    }
}

impl<State> ArchiveCleanerBuilder<State> {
    /// Configure the cleaner for a specific job type
    pub fn configure<J: BackgroundJob>(
        mut self,
        configuration: CleanupConfiguration,
    ) -> ArchiveCleanerBuilder<Configured> {
        debug!(
            job_type = J::JOB_NAME,
            cleanup_every = ?configuration.cleanup_every,
            policy = ?configuration.policy,
            "Configuring archive cleaner for job type"
        );

        self.configurations
            .insert(J::JOB_NAME.to_owned(), configuration);

        ArchiveCleanerBuilder {
            configurations: self.configurations,
            _state: PhantomData,
        }
    }
}

impl ArchiveCleanerBuilder<Configured> {
    /// Start the cleaner, spawning a `tokio::task::Task` for each configured job type
    /// Returns the `ArchiveCleaner` which holds the tasks
    pub fn run(self, pool: &PgPool) -> ArchiveCleaner {
        info!(
            job_types = ?self.configurations.keys().collect::<Vec<_>>(),
            num_configured = self.configurations.len(),
            "Starting archive cleaner with configured job types"
        );

        let handles = self
            .configurations
            .into_iter()
            .map(|(job_type, config)| {
                tokio::task::spawn(ArchiveCleaner::spawn_cleaner(
                    job_type,
                    config,
                    pool.clone(),
                ))
                .abort_handle()
            })
            .collect();
        ArchiveCleaner { handles }
    }
}

/// The `ArchiveCleaner` holds the individual cleaner tasks
#[derive(Debug)]
pub struct ArchiveCleaner {
    /// Handles to the spawned tasks
    handles: Vec<AbortHandle>,
}

impl ArchiveCleaner {
    /// Stop all cleaner tasks
    pub fn stop(self) {
        self.handles.into_iter().for_each(|h| h.abort());
    }

    #[instrument(skip(config, pool))]
    async fn spawn_cleaner(job_type: JobType, config: CleanupConfiguration, pool: PgPool) {
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
                    debug!(max_count = count, "Executing MaxCount cleanup policy");
                    sqlx::query(&format!(
                            r"DELETE FROM archived_jobs WHERE job_type = $1
                             AND archived_at < (SELECT archived_at FROM archived_jobs WHERE job_type = $1
                                                ORDER BY archived_at DESC OFFSET {offset} LIMIT 1)",
                            offset = count.saturating_sub(1)
                        ))
                        .bind(&job_type)
                        .execute(&pool)
                        .await
                }
                CleanupPolicy::Retain {
                    max_age,
                    keep_at_least,
                } => {
                    debug!(
                        max_age = ?max_age,
                        keep_at_least = keep_at_least,
                        "Executing Retain cleanup policy"
                    );
                    sqlx::query(&format!(
                        r"DELETE FROM archived_jobs WHERE job_type = $1 AND
                          archived_at < (NOW() - $2) AND
                          archived_at < (
                              SELECT archived_at FROM archived_jobs WHERE job_type = $1
                              ORDER BY archived_at DESC OFFSET {offset} LIMIT 1
                          )",
                        offset = keep_at_least.saturating_sub(1)
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
                }
            }
        }
    }
}
