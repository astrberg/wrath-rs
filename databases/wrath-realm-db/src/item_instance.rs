use anyhow::Result;

pub struct DBItemInstance {
    pub character_id: u32,
    pub slot_id: u8,
    pub item: Option<u32>,
    pub enchant: Option<u32>,
}

impl super::RealmDatabase {
    pub async fn get_all_character_equipment(&self, character_id: u32) -> Result<Vec<DBItemInstance>> {
        let res = sqlx::query_as!(DBItemInstance, "SELECT * FROM character_equipment WHERE character_id = ?", character_id)
            .fetch_all(&self.connection_pool)
            .await?;

        Ok(res)
    }

    pub async fn insert_character_item(&self, character_id: u32, slot_id: u8, item_id: u32) -> Result<()> {
        sqlx::query!(
            "INSERT INTO character_equipment (character_id, slot_id, item, enchant) VALUES (?, ?, ?, NULL)",
            character_id,
            slot_id,
            item_id
        )
        .execute(&self.connection_pool)
        .await?;
        Ok(())
    }

    pub async fn delete_character_item(&self, character_id: u32, slot_id: u8) -> Result<()> {
        sqlx::query!(
            "DELETE FROM character_equipment WHERE character_id = ? AND slot_id = ?",
            character_id,
            slot_id
        )
        .execute(&self.connection_pool)
        .await?;
        Ok(())
    }
}
