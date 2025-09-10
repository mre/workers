# Stress Test

A stress test that demonstrates the `workers` library using Pokemon-themed background jobs.

## Overview

The stress test creates multiple types of background jobs and processes them
across different queues with varying priorities and worker counts. Jobs have
realistic processing times and success rates to simulate actual workloads.

Key characteristics:
- 5 job types with different priorities and processing requirements
- 5 specialized queues with independent worker pools
- Job deduplication for training operations

## Job Types

1. **Catch Pokemon** (`field_work` queue, priority 10)
   - Variable processing time (100-800ms)
   - Success rate based on Pokemon level and trainer experience

2. **Train Pokemon** (`training` queue, priority 5, deduplicated)
   - Processing time 200-1500ms
   - Deduplicated to prevent duplicate training sessions

3. **Heal Pokemon** (`pokemon_center` queue, priority 15)
   - Processing time scales with number of Pokemon
   - Simulates batch healing operations

4. **Gym Battles** (`gym_battles` queue, priority 20)
   - Longest processing time (1000-3000ms)
   - Success rate improves with trainer badges

5. **Explore Areas** (`exploration` queue, priority 3)
   - Processing time 300-1200ms
   - Generates multiple discovery events per job

## Queue Configuration

The test configures 5 queues with different worker counts and poll intervals:

- **Field Work**: Default worker count, 50ms poll interval
- **Training**: Half worker count, 100ms poll interval  
- **Pokemon Center**: Quarter worker count, 100ms poll interval
- **Gym Battles**: Fixed 2 workers, 200ms poll interval
- **Exploration**: Default worker count, 150ms poll interval

## Usage

### Run from project root with make:
```bash
make stress
```

### Run directly with custom parameters:
```bash
cd examples/stress_test
cargo run --release -- --help
```

### Command-line Options:
- `--jobs <N>`: Number of jobs to enqueue (default: 100)
- `--workers <N>`: Base worker count (default: CPU cores)
- `--duration <N>`: Maximum test duration in seconds (default: 30)
- `--skip-db-setup`: Use existing database instead of TestContainers
- `--database-url <URL>`: Database URL when skipping setup

### Examples:
```bash
# Quick test
cargo run --release -- --jobs 20 --workers 2 --duration 15

# Heavy load test
cargo run --release -- --jobs 1000 --workers 16 --duration 120

# Use existing database
cargo run --release -- --skip-db-setup --database-url postgresql://localhost/workers_test
```

## Implementation Details

It's surprising how much can be demonstrated with such a simple example:

- **Transaction-based job locking**: Jobs remain locked during execution to prevent concurrent processing
- **Priority queues**: Higher priority jobs (healing: 15, gym battles: 20) are processed before lower priority ones (exploration: 3)
- **Job deduplication**: Training jobs with identical data are automatically deduplicated
- **Retry handling**: Failed jobs are retried with exponential backoff

