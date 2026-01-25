use std::{net::SocketAddr, sync::Arc};

use crate::{
    character::character_manager::CharacterManager, client_manager::ClientManager, connection::events::ServerEvent, prelude::*,
    world::prelude::GameObject,
};
use wow_world_messages::wrath::{
    Language, PlayerChatTag, SMSG_MESSAGECHAT_ChatType, CMSG_GMTICKET_CREATE, SMSG_FORCE_RUN_BACK_SPEED_CHANGE, SMSG_FORCE_RUN_SPEED_CHANGE,
    SMSG_GMTICKET_GETTICKET, SMSG_GMTICKET_SYSTEMSTATUS, SMSG_MESSAGECHAT,
};

async fn send_system_message(
    client_manager: &ClientManager,
    character_manager: &CharacterManager,
    client_id: SocketAddr,
    message: &str,
) -> Result<()> {
    let client = client_manager.get_authenticated_client(client_id)?;
    let guid = client.get_active_character();
    let character = character_manager.get_character(guid)?;

    let msg = SMSG_MESSAGECHAT {
        chat_type: SMSG_MESSAGECHAT_ChatType::System {
            target6: character.get_guid(),
        },
        language: Language::Universal,
        sender: character.get_guid(),
        flags: 0,
        message: message.to_string(),
        tag: PlayerChatTag::None,
    };
    let event = ServerEvent::MessageChat(msg);
    client.connection_sender.send_async(event).await?;
    Ok(())
}

pub async fn handle_cmsg_gmticket_getticket(client_manager: &ClientManager, client_id: SocketAddr) -> Result<()> {
    let client = client_manager.get_authenticated_client(client_id)?;

    /*
    //Commented away because this adds an annoying bar to the client in the top-right corner
    //Used just for testing purposes to see if the gm ticket messages are correct, keep around for
    //reference.
    let ticket_status = wow_world_messages::wrath::SMSG_GMTICKET_GETTICKET_GmTicketStatus::HasText {
        days_since_last_updated: 1.0,
        days_since_oldest_ticket_creation: 2.0,
        days_since_ticket_creation: 0.5,
        escalation_status: wow_world_messages::wrath::GmTicketEscalationStatus::GmticketAssignedtogmStatusNotAssigned,
        id: 0,
        need_more_help: false,
        read_by_gm: false,
        text: "Wrath-rs currently does not have a functional GM ticket system. Contribute on github!".into(),
    };
    */

    let ticket_status = wow_world_messages::wrath::SMSG_GMTICKET_GETTICKET_GmTicketStatus::Default;
    let msg = SMSG_GMTICKET_GETTICKET { status: ticket_status };
    let event = ServerEvent::GMTicketGetTicket(msg);
    client.connection_sender.send_async(event).await?;
    Ok(())
}

pub async fn handle_cmsg_gmticket_create(client_manager: &ClientManager, client_id: SocketAddr, _packet: &CMSG_GMTICKET_CREATE) -> Result<()> {
    let _client = client_manager.get_authenticated_client(client_id)?;

    //Creating GM tickets is unhandled, there is no system in place. This function exists to
    //prevent warning spam until a GM ticketing system is made
    Ok(())
}

pub async fn handle_cmsg_gmticket_system_status(client_manager: &ClientManager, client_id: SocketAddr) -> Result<()> {
    let client = client_manager.get_authenticated_client(client_id)?;

    let msg = SMSG_GMTICKET_SYSTEMSTATUS {
        will_accept_tickets: wow_world_messages::wrath::GmTicketQueueStatus::Disabled,
    };
    let event = ServerEvent::GMTicketSystemStatus(msg);
    client.connection_sender.send_async(event).await?;
    Ok(())
}

pub async fn handle_speed_command(
    client_manager: &ClientManager,
    character_manager: &CharacterManager,
    client_id: SocketAddr,
    speed: f32,
) -> Result<()> {
    let client = client_manager.get_authenticated_client(client_id)?;
    let guid = client.get_active_character();
    let character = character_manager.get_character(guid)?;

    let clamped_speed = speed.clamp(0.1, 50.0);

    let msg = SMSG_FORCE_RUN_SPEED_CHANGE {
        guid: character.get_guid(),
        move_event: 0,
        speed: clamped_speed,
        unknown: 0,
    };
    let event = ServerEvent::ForceRunSpeedChange(msg);
    client.connection_sender.send_async(event).await?;

    // Set run back speed to half of run speed
    let back_msg = SMSG_FORCE_RUN_BACK_SPEED_CHANGE {
        guid: character.get_guid(),
        move_event: 0,
        speed: clamped_speed * 0.5,
    };
    let back_event = ServerEvent::ForceRunBackSpeedChange(back_msg);
    client.connection_sender.send_async(back_event).await?;

    send_system_message(client_manager, character_manager, client_id, &format!("Speed set to {}", clamped_speed)).await?;
    Ok(())
}

pub async fn handle_additem_command(
    client_manager: &ClientManager,
    character_manager: &mut CharacterManager,
    game_db: Arc<wrath_game_db::GameDatabase>,
    realm_db: Arc<wrath_realm_db::RealmDatabase>,
    client_id: SocketAddr,
    item_id: u32,
) -> Result<()> {
    if game_db.get_item_template(item_id).await.is_err() {
        return Ok(());
    }

    let client = client_manager.get_authenticated_client(client_id)?;
    let guid = client.get_active_character();
    let character = character_manager.get_character_mut(guid)?;
    let character_id = guid.guid() as u32;

    let Some(slot_id) = character.try_add_item_to_backpack(item_id, character_id, &client.connection_sender).await else {
        return Ok(());
    };

    let _ = realm_db.insert_character_item(character_id, slot_id, item_id).await;

    send_system_message(client_manager, character_manager, client_id, &format!("Added item {}", item_id)).await?;
    Ok(())
}
