-- Create archived_jobs table for job archive system
CREATE TABLE IF NOT EXISTS archived_jobs (
    id BIGINT NOT NULL, -- Original job ID from background_jobs
    job_type TEXT NOT NULL,
    data JSONB NOT NULL,
    retries INTEGER NOT NULL DEFAULT 0,
    last_retry TIMESTAMP WITH TIME ZONE NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL,
    archived_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    priority SMALLINT NOT NULL DEFAULT 0,
    PRIMARY KEY (id, archived_at) -- Composite key allows same job ID to be archived multiple times
);

-- Create indexes for efficient archive querying
CREATE INDEX IF NOT EXISTS idx_archived_jobs_job_type ON archived_jobs(job_type);
CREATE INDEX IF NOT EXISTS idx_archived_jobs_archived_at ON archived_jobs(archived_at DESC);
CREATE INDEX IF NOT EXISTS idx_archived_jobs_created_at ON archived_jobs(created_at DESC);