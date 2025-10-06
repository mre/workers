use crate::job_registry::JobRegistry;
use crate::worker::Worker;
use crate::{BackgroundJob, schema};
use futures_util::future::join_all;
use sqlx::PgPool;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::{Instrument, info, info_span, warn};

const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(1);
const DEFAULT_JITTER: Duration = Duration::from_millis(100);

/// Marker type for a configured runner
#[derive(Debug)]
#[allow(missing_copy_implementations)]
pub struct Configured;
/// Marker type for an unconfigured runner
#[derive(Debug)]
#[allow(missing_copy_implementations)]
pub struct Unconfigured;

/// The core runner responsible for locking and running jobs
pub struct Runner<Context: Clone + Send + Sync + 'static, State = Unconfigured> {
    connection_pool: PgPool,
    queues: HashMap<String, Queue<Context, Configured>>,
    context: Context,
    shutdown_when_queue_empty: bool,
    _state: PhantomData<State>,
}

impl<Context: std::fmt::Debug + Clone + Sync + Send, State: std::fmt::Debug> std::fmt::Debug
    for Runner<Context, State>
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runner")
            .field("queues", &self.queues.keys().collect::<Vec<_>>())
            .field("context", &self.context)
            .field("shutdown_when_queue_empty", &self.shutdown_when_queue_empty)
            .finish()
    }
}

impl<Context: Clone + Send + Sync + 'static> Runner<Context> {
    /// Create a new runner with the given connection pool and context.
    pub fn new(connection_pool: PgPool, context: Context) -> Self {
        Self {
            connection_pool,
            queues: HashMap::new(),
            context,
            shutdown_when_queue_empty: false,
            _state: PhantomData,
        }
    }
}

impl<Context: Clone + Send + Sync + 'static, State> Runner<Context, State> {
    /// Configure a queue
    pub fn configure_queue(
        mut self,
        queue_name: &str,
        config_fn: impl FnOnce(Queue<Context>) -> Queue<Context, Configured>,
    ) -> Runner<Context, Configured> {
        self.queues
            .insert(queue_name.into(), config_fn(Queue::default()));

        Runner {
            connection_pool: self.connection_pool,
            queues: self.queues,
            context: self.context,
            shutdown_when_queue_empty: self.shutdown_when_queue_empty,
            _state: PhantomData,
        }
    }

    /// Set the runner to shut down when the background job queue is empty.
    pub fn shutdown_when_queue_empty(mut self) -> Self {
        self.shutdown_when_queue_empty = true;
        self
    }
}

impl<Context: Clone + Send + Sync + 'static> Runner<Context, Configured> {
    /// Start the background workers.
    ///
    /// This returns a `RunningRunner` which can be used to wait for the workers to shutdown.
    pub fn start(&self) -> RunHandle {
        let mut handles = Vec::new();
        for (queue_name, queue) in &self.queues {
            for i in 1..=queue.num_workers {
                let name = format!("background-worker-{queue_name}-{i}");
                info!(worker.name = %name, "Starting worker…");

                let worker = Worker {
                    connection_pool: self.connection_pool.clone(),
                    context: self.context.clone(),
                    job_registry: Arc::new(queue.job_registry.clone()),
                    shutdown_when_queue_empty: self.shutdown_when_queue_empty,
                    poll_interval: queue.poll_interval,
                    jitter: queue.jitter,
                    archive_completed_jobs: queue.archive_completed_jobs.clone(),
                };

                let span = info_span!("worker", worker.name = %name);
                let handle = tokio::spawn(async move { worker.run().instrument(span).await });

                handles.push(handle);
            }
        }

        RunHandle { handles }
    }
}

/// Handle to a running background job processing system
#[derive(Debug)]
pub struct RunHandle {
    handles: Vec<JoinHandle<()>>,
}

impl RunHandle {
    /// Wait for all background workers to shut down.
    pub async fn wait_for_shutdown(self) {
        join_all(self.handles).await.into_iter().for_each(|result| {
            if let Err(error) = result {
                warn!(%error, "Background worker task panicked");
            }
        });
    }
}

/// Archival configuration
#[derive(Clone, Default)]
pub enum ArchivalPolicy<Context> {
    /// Archive nothing
    #[default]
    Never,
    /// Archive all completed jobs
    Always,
    /// Archive based on a predicate function
    If(fn(&schema::BackgroundJob, &Context) -> bool),
}

impl<Context> std::fmt::Debug for ArchivalPolicy<Context> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Never => write!(f, "ArchivalPolicy::Never"),
            Self::Always => write!(f, "ArchivalPolicy::Always"),
            Self::If(_) => write!(f, "ArchivalPolicy::If(<function>)"),
        }
    }
}

/// Configuration and state for a job queue
#[derive(Debug)]
pub struct Queue<Context: Clone + Send + Sync + 'static, State = Unconfigured> {
    job_registry: JobRegistry<Context>,
    num_workers: usize,
    poll_interval: Duration,
    jitter: Duration,
    archive_completed_jobs: ArchivalPolicy<Context>,
    _state: PhantomData<State>,
}

impl<Context: Clone + Send + Sync + 'static> Default for Queue<Context, Unconfigured> {
    fn default() -> Self {
        Self {
            job_registry: JobRegistry::default(),
            num_workers: 1,
            poll_interval: DEFAULT_POLL_INTERVAL,
            jitter: DEFAULT_JITTER,
            archive_completed_jobs: ArchivalPolicy::default(),
            _state: PhantomData,
        }
    }
}

impl<Context: Clone + Send + Sync + 'static, State> Queue<Context, State> {
    /// Set the number of worker threads for this queue.
    pub fn num_workers(mut self, num_workers: usize) -> Self {
        self.num_workers = num_workers;
        self
    }

    /// Set how often workers poll for new jobs.
    pub fn poll_interval(mut self, poll_interval: Duration) -> Self {
        self.poll_interval = poll_interval;
        self
    }

    /// Set the maximum random jitter to add to poll intervals.
    ///
    /// Jitter helps reduce thundering herd effects when multiple workers
    /// are polling for jobs simultaneously. The actual jitter applied will
    /// be a random value between 0 and the specified duration.
    pub fn jitter(mut self, jitter: Duration) -> Self {
        self.jitter = jitter;
        self
    }

    /// Set whether completed jobs should be archived instead of deleted.
    pub fn archive(mut self, policy: ArchivalPolicy<Context>) -> Self {
        self.archive_completed_jobs = policy;
        self
    }

    /// Configure a job to run as part of this queue.
    pub fn register<J: BackgroundJob<Context = Context>>(mut self) -> Queue<Context, Configured> {
        self.job_registry.register::<J>();
        Queue {
            job_registry: self.job_registry,
            num_workers: self.num_workers,
            poll_interval: self.poll_interval,
            jitter: self.jitter,
            archive_completed_jobs: self.archive_completed_jobs,
            _state: PhantomData,
        }
    }
}
