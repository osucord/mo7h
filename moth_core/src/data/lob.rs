use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};

use std::sync::{OnceLock, RwLock};

use crate::data::structs::{Context, Error};
use rand::seq::IteratorRandom;

pub const LOB_PATH: &str = "config/lists/loblist.txt";

fn get_loblist() -> &'static RwLock<HashSet<String>> {
    static LOBLIST: OnceLock<RwLock<HashSet<String>>> = OnceLock::new();
    LOBLIST.get_or_init(|| {
        let data = std::fs::read_to_string(LOB_PATH).unwrap_or_else(|_| String::new());
        let words: HashSet<String> = data.lines().map(String::from).collect();
        RwLock::new(words)
    })
}

#[must_use]
pub fn get_random_lob() -> Option<String> {
    let loblist = get_loblist().read().unwrap();

    let mut rng = rand::rng();
    loblist.iter().choose(&mut rng).cloned()
}

pub fn update_lob() -> Result<(usize, usize), Error> {
    let new_lob = std::fs::read_to_string(LOB_PATH)?;
    let old_count;

    let lines: HashSet<String> = new_lob
        .lines()
        .map(std::string::ToString::to_string)
        .collect();
    let new_count = lines.len();

    {
        let mut loblist = get_loblist().write().unwrap();
        old_count = loblist.len();
        *loblist = lines;
    }

    Ok((old_count, new_count))
}

pub fn unload_lob() -> Result<(), Error> {
    let mut loblist = get_loblist().write().unwrap();
    *loblist = HashSet::new();

    Ok(())
}

pub fn add_lob(content: &str) -> Result<(), Error> {
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .open(LOB_PATH)?;

    let content = if content.starts_with('\n') || content.is_empty() {
        content.to_string()
    } else {
        format!("\n{content}")
    };
    file.write_all(content.as_bytes())?;

    let file = std::fs::File::open(LOB_PATH)?;
    let reader = BufReader::new(file);
    let mut unique_lines = HashSet::new();
    let deduplicated_lines: Vec<String> = reader
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| {
            let trimmed = line.trim().to_string();
            if trimmed.is_empty() || !unique_lines.insert(trimmed.clone()) {
                None
            } else {
                Some(trimmed)
            }
        })
        .collect();

    std::fs::write(LOB_PATH, deduplicated_lines.join("\n"))?;

    Ok(())
}

pub fn remove_lob(target: &str) -> Result<bool, Error> {
    let mut lines = Vec::new();
    let mut line_removed = false;

    let file = File::open(LOB_PATH)?;
    let reader = BufReader::new(file);

    for line in reader.lines() {
        let line = line?;
        if line.trim() == target {
            line_removed = true;
        } else {
            lines.push(line);
        }
    }

    if line_removed {
        let mut file = File::create(LOB_PATH)?;
        for line in lines {
            writeln!(file, "{line}")?;
        }
    }

    Ok(line_removed)
}

pub fn count_lob() -> Result<usize, Error> {
    let file = File::open(LOB_PATH)?;
    let reader = BufReader::new(file);

    Ok(reader.lines().count())
}

// A check for Trash, so he can refresh the loblist. Includes me because, well I'm me.
// Also includes a few gg/osu mods because well why not!
#[allow(clippy::unused_async)]
pub async fn trontin(ctx: Context<'_>) -> Result<bool, Error> {
    let allowed_users = [291089948709486593, 288054604548276235]; // me, trontin
    let user_id = ctx.author().id.get();
    if allowed_users.contains(&user_id) {
        return Ok(true);
    }

    Err("You are not worthy of the lob.".into())
}
