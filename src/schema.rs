//! Database schema definitions for SQLx.
//!
//! This module contains the database types and structures for the background job system.

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::FromRow;

/// Represents a background job record in the database
#[derive(Debug, Clone, FromRow)]
pub struct BackgroundJob {
    /// Unique identifier for the job
    pub id: i64,
    /// Type identifier for the job (used for dispatch)
    pub job_type: String,
    /// JSON data containing the job payload
    pub data: Value,
    /// Number of retry attempts made
    pub retries: i32,
    /// Timestamp of the last retry attempt
    pub last_retry: DateTime<Utc>,
    /// Timestamp when the job was created
    pub created_at: DateTime<Utc>,
    /// Priority of the job (higher = more important)
    pub priority: i16,
}

/// Represents an archived job record in the database
#[derive(Debug, Clone, FromRow)]
pub struct ArchivedJob {
    /// The original background job data
    #[sqlx(flatten)]
    pub job: BackgroundJob,
    /// Timestamp when the job was archived
    pub archived_at: DateTime<Utc>,
}
