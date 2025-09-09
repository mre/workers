use crate::schema::BackgroundJob;
use sqlx::PgPool;

/// Finds the next job that is unlocked, and ready to be retried. If a row is
/// found, it will be locked using SKIP LOCKED.
pub(crate) async fn find_next_unlocked_job(
    pool: &PgPool,
    job_types: &[String],
) -> Result<BackgroundJob, sqlx::Error> {
    sqlx::query_as::<_, BackgroundJob>(
        r"
        SELECT id, job_type, data, retries, last_retry, created_at, priority
        FROM background_jobs
        WHERE job_type = ANY($1)
          AND last_retry < NOW() - INTERVAL '1 minute' * POWER(2, retries)
        ORDER BY priority DESC, id ASC
        FOR UPDATE SKIP LOCKED
        LIMIT 1
        ",
    )
    .bind(job_types)
    .fetch_one(pool)
    .await
}

/// The number of jobs that have failed at least once
pub(crate) async fn failed_job_count(pool: &PgPool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM background_jobs WHERE retries > 0")
        .fetch_one(pool)
        .await
}

/// Deletes a job that has successfully completed running
pub(crate) async fn delete_successful_job(pool: &PgPool, job_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM background_jobs WHERE id = $1")
        .bind(job_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Marks that we just tried and failed to run a job.
///
/// Ignores any database errors that may have occurred. If the DB has gone away,
/// we assume that just trying again with a new connection will succeed.
pub(crate) async fn update_failed_job(pool: &PgPool, job_id: i64) {
    let _ = sqlx::query(
        "UPDATE background_jobs SET retries = retries + 1, last_retry = NOW() WHERE id = $1",
    )
    .bind(job_id)
    .execute(pool)
    .await;
}
