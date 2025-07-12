use std::{pin::Pin, task::Poll};

use rosu_v2::prelude::{GameMode, UserExtended};
use serenity::futures::FutureExt;
use sqlx::query;
use tokio::time::Timeout;

use crate::data::structs::Error;
use chrono::Utc;
use serenity::all::{RoleId, UserId};

pub struct WaitForOsuAuth {
    pub state: u8,
    fut: Pin<Box<Timeout<tokio::sync::oneshot::Receiver<UserExtended>>>>,
}
pub enum AuthenticationStandbyError {
    Canceled,
    Timeout,
}

impl Future for WaitForOsuAuth {
    type Output = Result<UserExtended, AuthenticationStandbyError>;

    #[inline]
    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        match self.fut.poll_unpin(cx) {
            Poll::Ready(Ok(Ok(user))) => Poll::Ready(Ok(user)),
            Poll::Ready(Ok(Err(_))) => Poll::Ready(Err(AuthenticationStandbyError::Canceled)),
            Poll::Ready(Err(_)) => Poll::Ready(Err(AuthenticationStandbyError::Timeout)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl super::Database {
    pub async fn get_gamemode(&self, user_id: UserId, osu_id: u32) -> Result<GameMode, Error> {
        let res = query!(
            "SELECT gamemode FROM verified_users WHERE user_id = $1 AND osu_id = $2",
            &self.get_user(user_id).await?.id,
            osu_id as i32
        )
        .fetch_one(&self.db)
        .await?;

        Ok((res.gamemode as u8).into())
    }

    pub async fn inactive_user(&self, user_id: UserId) -> Result<(), Error> {
        Ok(query!(
            "UPDATE verified_users SET is_active = FALSE WHERE user_id = $1",
            &self.get_user(user_id).await?.id,
        )
        .execute(&self.db)
        .await
        .map(|_| ())?)
    }

    pub async fn update_last_updated(
        &self,
        user_id: UserId,
        time: chrono::DateTime<chrono::Utc>,
        rank: Option<Option<u32>>,
        map_status: u8,
        roles: &[RoleId],
    ) -> Result<(), Error> {
        if let Some(rank) = rank {
            query!(
                "UPDATE verified_users SET last_updated = $2, rank = $3, map_status = $4, \
                 verified_roles = $5 WHERE user_id = $1",
                &self.get_user(user_id).await?.id,
                time,
                rank.map(|r| r as i32),
                i16::from(map_status),
                &roles.iter().map(|r| r.get() as i64).collect::<Vec<_>>(),
            )
            .execute(&self.db)
            .await?;
        } else {
            query!(
                "UPDATE verified_users SET last_updated = $2, map_status = $3, verified_roles = \
                 $4 WHERE user_id = $1",
                &self.get_user(user_id).await?.id,
                time,
                i16::from(map_status),
                &roles.iter().map(|r| r.get() as i64).collect::<Vec<_>>(),
            )
            .execute(&self.db)
            .await?;
        }

        Ok(())
    }

    pub async fn verify_user(&self, user_id: UserId, osu_id: u32) -> Result<(), Error> {
        let now = Utc::now();

        let user = self.get_user(user_id).await?;

        query!(
            r#"
            INSERT INTO verified_users (user_id, osu_id, last_updated, is_active, gamemode)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (user_id)
            DO UPDATE SET
                last_updated = EXCLUDED.last_updated,
                is_active = EXCLUDED.is_active,
                gamemode = 0
            "#,
            user.id,
            osu_id as i32,
            now,
            true,
            0
        )
        .execute(&self.db)
        .await?;

        Ok(())
    }

    pub async fn unlink_user(&self, user_id: UserId) -> Result<u32, Error> {
        let record = query!(
            "DELETE FROM verified_users WHERE user_id = $1 RETURNING osu_id",
            &self.get_user(user_id).await?.id,
        )
        .fetch_optional(&self.db)
        .await?;

        Ok(record.unwrap().osu_id as u32)
    }

    pub async fn get_osu_user_id(&self, user_id: UserId) -> Option<(u32, GameMode)> {
        let query = query!(
            "SELECT osu_id, gamemode FROM verified_users WHERE user_id = $1",
            &self.get_user(user_id).await.ok()?.id,
        )
        .fetch_one(&self.db)
        .await
        .ok()?;

        Some((query.osu_id as u32, (query.gamemode as u8).into()))
    }

    pub async fn change_mode(&self, user_id: UserId, gamemode: GameMode) -> Result<(), Error> {
        query!(
            "UPDATE verified_users SET gamemode = $1 WHERE user_id = $2",
            gamemode as i16,
            &self.get_user(user_id).await?.id,
        )
        .execute(&self.db)
        .await?;

        Ok(())
    }

    pub async fn get_existing_links(&self, osu_id: u32) -> Result<Vec<UserId>, sqlx::Error> {
        sqlx::query_scalar!(
            "SELECT u.user_id FROM verified_users vu JOIN users u ON vu.user_id = u.id WHERE \
             vu.osu_id = $1",
            osu_id as i32
        )
        .fetch_all(&self.db)
        .await
        .map(|user_ids| {
            user_ids
                .into_iter()
                .map(|id| UserId::new(id as u64))
                .collect()
        })
    }
}
