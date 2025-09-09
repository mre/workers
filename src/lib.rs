#![doc = include_str!("../README.md")]
#![warn(missing_docs)]
#![forbid(unsafe_code)]

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
/// Error type for job enqueueing operations.
pub use self::errors::EnqueueError;
/// The main runner that orchestrates job processing.
pub use self::runner::Runner;
