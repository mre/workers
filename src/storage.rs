use crate::schema::{ArchivedJob, BackgroundJob};
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

/// Archives a job that has successfully completed running
///
/// This moves the job from the `background_jobs` table to the `archived_jobs` table,
/// preserving all the original data plus an archive timestamp.
pub(crate) async fn archive_successful_job(
    tx: &mut Transaction<'_, Postgres>,
    job_id: i64,
) -> Result<(), sqlx::Error> {
    // Insert into archived_jobs table from background_jobs
    sqlx::query(
        r"
        INSERT INTO archived_jobs (id, job_type, data, retries, last_retry, created_at, priority)
        SELECT id, job_type, data, retries, last_retry, created_at, priority
        FROM background_jobs
        WHERE id = $1
        ",
    )
    .bind(job_id)
    .execute(&mut **tx)
    .await?;

    // Delete from background_jobs table
    sqlx::query("DELETE FROM background_jobs WHERE id = $1")
        .bind(job_id)
        .execute(&mut **tx)
        .await?;

    Ok(())
}

/// Get archived jobs for a specific job type
pub async fn get_archived_jobs(
    pool: &PgPool,
    job_type: Option<&str>,
    limit: Option<i64>,
) -> Result<Vec<ArchivedJob>, sqlx::Error> {
    let mut query = "SELECT id, job_type, data, retries, last_retry, created_at, archived_at, priority FROM archived_jobs".to_string();

    if job_type.is_some() {
        query.push_str(" WHERE job_type = $1");
    }

    query.push_str(" ORDER BY archived_at DESC");

    if limit.is_some() {
        if job_type.is_some() {
            query.push_str(" LIMIT $2");
        } else {
            query.push_str(" LIMIT $1");
        }
    }

    let mut query_builder = sqlx::query_as::<_, ArchivedJob>(&query);

    if let Some(job_type_val) = job_type {
        query_builder = query_builder.bind(job_type_val);
    }

    if let Some(limit_val) = limit {
        query_builder = query_builder.bind(limit_val);
    }

    query_builder.fetch_all(pool).await
}

/// Get count of archived jobs
pub async fn archived_job_count(pool: &PgPool) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM archived_jobs")
        .fetch_one(pool)
        .await
}
