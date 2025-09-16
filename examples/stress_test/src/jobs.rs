use anyhow::Result;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::time::sleep;
use tracing::{info, warn};
use workers::BackgroundJob;

/// Shared context for all Pokemon jobs
#[derive(Clone, Debug)]
pub struct PokemonContext {
    pub trainer_name: String,
    pub gym_badge_count: u32,
}

/// A job that catches a wild Pokemon
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CatchPokemonJob {
    pub pokemon_name: String,
    pub pokemon_level: u32,
    pub location: String,
}

impl BackgroundJob for CatchPokemonJob {
    const JOB_TYPE: &'static str = "catch_pokemon";
    const PRIORITY: i16 = 10; // Higher priority for catching!
    const QUEUE: &'static str = "field_work";
    type Context = PokemonContext;

    async fn run(&self, ctx: Self::Context) -> Result<()> {
        let catch_time = Duration::from_millis(rand::thread_rng().gen_range(100..800));
        
        info!(
            "🎯 Attempting to catch {} (Level {}) at {}...",
            self.pokemon_name, self.pokemon_level, self.location
        );

        // Simulate the catching process
        sleep(catch_time).await;

        // Success rate depends on Pokemon level and trainer skill
        let success_rate = if self.pokemon_level <= 10 {
            90.0
        } else if self.pokemon_level <= 25 {
            70.0
        } else {
            50.0 + (ctx.gym_badge_count as f64 * 5.0)
        };

        let roll = rand::thread_rng().gen_range(0.0..100.0);
        
        if roll < success_rate {
            info!(
                "✌️ Successfully caught {}! Added to Pokédex.",
                self.pokemon_name
            );
        } else {
            warn!(
                "💨 {} broke free and escaped!",
                self.pokemon_name
            );
        }

        Ok(())
    }
}

/// A job that trains a Pokemon to level up
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TrainPokemonJob {
    pub pokemon_name: String,
    pub current_level: u32,
    pub training_method: String,
}

impl BackgroundJob for TrainPokemonJob {
    const JOB_TYPE: &'static str = "train_pokemon";
    const PRIORITY: i16 = 5;
    const QUEUE: &'static str = "training";
    const DEDUPLICATED: bool = true; // Don't train the same Pokemon multiple times
    type Context = PokemonContext;

    async fn run(&self, _ctx: Self::Context) -> Result<()> {
        let training_time = Duration::from_millis(rand::thread_rng().gen_range(200..1500));
        
        info!(
            "🏋️‍♂️ Starting {} training for {} (Level {})...",
            self.training_method, self.pokemon_name, self.current_level
        );

        // Simulate training
        sleep(training_time).await;

        let exp_gained = rand::thread_rng().gen_range(50..200);
        let new_level = self.current_level + (exp_gained / 100);

        info!(
            "🧠 {} gained {} EXP and reached level {}!",
            self.pokemon_name, exp_gained, new_level
        );

        Ok(())
    }
}

/// A job that heals Pokemon at the Pokemon Center
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct HealPokemonJob {
    pub pokemon_names: Vec<String>,
    pub injury_severity: String,
}

impl BackgroundJob for HealPokemonJob {
    const JOB_TYPE: &'static str = "heal_pokemon";
    const PRIORITY: i16 = 15; // Healing is urgent!
    const QUEUE: &'static str = "pokemon_center";
    type Context = PokemonContext;

    async fn run(&self, _ctx: Self::Context) -> Result<()> {
        let healing_time = Duration::from_millis(
            self.pokemon_names.len() as u64 * rand::thread_rng().gen_range(100..400)
        );
        
        info!(
            "🏥 Nurse Joy is healing {} Pokemon ({} injuries)...",
            self.pokemon_names.len(), self.injury_severity
        );

        // Simulate healing process
        sleep(healing_time).await;

        info!(
            "✨ All Pokemon have been fully healed! They're ready for adventure again."
        );

        Ok(())
    }
}

/// A job that battles a gym leader
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct GymBattleJob {
    pub gym_leader: String,
    pub gym_type: String,
    pub city: String,
}

impl BackgroundJob for GymBattleJob {
    const JOB_TYPE: &'static str = "gym_battle";
    const PRIORITY: i16 = 20; // Gym battles are the highest priority!
    const QUEUE: &'static str = "gym_battles";
    type Context = PokemonContext;

    async fn run(&self, ctx: Self::Context) -> Result<()> {
        let battle_time = Duration::from_millis(rand::thread_rng().gen_range(1000..3000));
        
        info!(
            "💥️  Challenging {} ({} Gym) in {} City!",
            self.gym_leader, self.gym_type, self.city
        );

        // Simulate intense battle
        sleep(battle_time).await;

        // Battle outcome based on trainer experience
        let win_chance = 50.0 + (ctx.gym_badge_count as f64 * 8.0);
        let roll = rand::thread_rng().gen_range(0.0..100.0);

        if roll < win_chance {
            info!(
                "🏆 Victory! {} earned the {} Badge from {}!",
                ctx.trainer_name, self.gym_type, self.gym_leader
            );
        } else {
            warn!(
                "😤 Defeated by {}! Need more training before the next attempt.",
                self.gym_leader
            );
        }

        Ok(())
    }
}

/// A job that explores a new area and discovers Pokemon
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ExploreAreaJob {
    pub area_name: String,
    pub terrain_type: String,
}

impl BackgroundJob for ExploreAreaJob {
    const JOB_TYPE: &'static str = "explore_area";
    const PRIORITY: i16 = 3;
    const QUEUE: &'static str = "exploration";
    type Context = PokemonContext;

    async fn run(&self, ctx: Self::Context) -> Result<()> {
        let exploration_time = Duration::from_millis(rand::thread_rng().gen_range(300..1200));
        
        info!(
            "🗺️  {} is exploring {} ({} terrain)...",
            ctx.trainer_name, self.area_name, self.terrain_type
        );

        // Simulate exploration
        sleep(exploration_time).await;

        let discoveries = rand::thread_rng().gen_range(1..5);
        let pokemon_types = match self.terrain_type.as_str() {
            "forest" => vec!["Caterpie", "Pidgey", "Oddish", "Bellsprout"],
            "cave" => vec!["Zubat", "Geodude", "Onix", "Machop"],
            "water" => vec!["Magikarp", "Psyduck", "Tentacool", "Staryu"],
            "mountain" => vec!["Spearow", "Ekans", "Sandshrew", "Mankey"],
            _ => vec!["Rattata", "Pidgey", "Spearow"],
        };

        for _i in 0..discoveries {
            let pokemon = pokemon_types[rand::thread_rng().gen_range(0..pokemon_types.len())];
            info!(
                "👀 Discovered a wild {}!",
                pokemon
            );
        }

        info!(
            "🌟 Exploration complete! Found {} new Pokemon in {}.",
            discoveries, self.area_name
        );

        Ok(())
    }
}