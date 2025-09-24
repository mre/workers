use std::{
    collections::HashMap,
    marker::PhantomData,
    time::{Duration, Instant},
};

use sqlx::PgPool;
use tokio::task::AbortHandle;

struct Configured;
struct Unconfigured;

type JobType = String;

/// The `ArchiveCleaner` spawns a thread that is in charge of cleaning up various archived jobs of given types
/// Uses typestate to ensure you cannot start a cleaner that will do nothing
pub(crate) struct ArchiveCleaner<State = Unconfigured> {
    configurations: HashMap<JobType, CleanupConfiguration>,
    _state: PhantomData<State>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum CleanupPolicy {
    /// Keep all entries newer than `now - Duration`
    MaxAge(Duration),
    /// Keep at most n entries
    MaxCount(usize),
    /// Discard entries older than the `max_age` _and_ keep at most `max_count`
    Mixed { max_age: Duration, max_count: usize },
}

impl Default for CleanupPolicy {
    fn default() -> Self {
        Self::MaxAge(Duration::from_secs(3600))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CleanupConfiguration {
    /// Interval at which to run
    cleanup_every: Duration,
    /// How to go about cleaning the archived entries
    policy: CleanupPolicy,
}

impl Default for CleanupConfiguration {
    fn default() -> Self {
        Self {
            cleanup_every: Duration::from_secs(3600),
            policy: Default::default(),
        }
    }
}

impl ArchiveCleaner {
    pub fn new() -> ArchiveCleaner<Unconfigured> {
        Self {
            configurations: HashMap::new(),
            _state: PhantomData,
        }
    }
}

impl<State> ArchiveCleaner<State> {
    pub fn configure(
        mut self,
        job_type: String,
        configuration: CleanupConfiguration,
    ) -> ArchiveCleaner<Configured> {
        self.configurations.insert(job_type, configuration);

        ArchiveCleaner {
            configurations: self.configurations,
            _state: PhantomData,
        }
    }
}

impl ArchiveCleaner<Configured> {
    /// The rate of the shortest cleanup interval configured
    fn ticker_speed(&self) -> Duration {
        self.configurations
            .values()
            .min_by_key(|cfg| cfg.cleanup_every)
            .map(|cfg| cfg.cleanup_every)
            // The unwrap should never trigger, as a Configured ArchiveCleaner always has at least one entry
            .unwrap_or_else(|| Duration::from_secs(60))
    }

    pub fn run(self, pool: &PgPool) -> AbortHandle {
        let task = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(self.ticker_speed());
            let mut last_runs: HashMap<JobType, Instant> = HashMap::new();

            loop {
                ticker.tick().await;

                for (queue_name, config) in &self.configurations {
                    if last_runs
                        .get(queue_name)
                        .map(|last_run| {
                            Instant::now().duration_since(*last_run) < config.cleanup_every
                        })
                        .unwrap_or(false)
                    {
                        continue;
                    }

                    let mut query =
                        sqlx::QueryBuilder::new("DELETE FROM archived_jobs WHERE job_type = ");
                    query.push_bind(queue_name);
                    match config.policy {
                        CleanupPolicy::MaxAge(duration) => todo!(),
                        CleanupPolicy::MaxCount(_) => todo!(),
                        CleanupPolicy::Mixed { max_age, max_count } => todo!(),
                    }

                    // TODO: execute
                    last_runs.insert(queue_name.to_owned(), Instant::now());
                }
            }
        });
        task.abort_handle()
    }
}
