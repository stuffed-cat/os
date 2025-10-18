//! Prototype `history` command that replays entries provided via environment variables.

use std::env;

fn main() {
    let history_blob = env::var("SHELL_HISTORY").unwrap_or_default();
    if history_blob.trim().is_empty() {
        println!("history: no recorded commands available");
        return;
    }

    for entry in history_blob.split('\n') {
        if entry.is_empty() {
            continue;
        }
        println!("{entry}");
    }
}
