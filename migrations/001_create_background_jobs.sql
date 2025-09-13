-- Create background_jobs table for job queue system
CREATE TABLE IF NOT EXISTS background_jobs (
    id BIGSERIAL PRIMARY KEY,
    job_type TEXT NOT NULL,
    data JSONB NOT NULL,
    retries INTEGER NOT NULL DEFAULT 0,
    last_retry TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    priority SMALLINT NOT NULL DEFAULT 0
);

-- Create indexes for efficient job processing
CREATE INDEX IF NOT EXISTS idx_background_jobs_job_type ON background_jobs(job_type);
CREATE INDEX IF NOT EXISTS idx_background_jobs_priority_created_at ON background_jobs(priority DESC, created_at ASC);
CREATE INDEX IF NOT EXISTS idx_background_jobs_retries ON background_jobs(retries);