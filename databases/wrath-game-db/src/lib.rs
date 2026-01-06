use anyhow::Result;
use std::time::Duration;

mod areatrigger_restedzone;
mod areatrigger_teleport;
mod item_template;
mod player_create_info;

pub use areatrigger_restedzone::DBAreaTriggerRestedZone;
pub use areatrigger_teleport::DBAreaTriggerTeleport;
pub use item_template::DBItemTemplate;
pub use player_create_info::DBPlayerCreateInfo;

pub struct GameDatabase {
    connection_pool: sqlx::MySqlPool,
}

impl GameDatabase {
    pub async fn new(conn_string: &str, timeout: Duration) -> Result<Self> {
        let pool = sqlx::mysql::MySqlPoolOptions::new()
            .max_connections(5)
            .acquire_timeout(timeout)
            .connect(conn_string)
            .await?;

        Ok(Self { connection_pool: pool })
    }
}
