#![doc = include_str!("../README.md")]

mod background_job;
mod cleaner;
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
/// The archive cleaner for purging archived jobs.
pub use self::cleaner::{ArchiveCleanerBuilder, CleanupConfiguration, CleanupPolicy};
/// Error type for job enqueueing operations.
pub use self::errors::EnqueueError;
/// The main runner that orchestrates job processing.
pub use self::runner::{ArchivalPolicy, Queue, Runner};
/// Database schema types.
pub use self::schema::{ArchivedJob, BackgroundJob as BackgroundJobRecord};
/// Archive functionality.
pub use self::storage::{ArchiveQuery, archived_job_count, get_archived_jobs};

/// Database setup
///
/// Sets up the required database tables for the workers library.
/// This function is idempotent - it's safe to call multiple times.
///
/// # Example
///
/// ```rust,no_run
/// use sqlx::PgPool;
/// use workers::setup_database;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let pool = PgPool::connect("postgresql://user:pass@localhost/db").await?;
///     setup_database(&pool).await?;
///
///     // Now you can use the workers library...
///     Ok(())
/// }
/// ```
pub async fn setup_database(pool: &sqlx::PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("./migrations").run(pool).await
}
