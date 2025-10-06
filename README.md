# workers ⚙️🦀

[![docs.rs](https://docs.rs/workers/badge.svg)](https://docs.rs/workers)
[![CI](https://github.com/mre/workers/actions/workflows/ci.yml/badge.svg)](https://github.com/mre/workers/actions/workflows/ci.yml)

A robust async PostgreSQL-backed background job processing system. It features:

- **Prioritized job execution** with configurable priorities
- **Job deduplication** to prevent duplicate work
- **Multiple job queues** with independent worker pools
- **Automatic retry** with exponential backoff for failed jobs
- **Graceful shutdown** and queue management
- **Error tracking** with Sentry integration

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
    const JOB_TYPE: &'static str = "send_email";
    const PRIORITY: i16 = 10;
    const DEDUPLICATED: bool = false;

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
    .configure_queue("emails", |queue| {
        queue
        .register::<SendEmailJob>()
        .num_workers(2)
        .poll_interval(Duration::from_secs(5))
        .jitter(Duration::from_millis(500))
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

- **`JOB_TYPE`**: Unique identifier for the job type
- **`PRIORITY`**: Execution priority (higher values = higher priority)
- **`DEDUPLICATED`**: Whether to prevent duplicate jobs with identical data
- **`QUEUE`**: Queue name for job execution (defaults to "default")

### Queue Configuration

- **Worker count**: Number of concurrent workers per queue
- **Poll interval**: How often workers check for new jobs
- **Jitter**: Random time added to poll intervals to reduce thundering herd effects (default: 100ms)
- **Shutdown behavior**: Whether to stop when queue is empty
- **Archive completed jobs**: Whether to archive successful jobs instead of deleting them

## Batch Operations

For improved throughput in high-volume scenarios, you can enqueue multiple jobs
of the same type in a single operation. This is more efficient than individual
`enqueue()` calls as it uses a single database transaction and `PostgreSQL` bulk
insert operations. Under the hood, this uses `PostgreSQL` arrays with UNNEST for
bulk insert, handles deduplication with conditional inserts, and uses a single
transaction for atomicity.

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

### Batch Enqueuing

```rust,ignore
use workers::BackgroundJob;

// Create multiple jobs of the same type
let email_jobs = vec![
    SendEmailJob { to: "user1@example.com".to_string(), subject: "Welcome!".to_string(), body: "...".to_string() },
    SendEmailJob { to: "user2@example.com".to_string(), subject: "Welcome!".to_string(), body: "...".to_string() },
    SendEmailJob { to: "user3@example.com".to_string(), subject: "Welcome!".to_string(), body: "...".to_string() },
];

// Enqueue all jobs in a single transaction
let job_ids = SendEmailJob::enqueue_batch(&email_jobs, &pool).await?;

// job_ids contains Option<i64> for each job:
// - Some(id) for successfully enqueued jobs
// - None for jobs that were deduplicated (if DEDUPLICATED = true)
```

You can see this in action with:
```bash
cargo run --example batch
```

## Job Archiving

By default, successfully completed jobs are deleted from the database. However, you can configure jobs to be archived instead, which moves them to an archive table for debugging, auditing, and potential replay.

```rust,ignore
use workers::Runner;
use runner::ArchivalPolicy;

let runner = Runner::new(pool, context)
    .configure_queue("important", |queue| {
        queue
        .register::<MyJob>()
        .archive(ArchivalPolicy::Always)  // Enable archiving
    });

// We can get much more fine-grained control by using a predicate function:
let runner = Runner::new(pool, context)
    .configure_queue("fails_sometimes", |queue| {
        queue
        .register::<MyJob>()
        .archive(ArchivalPolicy::If(|job, _ctx| {
            // Archive only jobs that had to retry
            job.retries > 0
        }))
    });
```

To query archived jobs:

```rust,no_run
use workers::{get_archived_jobs, ArchiveQuery, archived_job_count};
# use sqlx::PgPool;
# async fn example() -> Result<(), Box<dyn std::error::Error>> {
# let pool = PgPool::connect("").await?;

// Get all archived jobs
let archived = get_archived_jobs(&pool, ArchiveQuery::All).await?;

// Get archived jobs for a specific job type
let archived = get_archived_jobs(
    &pool,
    ArchiveQuery::Filter {
        job_filter: Some("important".to_string()),
        limit: None,
    }
).await?;

// Get archived jobs with limit
let archived = get_archived_jobs(
    &pool,
    ArchiveQuery::Filter {
        job_filter: Some("important".to_string()),
        limit: Some(50),
    }
).await?;

// Get count of archived jobs
let count = archived_job_count(&pool).await?;
# Ok(())
# }
```

## Archive cleanup

In case you have chosen to archive successfully completed jobs, you might run into the issue that your database keeps growing in size indefinitely.

For this eventuality, you can configure an `ArchiveCleaner` to take care of cleanups for you.

Each configured job type will start a background task with its own timer that periodically runs cleanup operations based on the provided configuration.

```rust,ignore
use workers::{ArchiveCleanerBuilder, CleanupConfiguration, CleanupPolicy};
use std::time::Duration;

let cleaner = ArchiveCleanerBuilder::new()
    .configure::<MyJob>(CleanupConfiguration {
        cleanup_every: Duration::from_secs(60), // Run cleanup every 60 seconds
        policy: CleanupPolicy::MaxCount(1000),  // Keep only the latest 1000 archived jobs
    })
    .run(&pool);

// Do whatever else...

// You can manually stop the cleaner when needed:
cleaner.stop();
// or just let it fall out of scope and be aborted automatically.
```

## Database Setup

> [!NOTE]\
>  These migrations only CREATE new tables - your existing data is completely safe.

This library requires some `PostgreSQL` tables to work. There are a few ways to setup up the DB.

### One-line Setup

The easiest way is to use the built-in setup function:

```rust,no_run
use sqlx::PgPool;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pool = PgPool::connect("postgresql://user:pass@localhost/db").await?;

    // One-line database setup - safe to call multiple times
    // Under the hood, this runs `sqlx::migrate`
    workers::setup_database(&pool).await?;

    // Now you can use the workers library...
    Ok(())
}
```

This embeds the migrations at compile time and is completely self-contained.

### Option 2: Run SQL directly

Execute the SQL statements in the `migrations` folder on your existing `PostgreSQL` database.

## Error Handling

Failed jobs are automatically retried with exponential backoff. The retry count and last retry timestamp are tracked in the database. Jobs that continue to fail will eventually be abandoned after reaching the maximum retry limit.

All job execution is instrumented with tracing and optionally reported to Sentry for error monitoring.

## Production Considerations

### Database Polling

The workers poll the database for new jobs at regular intervals (configurable via `poll_interval`). While this is simple and reliable, it does generate constant database queries. For production deployments, consider:

**Tuning Poll Intervals:**
```rust,ignore
.configure_queue("low_priority", |queue| {
    queue.poll_interval(Duration::from_secs(30))  // Less frequent polling
})
.configure_queue("urgent", |queue| {
    queue.poll_interval(Duration::from_millis(100))  // More frequent polling
})
```

### Why Use Polling Instead of LISTEN/NOTIFY?

`PostgreSQL`'s LISTEN/NOTIFY feature seems perfect for real-time job notifications, but we chose polling with jitter instead. Here's why:

- LISTEN/NOTIFY creates thundering herds. When you send one NOTIFY, all workers wake up at once and compete for the same single job. This creates database contention spikes that hurt performance.
- You still need polling anyway. LISTEN/NOTIFY isn't reliable enough on its own - notifications can be lost during connection drops or server restarts. You end up implementing both systems, which adds complexity.
- Connection poolers interfere. Production deployments often use `PgBouncer` or similar tools. LISTEN requires persistent connections, which conflicts with connection pool strategies.

The polling approach prevents thundering herds through added jitter, works with any `PostgreSQL` setup, is simple and reliable (no missed notifications) and easy to tune for your workload (e.g. few or many jobs). Feel free to open an issue if we're missing anything.

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
