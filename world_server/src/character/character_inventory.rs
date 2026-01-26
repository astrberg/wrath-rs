use crate::connection::events::ServerEvent;
use crate::item::item_container::ItemContainer;
use crate::world::prelude::GameObject;
use crate::{
    item::Item,
    prelude::*,
    world::prelude::inventory::{self, get_compatible_equipment_slots_for_inventory_type, BagSlot, EquipmentSlot, BAG_SLOTS_END},
};
use std::{
    collections::HashMap,
    fmt::Display,
    ops::{Index, IndexMut},
};
use wow_world_base::wrath::ItemSlot;
use wow_world_messages::wrath::UpdateItem;
use wow_world_messages::wrath::{
    InventoryType, MovementBlock, MovementBlock_UpdateFlag, Object, ObjectType, Object_UpdateType, UpdateItemBuilder, UpdateMask, VisibleItem,
    VisibleItemIndex, SMSG_UPDATE_OBJECT,
};

//An identifier for the player inventory (the thing ItemSlot models a cell of)
pub const INVENTORY_SLOT_BAG_0: u8 = 255;
//Models anything that can be stored in the inventory
pub trait InventoryStorable: Display {
    fn get_inventory_type(&self) -> InventoryType;
}

// Struct that can be used to model the character equipment slots and their fillings
// Generic so that it can be used in different cases (simple case during character creation, or
// full feature version during gameplay)
pub struct CharacterInventory<ItemType: InventoryStorable> {
    items: HashMap<EquipmentSlot, ItemType>,
}

impl<ItemType: InventoryStorable> Default for CharacterInventory<ItemType> {
    fn default() -> Self {
        Self::new()
    }
}

impl<ItemType: InventoryStorable> CharacterInventory<ItemType> {
    pub fn new() -> Self {
        Self { items: HashMap::new() }
    }

    //Tries to insert item, returns Ok(inserted slot) if successful
    pub fn try_insert_item(&mut self, item: ItemType) -> Result<EquipmentSlot> {
        let inventory_type = item.get_inventory_type();
        let possible_slots = get_compatible_equipment_slots_for_inventory_type(&inventory_type);

        for &possible_slot in possible_slots {
            if let std::collections::hash_map::Entry::Vacant(e) = self.items.entry(possible_slot) {
                e.insert(item);
                return Ok(possible_slot);
            }
        }
        bail!("No free slots to put item {}", item);
    }

    pub fn get_item(&self, slot: EquipmentSlot) -> Option<&ItemType> {
        self.items.get(&slot)
    }

    pub fn take_item(&mut self, slot: EquipmentSlot) -> Option<ItemType> {
        self.items.remove(&slot)
    }

    pub fn get_all_equipment(&self) -> [Option<&ItemType>; (BAG_SLOTS_END + 1) as usize] {
        let mut result = [None; (BAG_SLOTS_END + 1) as usize];
        for (slot, item) in self.items.iter() {
            result[*slot as usize] = Some(item);
        }
        result
    }
}

#[derive(Copy, Clone)]
pub struct SimpleItemDescription {
    pub item_id: u32,
    pub inventory_type: InventoryType,
}

impl Display for SimpleItemDescription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "SimpleItemDescription {{ item_id: {}, inventory_type: {} }}",
            self.item_id, self.inventory_type
        )
    }
}

impl InventoryStorable for SimpleItemDescription {
    fn get_inventory_type(&self) -> InventoryType {
        self.inventory_type
    }
}

pub type SimpleCharacterInventory = CharacterInventory<SimpleItemDescription>;
pub type GameplayCharacterInventory = CharacterInventory<Item>;

#[derive(Default)]
pub struct BagInventory {
    items: [Option<Item>; 16],
}
impl Index<BagSlot> for BagInventory {
    type Output = Option<Item>;

    fn index(&self, index: BagSlot) -> &Self::Output {
        &self.items[Self::to_index(&index)]
    }
}
impl IndexMut<BagSlot> for BagInventory {
    fn index_mut(&mut self, index: BagSlot) -> &mut Self::Output {
        &mut self.items[Self::to_index(&index)]
    }
}

impl BagInventory {
    fn to_index(slot: &BagSlot) -> usize {
        (*slot as usize) - (BagSlot::Item1 as usize)
    }
    fn take_item(&mut self, index: BagSlot) -> Option<Item> {
        let index = Self::to_index(&index);
        if self.items[index].is_some() {
            self.items[index].take()
        } else {
            None
        }
    }

    pub fn get_create_objects(&self) -> Vec<Object> {
        self.items
            .iter()
            .flatten()
            .map(|item| Object {
                update_type: Object_UpdateType::CreateObject {
                    guid3: item.update_state.object_guid().unwrap(),
                    mask2: UpdateMask::Item(item.update_state.clone()),
                    movement2: MovementBlock {
                        update_flag: MovementBlock_UpdateFlag::empty(),
                    },
                    object_type: ObjectType::Item,
                },
            })
            .collect()
    }
}

impl ItemContainer<BagSlot> for BagInventory {
    fn get_items_update_state(&self) -> Vec<UpdateItem> {
        let mut updates = Vec::new();
        for item in self.items.iter().flatten() {
            updates.push(item.update_state.clone());
        }
        updates
    }
}

impl crate::character::Character {
    async fn send_item_update(item: &Item, connection_sender: &flume::Sender<ServerEvent>) {
        let object = Object {
            update_type: Object_UpdateType::CreateObject {
                guid3: item.update_state.object_guid().unwrap(),
                mask2: UpdateMask::Item(item.update_state.clone()),
                movement2: MovementBlock {
                    update_flag: MovementBlock_UpdateFlag::empty(),
                },
                object_type: ObjectType::Item,
            },
        };
        let msg = SMSG_UPDATE_OBJECT { objects: vec![object] };
        let _ = connection_sender.send_async(ServerEvent::UpdateObject(msg)).await;
    }

    //This function is meant to be used both with inventory and equipment or bags
    //It sets the item in the slot, and returns the old item if there was one
    //Doesn't check if the item is compatible with the slot
    //Slot is u16 because lower 8 bits contain slot data and upper 8 bits contain bag data
    pub async fn set_item(
        &mut self,
        item: Option<Item>,
        item_position: (u8, u8),
        realm_db: Option<&wrath_realm_db::RealmDatabase>,
        connection_sender: Option<&flume::Sender<ServerEvent>>,
    ) -> Result<Option<Item>> {
        let (slot, bag) = item_position;

        if bag != INVENTORY_SLOT_BAG_0 {
            todo!("Bags not implemented yet");
        }

        let character_id = self.get_guid().guid() as u32;

        if let Ok(equipment_slot) = EquipmentSlot::try_from(slot) {
            self.set_equipment_item(item, slot, equipment_slot, character_id, realm_db, connection_sender)
                .await
        } else if let Ok(bag_slot) = inventory::BagSlot::try_from(slot) {
            self.set_bag_item(item, slot, bag_slot, character_id, realm_db, connection_sender).await
        } else {
            todo!("Non-equipment inventory not implemented yet")
        }
    }

    /// Rebuild the item's `UpdateItem` with a new object GUID.
    ///
    /// The client associates item objects with inventory/equipment cells using the
    /// object's GUID. We encode the GUID as a 64-bit value where:
    /// - high 32 bits = character id
    /// - low  32 bits = slot id (bag/equipment slot)
    ///
    /// Example (binary):
    /// - character_id = 10 => high 32 bits = 00000000 00000000 00000000 00001010
    /// - slot         = 3  => low  32 bits = 00000000 00000000 00000000 00000011
    /// - GUID (binary) = <high 32 bits> <low 32 bits>
    ///   00000000 00000000 00000000 00001010 00000000 00000000 00000000 00000011
    ///
    /// Constructed in code as: ((character_id as u64) << 32) | (slot as u64).
    /// Rebuilding preserves other fields (entry, owner, stack, durability) because
    /// `UpdateItem` does not expose setters for those fields.
    fn set_item_guid(item: &mut Item, guid: Guid) {
        let entry = item.update_state.object_entry().unwrap_or(0);
        let scale = item.update_state.object_scale_x().unwrap_or(1.0);
        let owner = item.update_state.item_owner().unwrap_or(Guid::zero());
        let contained = item.update_state.item_contained().unwrap_or(Guid::zero());
        let stack = item.update_state.item_stack_count().unwrap_or(1);
        let durability = item.update_state.item_durability().unwrap_or(100);
        let maxdur = item.update_state.item_maxdurability().unwrap_or(100);

        item.update_state = UpdateItemBuilder::new()
            .set_object_guid(guid)
            .set_object_entry(entry)
            .set_object_scale_x(scale)
            .set_item_owner(owner)
            .set_item_contained(contained)
            .set_item_stack_count(stack)
            .set_item_durability(durability)
            .set_item_maxdurability(maxdur)
            .finalize();
    }

    async fn set_equipment_item(
        &mut self,
        item: Option<Item>,
        slot: u8,
        equipment_slot: EquipmentSlot,
        character_id: u32,
        realm_db: Option<&wrath_realm_db::RealmDatabase>,
        connection_sender: Option<&flume::Sender<ServerEvent>>,
    ) -> Result<Option<Item>> {
        let previous_item = self.equipped_items.take_item(equipment_slot);

        match item {
            Some(mut item) => {
                let item_id = item.update_state.object_entry().unwrap() as u32;

                let new_guid = ((character_id as u64) << 32 | slot as u64).into();
                Self::set_item_guid(&mut item, new_guid);

                self.update_visible_item(slot, &item);
                self.update_inventory_field(slot, new_guid);

                if let Some(sender) = connection_sender {
                    Self::send_item_update(&item, sender).await;
                }

                self.equipped_items.try_insert_item(item)?;

                if let Some(db) = realm_db {
                    let _ = db.delete_character_item(character_id, slot).await;
                    let _ = db.insert_character_item(character_id, slot, item_id).await;
                }
            }
            None => {
                self.clear_visible_item(slot);
                self.update_inventory_field(slot, Guid::zero());
                if let Some(db) = realm_db {
                    let _ = db.delete_character_item(character_id, slot).await;
                }
            }
        }

        Ok(previous_item)
    }

    async fn set_bag_item(
        &mut self,
        item: Option<Item>,
        slot: u8,
        bag_slot: BagSlot,
        character_id: u32,
        realm_db: Option<&wrath_realm_db::RealmDatabase>,
        connection_sender: Option<&flume::Sender<ServerEvent>>,
    ) -> Result<Option<Item>> {
        let previous_item = self.bag_items.take_item(bag_slot);

        if let Some(mut item) = item {
            let item_id = item.update_state.object_entry().unwrap() as u32;

            let new_guid = ((character_id as u64) << 32 | slot as u64).into();
            Self::set_item_guid(&mut item, new_guid);

            if let Some(sender) = connection_sender {
                Self::send_item_update(&item, sender).await;
            }

            if let Some(db) = realm_db {
                let _ = db.delete_character_item(character_id, slot).await;
                let _ = db.insert_character_item(character_id, slot, item_id).await;
            }

            let guid = new_guid;
            self.update_inventory_field(slot, guid);
            self.bag_items[bag_slot] = Some(item);
        } else {
            if let Some(db) = realm_db {
                let _ = db.delete_character_item(character_id, slot).await;
            }
            self.update_inventory_field(slot, Guid::zero());
            self.bag_items[bag_slot] = None;
        }

        Ok(previous_item)
    }

    fn update_visible_item(&mut self, slot: u8, item: &Item) {
        if slot <= inventory::EQUIPMENT_SLOTS_END {
            //TODO: add display enchants
            let visible_item = VisibleItem::new(item.update_state.object_entry().unwrap() as u32, [0u16; 2]);
            self.gameplay_data
                .set_player_visible_item(visible_item, VisibleItemIndex::try_from(slot).unwrap());
        }
    }

    fn clear_visible_item(&mut self, slot: u8) {
        if slot <= inventory::EQUIPMENT_SLOTS_END {
            self.gameplay_data
                .set_player_visible_item(VisibleItem::new(0u32, [0u16; 2]), VisibleItemIndex::try_from(slot).unwrap());
        }
    }

    fn update_inventory_field(&mut self, slot: u8, guid: Guid) {
        self.gameplay_data.set_player_field_inv(ItemSlot::try_from(slot).unwrap(), guid);
    }

    /// Equips an item from inventory to the first compatible equipment slot.
    /// Returns the previously equipped item from that slot, if any.
    pub async fn auto_equip_item_from_bag(
        &mut self,
        item_position: (u8, u8),
        realm_db: Option<&wrath_realm_db::RealmDatabase>,
        connection_sender: Option<&flume::Sender<ServerEvent>>,
    ) -> Result<Option<Item>> {
        let (slot, bag) = item_position;

        if bag != INVENTORY_SLOT_BAG_0 {
            todo!("Bags not implemented yet");
        }

        let bag_slot = inventory::BagSlot::try_from(slot)?;
        let Some(item) = self.bag_items.take_item(bag_slot) else {
            bail!("No item in that slot");
        };

        // Remove previous DB entry for that bag slot since item is being moved
        let character_id = self.get_guid().guid() as u32;
        if let Some(db) = realm_db {
            let _ = db.delete_character_item(character_id, bag_slot as u8).await;
        }

        let item_inventory = item.get_inventory_type();

        let possible_slots = get_compatible_equipment_slots_for_inventory_type(&item_inventory);

        // Find first empty slot, or fall back to the first available slot
        let Some(&target_slot) = possible_slots
            .iter()
            .find(|&&slot| self.equipped_items.get_item(slot).is_none())
            .or_else(|| possible_slots.first())
        else {
            bail!("No compatible equipment slot");
        };

        self.set_item(Some(item), (target_slot as u8, INVENTORY_SLOT_BAG_0), realm_db, connection_sender)
            .await
    }

    // Try to add item to first available backpack slot (BagSlot::Item1-Item16)
    pub async fn try_add_item_to_backpack(
        &mut self,
        item_id: u32,
        character_id: u32,
        connection_sender: &flume::Sender<ServerEvent>,
        realm_db: Option<&wrath_realm_db::RealmDatabase>,
    ) -> Option<u8> {
        for slot_id in (BagSlot::Item1 as u8)..=(BagSlot::Item16 as u8) {
            let bag_slot = BagSlot::try_from(slot_id).unwrap();
            if self.bag_items[bag_slot].is_some() {
                continue;
            }

            let item = Item {
                update_state: UpdateItemBuilder::new()
                    .set_object_guid(((character_id as u64) << 32 | slot_id as u64).into())
                    .set_object_entry(item_id as i32)
                    .set_object_scale_x(1.0)
                    .set_item_owner(Guid::new(character_id as u64))
                    .set_item_contained(Guid::new(character_id as u64))
                    .set_item_stack_count(1)
                    .set_item_durability(100)
                    .set_item_maxdurability(100)
                    .finalize(),
            };

            if self
                .set_item(Some(item), (slot_id, INVENTORY_SLOT_BAG_0), realm_db, Some(connection_sender))
                .await
                .is_ok()
            {
                return Some(slot_id);
            }
        }

        None
    }
}
