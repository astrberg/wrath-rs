use crate::item_instance::DBItemInstance;
use anyhow::Result;
use sqlx::{MySql, QueryBuilder};

#[derive(Debug)]
pub struct DBCharacterEquipmentDisplayInfo {
    pub slot_id: u8,
    pub inventory_type: Option<u8>,
    pub enchant: Option<u32>,
    pub displayid: Option<u32>,
}

impl super::RealmDatabase {
    pub async fn get_all_character_equipment_display_info(
        &self,
        character_id: u32,
        game_db: &wrath_game_db::GameDatabase,
    ) -> Result<Vec<DBCharacterEquipmentDisplayInfo>> {
        let equipment: Vec<_> = sqlx::query!(
            "SELECT slot_id, item, enchant FROM character_equipment WHERE character_id = ? AND item IS NOT NULL",
            character_id
        )
        .fetch_all(&self.connection_pool)
        .await?;

        if equipment.is_empty() {
            return Ok(Vec::new());
        }

        // Collect all item IDs to fetch their templates in bulk
        let item_ids: Vec<u32> = equipment.iter().map(|e| e.item.unwrap()).collect();
        let item_templates = game_db.get_multiple_item_templates(&item_ids).await?;
        let item_map: std::collections::HashMap<u32, _> = item_templates.into_iter().map(|item| (item.id, item)).collect();
        let result = equipment
            .into_iter()
            .map(|equip| {
                let item_id = equip.item.unwrap();
                let template = item_map.get(&item_id);
                DBCharacterEquipmentDisplayInfo {
                    slot_id: equip.slot_id,
                    inventory_type: template.map(|t| t.inventory_type),
                    enchant: equip.enchant,
                    displayid: template.map(|t| t.displayid),
                }
            })
            .collect();

        Ok(result)
    }

    pub async fn give_character_start_equipment(
        &self,
        character_id: u32,
        item_ids: [i32; 24],
        slot_ids: impl IntoIterator<Item = i32> + Clone,
    ) -> Result<()> {
        #[cfg(debug_assertions)]
        {
            //Cannot already have starting equipment
            assert_eq!(self.get_all_character_equipment(character_id).await?.len(), 0);
            assert_eq!(slot_ids.clone().into_iter().count(), 24);
        }

        //Have to use slightly more complicated query builder syntax to bulk-insert.
        //Bulk insert is vastly faster than for-looping each item and "regular" inserting the items
        //one by one.
        let insert_iter = item_ids.iter().zip(slot_ids).filter_map(|(&item, slot_id)| {
            if item != -1 && slot_id != -1 {
                Some(DBItemInstance {
                    character_id,
                    slot_id: slot_id as u8,
                    item: Some(item as u32),
                    enchant: None,
                })
            } else {
                None
            }
        });

        let mut query_builder: QueryBuilder<MySql> = QueryBuilder::new("INSERT INTO character_equipment (character_id, slot_id, item, enchant) ");
        query_builder.push_values(insert_iter, |mut b, item| {
            b.push_bind(item.character_id)
                .push_bind(item.slot_id)
                .push_bind(item.item)
                .push_bind(item.enchant);
        });

        let query = query_builder.build();
        query.execute(&self.connection_pool).await?;
        Ok(())
    }
}
