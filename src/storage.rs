use crate::schema::BackgroundJob;
use sqlx::{PgPool, Postgres, Transaction};

/// The number of jobs that have failed at least once
pub(crate) async fn failed_job_count(pool: &PgPool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM background_jobs WHERE retries > 0")
        .fetch_one(pool)
        .await
}

/// Finds the next job that is unlocked, and ready to be retried.
pub(crate) async fn find_next_unlocked_job_tx(
    tx: &mut Transaction<'_, Postgres>,
    job_types: &[String],
) -> Result<BackgroundJob, sqlx::Error> {
    sqlx::query_as::<_, BackgroundJob>(
        r"
        SELECT id, job_type, data, retries, last_retry, created_at, priority
        FROM background_jobs
        WHERE job_type = ANY($1)
          AND (retries = 0 OR last_retry < NOW() - INTERVAL '1 minute' * POWER(2, retries))
        ORDER BY priority DESC, id ASC
        FOR UPDATE SKIP LOCKED
        LIMIT 1
        ",
    )
    .bind(job_types)
    .fetch_one(&mut **tx)
    .await
}

/// Deletes a job that has successfully completed running
pub(crate) async fn delete_successful_job(
    tx: &mut Transaction<'_, Postgres>,
    job_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM background_jobs WHERE id = $1")
        .bind(job_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Marks that we just tried and failed to run a job.
///
/// Ignores any database errors that may have occurred. If the DB has gone away,
/// we assume that just trying again with a new connection will succeed.
pub(crate) async fn update_failed_job(
    tx: &mut Transaction<'_, Postgres>,
    job_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE background_jobs SET retries = retries + 1, last_retry = NOW() WHERE id = $1",
    )
    .bind(job_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
