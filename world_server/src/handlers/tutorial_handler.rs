use std::net::SocketAddr;

use crate::character::{character_manager::CharacterManager, *};
use crate::client_manager::ClientManager;
use crate::connection::events::ServerEvent;
use crate::prelude::*;
use crate::world::game_object::GameObject;
use crate::world::World;
use bit_field::BitArray;
use wow_world_messages::wrath::{CMSG_TUTORIAL_FLAG, SMSG_TUTORIAL_FLAGS};

pub async fn send_tutorial_flags(character: &Character) -> Result<()> {
    ServerEvent::TutorialFlags(SMSG_TUTORIAL_FLAGS {
        tutorial_data: character.tutorial_flags.flag_data,
    })
    .send_to_character(character)
    .await
}

pub async fn handle_cmsg_tutorial_flag(
    client_manager: &ClientManager,
    character_manager: &mut CharacterManager,
    world: &World,
    client_id: SocketAddr,
    packet: &CMSG_TUTORIAL_FLAG,
) -> Result<()> {
    let client = client_manager.get_authenticated_client(client_id)?;
    let character_guid = client.get_active_character();
    let character = character_manager.get_character_mut(character_guid)?;

    let tut_flag_index = packet.tutorial_flag as usize;

    character.tutorial_flags.set_bit(tut_flag_index, true);

    let tutorial_bytes = character.tutorial_flags.to_bytes();

    let character_id = character.get_guid().guid() as u32;
    let realm_database = world.get_realm_database();
    realm_database.update_character_tutorial_data(character_id, tutorial_bytes).await?;

    warn!("Handled tutorial flag, flags are now: {:?}", character.tutorial_flags.flag_data);

    Ok(())
}

pub async fn handle_cmsg_tutorial_reset(
    client_manager: &ClientManager,
    character_manager: &mut CharacterManager,
    world: &World,
    client_id: SocketAddr,
) -> Result<()> {
    let client = client_manager.get_authenticated_client(client_id)?;
    let character_guid = client.get_active_character();
    let character = character_manager.get_character_mut(character_guid)?;

    character.tutorial_flags.reset();

    let tutorial_bytes = character.tutorial_flags.to_bytes();
    let character_id = character.get_guid().guid() as u32;
    let realm_database = world.get_realm_database();
    realm_database.update_character_tutorial_data(character_id, tutorial_bytes).await?;

    Ok(())
}
