# workers

A robust async PostgreSQL-backed background job processing system.

## Overview

This crate provides an async PostgreSQL-backed job queue system with support for:

- **Prioritized job execution** with configurable priorities
- **Job deduplication** to prevent duplicate work
- **Multiple job queues** with independent worker pools
- **Automatic retry** with exponential backoff for failed jobs
- **Graceful shutdown** and queue management
- **Error tracking** with Sentry integration

## Architecture

The system consists of three main components:

- **`BackgroundJob`** trait - Define job types and their execution logic
- **`Runner`** - High-level orchestrator that manages multiple queues and their worker pools
- **`Worker`** - Low-level executor that polls for and processes individual jobs

### Runner vs Worker

- **`Runner`** is the entry point and orchestrator:
  - Manages multiple named queues (e.g., "default", "emails", "indexing")
  - Spawns and coordinates multiple `Worker` instances per queue
  - Handles job type registration and queue configuration
  - Provides graceful shutdown coordination across all workers

- **`Worker`** is the actual job processor:
  - Polls the database for available jobs in a specific queue
  - Locks individual jobs to prevent concurrent execution
  - Executes job logic with error handling and retry logic
  - Reports job completion or failure back to the database

Jobs are stored in the `background_jobs` `PostgreSQL` table and processed asynchronously by worker instances that poll for available work in their assigned queues.

### Job Processing and Locking

When a worker picks up a job from the database, the table row is immediately locked to prevent other workers from processing the same job concurrently. This ensures that:

- Each job is processed exactly once, even with multiple workers running
- Failed jobs can be safely retried without duplication
- The system scales horizontally by adding more worker processes

Once job execution completes successfully, the row is deleted from the table. If the job fails, the row remains with updated retry information for future processing attempts.

## Database Schema

```sql
CREATE TABLE background_jobs (
    id BIGSERIAL PRIMARY KEY,
    job_type TEXT NOT NULL,
    data JSONB NOT NULL,
    retries INTEGER NOT NULL DEFAULT 0,
    last_retry TIMESTAMP NOT NULL DEFAULT NOW(),
    created_at TIMESTAMP NOT NULL DEFAULT NOW(),
    priority SMALLINT NOT NULL DEFAULT 0
);
```

## Usage

### Defining a Job

```rust, ignore
use workers::BackgroundJob;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct SendEmailJob {
    to: String,
    subject: String,
    body: String,
}

impl BackgroundJob for SendEmailJob {
    const JOB_NAME: &'static str = "send_email";
    const PRIORITY: i16 = 10;
    const DEDUPLICATED: bool = false;
    const QUEUE: &'static str = "emails";

    type Context = AppContext;

    async fn run(&self, ctx: Self::Context) -> anyhow::Result<()> {
        // Job implementation
        ctx.email_service.send(&self.to, &self.subject, &self.body).await?;
        Ok(())
    }
}
```

### Running the Worker

```rust,ignore
use workers::Runner;
use std::time::Duration;

let runner = Runner::new(connection_pool, app_context)
    .register_job_type::<SendEmailJob>()
    .configure_queue("emails", |queue| {
        queue.num_workers(2).poll_interval(Duration::from_secs(5))
    });

let handle = runner.start();
handle.wait_for_shutdown().await;
```

### Enqueuing Jobs

```rust,ignore
let job = SendEmailJob {
    to: "user@example.com".to_string(),
    subject: "Welcome!".to_string(),
    body: "Thanks for signing up!".to_string(),
};

job.enqueue(&mut conn).await?;
```

## Configuration

### Job Properties

- **`JOB_NAME`**: Unique identifier for the job type
- **`PRIORITY`**: Execution priority (higher values = higher priority)
- **`DEDUPLICATED`**: Whether to prevent duplicate jobs with identical data
- **`QUEUE`**: Queue name for job execution (defaults to "default")

### Queue Configuration

- **Worker count**: Number of concurrent workers per queue
- **Poll interval**: How often workers check for new jobs
- **Shutdown behavior**: Whether to stop when queue is empty

## Error Handling

Failed jobs are automatically retried with exponential backoff. The retry count and last retry timestamp are tracked in the database. Jobs that continue to fail will eventually be abandoned after reaching the maximum retry limit.

All job execution is instrumented with tracing and optionally reported to Sentry for error monitoring.

## Production Considerations

### Database Polling

The workers poll the database for new jobs at regular intervals (configurable via `poll_interval`). While this is simple and reliable, it does generate constant database queries. For production deployments, consider:

**Tuning Poll Intervals:**
```rust
.configure_queue("low_priority", |queue| {
    queue.poll_interval(Duration::from_secs(30))  // Less frequent polling
})
.configure_queue("urgent", |queue| {
    queue.poll_interval(Duration::from_millis(100))  // More frequent polling
})
```

## Development Setup

This project uses [TestContainers](https://rust.testcontainers.org/) for integration testing, which automatically spins up `PostgreSQL` containers during test execution.

### Example Usage 

You can run a stress test like so:

```bash
make stress
```

Or run with custom parameters

```bash
cd examples/stress_test
cargo run --release -- --jobs 500 --workers 12 --duration 60
```

The stress test is based on a popular monster collecting game. ;)
It demonstrates a few features:

- Multiple job types: Catch Pokemon, train them, heal at Pokemon Centers, battle gym leaders, and explore new areas
- Different queue priorities: Healing and gym battles get higher priority than exploration
- Job deduplication: Training jobs are deduplicated to prevent over-training the same Pokemon
- Different processing times: Each job type has different duration ranges to simulate real workloads

### Testing

Simply run the tests - `TestContainers` handles the database setup automatically:

```bash
# Run all tests (PostgreSQL containers managed automatically)
make test

# Run tests with verbose output
make test-verbose

# Run all quality checks (format, lint, test)
make ci
```

### Requirements

- **Docker**: `TestContainers` requires Docker to be running for integration tests
  - ⚠️ **Important**: Make sure Docker Desktop (or equivalent) is started before running tests
  - Tests will fail with connection errors if Docker is not available
- **Rust**: Standard Rust toolchain for compilation

The tests will automatically:
1. Start a `PostgreSQL` container using `TestContainers`
2. Run database migrations 
3. Execute the test suite
4. Clean up containers when finished

No manual database setup required!

### Troubleshooting

If tests fail with "client error (Connect)" or similar Docker errors:
1. Ensure Docker Desktop is running
2. Verify with: `docker ps`
3. If Docker is running but tests still fail, try: `docker system prune` to clean up resources

## History

The implementation was originally developed as part of crates.io and was extracted from the separate
[`swirl`](https://github.com/sgrif/swirl) project, then re-integrated and heavily modified
to provide a robust, production-ready job queue system.
