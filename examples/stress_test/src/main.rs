mod jobs;

use anyhow::Result;
use clap::Parser;
use jobs::*;
use rand::Rng;
use sqlx::PgPool;
use std::time::Duration;
use testcontainers::ContainerAsync;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use tokio::time::{Instant, sleep};
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use workers::{BackgroundJob, Runner};

/// Pokemon Stress Test - Demonstrates high-throughput job processing
#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Number of jobs to enqueue
    #[arg(short, long, default_value_t = 100)]
    jobs: usize,

    /// Number of workers per queue
    #[arg(short, long, default_value_t = num_cpus::get())]
    workers: usize,

    /// Duration to run the stress test (seconds)
    #[arg(short, long, default_value_t = 30)]
    duration: u64,

    /// Skip database setup and use existing connection
    #[arg(long)]
    skip_db_setup: bool,

    /// Database URL (if skip_db_setup is true)
    #[arg(
        long,
        default_value = "postgresql://postgres:postgres@localhost:5432/postgres"
    )]
    database_url: String,
}

const POKEMON_NAMES: &[&str] = &[
    "Pikachu",
    "Charizard",
    "Blastoise",
    "Venusaur",
    "Gengar",
    "Machamp",
    "Alakazam",
    "Golem",
    "Dragonite",
    "Mewtwo",
    "Mew",
    "Typhlosion",
    "Feraligatr",
    "Meganium",
    "Lugia",
    "Ho-Oh",
    "Celebi",
    "Blaziken",
    "Swampert",
    "Sceptile",
    "Rayquaza",
    "Kyogre",
    "Groudon",
    "Garchomp",
    "Lucario",
    "Dialga",
    "Palkia",
    "Giratina",
    "Arceus",
    "Reshiram",
];

const LOCATIONS: &[&str] = &[
    "Viridian Forest",
    "Mt. Moon",
    "Cerulean Cave",
    "Safari Zone",
    "Victory Road",
    "Indigo Plateau",
    "Route 1",
    "Pallet Town",
    "Pewter City",
    "Lavender Town",
    "Saffron City",
    "Fuchsia City",
];

const GYM_LEADERS: &[(&str, &str, &str)] = &[
    ("Brock", "Rock", "Pewter"),
    ("Misty", "Water", "Cerulean"),
    ("Lt. Surge", "Electric", "Vermilion"),
    ("Erika", "Grass", "Celadon"),
    ("Sabrina", "Psychic", "Saffron"),
    ("Blaine", "Fire", "Cinnabar"),
    ("Giovanni", "Ground", "Viridian"),
    ("Blue", "Mixed", "Viridian"),
];

const AREAS: &[(&str, &str)] = &[
    ("Dark Cave", "cave"),
    ("Ilex Forest", "forest"),
    ("Route 32", "water"),
    ("Mt. Silver", "mountain"),
    ("Tohjo Falls", "water"),
    ("Ice Path", "cave"),
    ("Dragon's Den", "water"),
    ("National Park", "forest"),
];

async fn setup_database() -> Result<(PgPool, Option<ContainerAsync<Postgres>>)> {
    info!("🐳 Starting PostgreSQL container...");

    let postgres_image = Postgres::default();
    let container = postgres_image.start().await?;

    let host = container.get_host().await?;
    let port = container.get_host_port_ipv4(5432).await?;
    let connection_string = format!("postgresql://postgres:postgres@{}:{}/postgres", host, port);

    info!("📡 Connecting to database at {}:{}...", host, port);
    let pool = PgPool::connect(&connection_string).await?;

    info!("🔄 Running database migrations...");
    sqlx::migrate!("../../migrations").run(&pool).await?;

    info!("✅ Database setup complete!");
    Ok((pool, Some(container)))
}

async fn connect_existing_database(
    url: &str,
) -> Result<(PgPool, Option<ContainerAsync<Postgres>>)> {
    info!("📡 Connecting to existing database...");
    let pool = PgPool::connect(url).await?;

    info!("🔄 Running database migrations...");
    sqlx::migrate!("../../migrations").run(&pool).await?;

    info!("✅ Connected to existing database!");
    Ok((pool, None))
}

async fn enqueue_pokemon_jobs(pool: &PgPool, job_count: usize) -> Result<()> {
    info!("📝 Enqueuing {} Pokemon jobs...", job_count);

    let start = Instant::now();
    let mut enqueued = 0;

    for i in 0..job_count {
        let job_type = rand::thread_rng().gen_range(0..5);

        match job_type {
            0 => {
                // Catch Pokemon jobs (40%)
                let pokemon = POKEMON_NAMES[rand::thread_rng().gen_range(0..POKEMON_NAMES.len())];
                let location = LOCATIONS[rand::thread_rng().gen_range(0..LOCATIONS.len())];
                let level = rand::thread_rng().gen_range(1..50);

                let job = CatchPokemonJob {
                    pokemon_name: pokemon.to_string(),
                    pokemon_level: level,
                    location: location.to_string(),
                };

                if job.enqueue(pool).await?.is_some() {
                    enqueued += 1;
                }
            }
            1 => {
                // Training jobs (25%)
                let pokemon = POKEMON_NAMES[rand::thread_rng().gen_range(0..POKEMON_NAMES.len())];
                let methods = [
                    "Battle Training",
                    "Speed Training",
                    "Strength Training",
                    "Agility Training",
                ];
                let method = methods[rand::thread_rng().gen_range(0..methods.len())];

                let job = TrainPokemonJob {
                    pokemon_name: pokemon.to_string(),
                    current_level: rand::thread_rng().gen_range(5..40),
                    training_method: method.to_string(),
                };

                if job.enqueue(pool).await?.is_some() {
                    enqueued += 1;
                }
            }
            2 => {
                // Healing jobs (15%)
                let pokemon_count = rand::thread_rng().gen_range(1..6);
                let mut pokemon_names = Vec::new();
                for _ in 0..pokemon_count {
                    let pokemon =
                        POKEMON_NAMES[rand::thread_rng().gen_range(0..POKEMON_NAMES.len())];
                    pokemon_names.push(pokemon.to_string());
                }

                let severities = ["Minor", "Moderate", "Severe"];
                let severity = severities[rand::thread_rng().gen_range(0..severities.len())];

                let job = HealPokemonJob {
                    pokemon_names,
                    injury_severity: severity.to_string(),
                };

                if job.enqueue(pool).await?.is_some() {
                    enqueued += 1;
                }
            }
            3 => {
                // Gym battles (10%)
                let (leader, gym_type, city) =
                    GYM_LEADERS[rand::thread_rng().gen_range(0..GYM_LEADERS.len())];

                let job = GymBattleJob {
                    gym_leader: leader.to_string(),
                    gym_type: gym_type.to_string(),
                    city: city.to_string(),
                };

                if job.enqueue(pool).await?.is_some() {
                    enqueued += 1;
                }
            }
            4 => {
                // Exploration jobs (10%)
                let (area, terrain) = AREAS[rand::thread_rng().gen_range(0..AREAS.len())];

                let job = ExploreAreaJob {
                    area_name: area.to_string(),
                    terrain_type: terrain.to_string(),
                };

                if job.enqueue(pool).await?.is_some() {
                    enqueued += 1;
                }
            }
            _ => unreachable!(),
        }

        // Add small delay every 20 jobs to make logs more readable
        if i > 0 && i % 20 == 0 {
            sleep(Duration::from_millis(100)).await;
        }
    }

    let elapsed = start.elapsed();
    info!(
        "✅ Enqueued {} jobs in {:.2}s ({:.0} jobs/sec)",
        enqueued,
        elapsed.as_secs_f64(),
        enqueued as f64 / elapsed.as_secs_f64()
    );

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing with compact formatting
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,stress_test=info,workers=info".into()),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .with_target(false) // Remove module path
                .with_thread_ids(false) // Remove thread IDs
                .compact(), // Use compact format
        )
        .init();

    let args = Args::parse();

    info!("Pokemon Stress Test Starting!");
    info!("Configuration:");
    info!("- Jobs to enqueue: {}", args.jobs);
    info!("- Workers per queue: {}", args.workers);
    info!("- Test duration: {}s", args.duration);
    info!("- Skip DB setup: {}", args.skip_db_setup);

    // Setup database
    let (pool, _container) = if args.skip_db_setup {
        connect_existing_database(&args.database_url).await?
    } else {
        setup_database().await?
    };

    // Create trainer context
    let trainer_context = PokemonContext {
        trainer_name: "Ash Ketchum".to_string(),
        gym_badge_count: rand::thread_rng().gen_range(0..8),
    };

    info!(
        "👤 Trainer: {} (Badges: {})",
        trainer_context.trainer_name, trainer_context.gym_badge_count
    );

    // Setup worker queues with different configurations
    info!("⚙️  Setting up worker queues...");
    let runner = Runner::new(pool.clone(), trainer_context.clone())
        .register_job_type::<CatchPokemonJob>()
        .register_job_type::<TrainPokemonJob>()
        .register_job_type::<HealPokemonJob>()
        .register_job_type::<GymBattleJob>()
        .register_job_type::<ExploreAreaJob>()
        .shutdown_when_queue_empty() // Stop when all jobs are processed
        .configure_queue("field_work", |queue| {
            queue
                .num_workers(args.workers)
                .poll_interval(Duration::from_millis(50))
                .jitter(Duration::from_millis(25))
        })
        .configure_queue("training", |queue| {
            queue
                .num_workers(args.workers / 2)
                .poll_interval(Duration::from_millis(100))
                .jitter(Duration::from_millis(50))
        })
        .configure_queue("pokemon_center", |queue| {
            queue
                .num_workers(args.workers / 4)
                .poll_interval(Duration::from_millis(100))
        })
        .configure_queue("gym_battles", |queue| {
            queue
                .num_workers(2)
                .poll_interval(Duration::from_millis(200))
        })
        .configure_queue("exploration", |queue| {
            queue
                .num_workers(args.workers)
                .poll_interval(Duration::from_millis(150))
        });

    info!("✅ Worker configuration complete:");
    info!(
        "   • Field Work: {} workers (catching Pokemon)",
        args.workers
    );
    info!("   • Training: {} workers (leveling up)", args.workers / 2);
    info!(
        "   • Pokemon Center: {} workers (healing)",
        args.workers / 4
    );
    info!("   • Gym Battles: 2 workers (badges)");
    info!("   • Exploration: {} workers (discovering)", args.workers);

    // Enqueue jobs
    enqueue_pokemon_jobs(&pool, args.jobs).await?;

    // Start the runner
    info!("🎮 Starting Pokemon adventure workers...");
    let runner_handle = runner.start();

    // Monitor progress in background while waiting for completion
    let monitor_handle = tokio::spawn({
        let pool = pool.clone();
        async move {
            let _start = Instant::now();
            let mut last_count = 0i64;

            loop {
                tokio::time::sleep(Duration::from_secs(3)).await;

                let current_jobs =
                    match sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM background_jobs")
                        .fetch_one(&pool)
                        .await
                    {
                        Ok(count) => count,
                        Err(_) => break, // Database might be shutting down
                    };

                if current_jobs == 0 {
                    break;
                }

                let failed_jobs = sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM background_jobs WHERE retries > 0",
                )
                .fetch_one(&pool)
                .await
                .unwrap_or(0);

                let processed = last_count - current_jobs;
                info!(
                    "📈 Progress: {} remaining, {} failed (processed {} since last check)",
                    current_jobs,
                    failed_jobs,
                    processed.max(0)
                );

                last_count = current_jobs;
            }
        }
    });

    // Wait for runner to complete all jobs (or timeout as safety)
    tokio::select! {
        _ = runner_handle.wait_for_shutdown() => {
            info!("✅ All workers completed!");
        }
        _ = sleep(Duration::from_secs(args.duration)) => {
            warn!("⏰ Test duration exceeded, workers may still be processing...");
        }
    }

    // Clean up monitoring
    monitor_handle.abort();

    // Final statistics
    let remaining_jobs = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM background_jobs")
        .fetch_one(&pool)
        .await?;
    let failed_jobs =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM background_jobs WHERE retries > 0")
            .fetch_one(&pool)
            .await?;

    info!("📈 Final Statistics:");
    info!("   • Jobs enqueued: {}", args.jobs);
    info!(
        "   • Jobs completed: {}",
        args.jobs - remaining_jobs as usize
    );
    info!("   • Jobs failed: {}", failed_jobs);

    if remaining_jobs > 0 {
        warn!(
            "⚠️  {} jobs were not completed within the time limit",
            remaining_jobs
        );
        info!(
            "   • Success rate: {:.1}%",
            (args.jobs - remaining_jobs as usize) as f64 / args.jobs as f64 * 100.0
        );
    } else {
        info!("🎉 All jobs completed successfully! Gotta catch 'em all!");
    }

    info!("👋 Pokemon stress test completed!");
    Ok(())
}
