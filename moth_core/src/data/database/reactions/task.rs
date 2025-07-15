use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use serenity::futures::StreamExt;
use sqlx::QueryBuilder;
use tokio_util::time::{DelayQueue, delay_queue::Key};

use crate::data::{
    database::{
        Database, EmoteUsageType,
        reactions::{EmoteCommand, EmoteUsage},
    },
    structs::Error,
};

pub(super) async fn start(
    database: Arc<Database>,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<EmoteCommand>,
) {
    let mut delay_queue = DelayQueue::new();
    let mut keys = HashMap::new();
    let mut batch = Vec::new();
    let mut pending_db_removals = Vec::new();
    let mut batch_started: Option<Instant> = None;

    let mut interval = tokio::time::interval(Duration::from_secs(1));

    loop {
        tokio::select! {
            Some(cmd) = rx.recv() => {
                // exit the task, we have shutdown
                if !handle_command(cmd, &mut delay_queue, &mut keys, &mut batch, &mut pending_db_removals) {
                    if should_flush_batch(&batch, batch_started) {
                        // TODO: log
                        let _ = flush_batch(&batch, &pending_db_removals, &database).await;
                    }

                    break
                }
            },
            Some(expired) = delay_queue.next() => {
                let meta = expired.into_inner();
                keys.remove(&meta);
                if batch.is_empty() {
                    batch_started = Some(Instant::now());
                }
                batch.push(meta);
            },
            _ = interval.tick() => {
                if should_flush_batch(&batch, batch_started) {
                    // TODO: log
                    let _ = flush_batch(&batch, &pending_db_removals, &database).await;
                    batch.clear();
                    pending_db_removals.clear();
                    batch_started = None;
                }
            }
        }
    }
}

fn should_flush_batch(batch: &[EmoteUsage], batch_started: Option<Instant>) -> bool {
    if batch.is_empty() {
        return false;
    }

    let batch_age = batch_started.map_or(Duration::ZERO, |t| t.elapsed());

    batch.len() >= 10 || batch_age >= Duration::from_secs(2)
}

fn handle_command(
    cmd: EmoteCommand,
    delay_queue: &mut DelayQueue<EmoteUsage>,
    keys: &mut HashMap<EmoteUsage, Key>,
    batch: &mut Vec<EmoteUsage>,
    pending_db_removals: &mut Vec<EmoteUsage>,
) -> bool {
    match cmd {
        EmoteCommand::ReactionAdd(reaction) => {
            let key = delay_queue.insert(reaction.clone(), Duration::from_millis(800));
            keys.insert(reaction, key);
        }
        EmoteCommand::ReactionRemove(reaction) => {
            // we don't remove messages at this time.
            if reaction.kind == EmoteUsageType::Message {
                return true;
            }

            if let Some(key) = keys.remove(&reaction) {
                delay_queue.remove(&key);
            } else if let Some(pos) = batch.iter().position(|r| *r == reaction) {
                batch.remove(pos);
            } else {
                pending_db_removals.push(reaction);
            }
        }
        EmoteCommand::Shutdown => return false,
    }
    true // Keep running
}

async fn flush_batch(
    batch: &[EmoteUsage],
    pending_db_removals: &[EmoteUsage],
    database: &Database,
) -> Result<(), Error> {
    use sqlx::Postgres;

    if !batch.is_empty() {
        let mut query_builder = QueryBuilder::<Postgres>::new(
            "INSERT INTO emote_usage (message_id, guild_id, channel_id, emote_id, user_id, \
             used_at, usage_type) ",
        );

        // TODO: use a reduced struct
        let mut values = Vec::new();

        for reaction in batch {
            let Some(message_author_id) = reaction.message_author_id else {
                continue;
            };

            let message_data = database
                .get_message(
                    reaction.message,
                    reaction.channel,
                    Some(reaction.guild),
                    message_author_id,
                )
                .await?;

            let user_id = database.get_user(reaction.user).await?.id;
            let emote_id = database.get_emote_id(&reaction.reaction_type).await?;

            values.push((
                message_data.id,
                message_data.guild_id,
                message_data.channel_id,
                emote_id,
                user_id,
                reaction.now,
                reaction.kind,
            ));
        }

        query_builder.push_values(values, |mut b, value| {
            b.push_bind(value.0) // message_id
                .push_bind(value.1) // guild_id
                .push_bind(value.2) // channel_id
                .push_bind(value.3) // emote_id
                .push_bind(value.4) // user_id
                .push_bind(value.5) // used_at
                .push_bind(value.6); // usage_type
        });

        query_builder.push(" ON CONFLICT DO NOTHING");

        query_builder.build().execute(&database.db).await?;
    }

    if !pending_db_removals.is_empty() {
        let mut query_builder = QueryBuilder::<Postgres>::new(
            "DELETE FROM emote_usage WHERE (message_id, emote_id, user_id) IN (",
        );

        // ditto
        let mut values = Vec::new();

        for reaction in pending_db_removals {
            let Ok(message_data) = database.get_message_dataless(reaction.message).await else {
                continue;
            };

            let user_id = database.get_user(reaction.user).await?.id;
            let emote_id = database.get_emote_id(&reaction.reaction_type).await?;

            values.push((message_data.id, emote_id, user_id));
        }

        query_builder.push_values(values, |mut b, value| {
            b.push_bind(value.0) // message_id
                .push_bind(value.1) // emote_id
                .push_bind(value.2); // user_id
        });

        query_builder.push(")");

        query_builder.build().execute(&database.db).await?;
    }

    Ok(())
}
