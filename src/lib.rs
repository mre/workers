#![doc = include_str!("../README.md")]

mod background_job;
mod errors;
mod job_registry;
mod runner;
/// Database schema definitions.
pub mod schema;
mod storage;
mod util;
mod worker;

/// The main trait for defining background jobs.
pub use self::background_job::BackgroundJob;
/// The default queue name used when no specific queue is specified.
pub use self::background_job::DEFAULT_QUEUE;
/// Error type for job enqueueing operations.
pub use self::errors::EnqueueError;
/// The main runner that orchestrates job processing.
pub use self::runner::Runner;
/// Database schema types.
pub use self::schema::{ArchivedJob, BackgroundJob as BackgroundJobRecord};
/// Archive functionality.
pub use self::storage::{archived_job_count, get_archived_jobs};
