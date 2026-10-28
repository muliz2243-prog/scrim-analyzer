//! Match-link diagnostics: which `gameId` the app sees right now.
//!
//! ```text
//! cargo run --example check_link
//! ```

use std::time::{Duration, SystemTime};
use rofl_analyzer::lcu::{
    find_active_game_id, find_latest_game_id_any_age, find_latest_rofl_game_id,
    find_latest_rofl_game_id_since,
};

fn main() {
    println!("carpetas de logs: {:?}", rofl_analyzer::lcu::game_log_dirs());
    println!("find_active_game_id() = {:?}", find_active_game_id());
    println!(
        "find_latest_rofl_game_id() = {:?}",
        find_latest_rofl_game_id()
    );
    let since = SystemTime::now() - Duration::from_secs(3600);
    println!(
        "find_latest_rofl_game_id_since(última hora) = {:?}",
        find_latest_rofl_game_id_since(since)
    );
    println!(
        "find_latest_game_id_any_age() = {:?}",
        find_latest_game_id_any_age()
    );
}
