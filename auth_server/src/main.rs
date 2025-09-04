//! Auth server entry point and I/O orchestration.
//!
//! Architecture overview:
//! - Each accepted TCP connection is handled by a lightweight task that only deals with the
//!   `TcpStream` (reading client packets and writing server responses).
//! - A single `ClientManager` task owns and mutates all connection/authentication state.
//! - Communication between per-connection tasks and the manager happens via Flume channels
//!   (`flume::Sender/Receiver`). This message-passing model:
//!   - avoids shared mutable state and explicit locking;
//!   - lets socket tasks focus on networking only;
//!   - keeps all state transitions serialized and exclusive within the manager task.

use anyhow::Result;
use flume::Sender;
use macro_rules_attribute::apply;
use smol::net::{TcpListener, TcpStream};
use smol_macros::main;
use std::time::Duration;
use time::macros::format_description;
use tracing::{error, info};
use tracing_subscriber::{fmt::time::UtcTime, EnvFilter};
use wow_login_messages::ServerMessage;

use wow_login_messages::version_8::opcodes::ClientOpcodeMessage;
use wrath_auth_db::AuthDatabase;

//mod auth;
mod client_manager;
mod console_input;
mod constants;
mod realms;
mod state;

use crate::client_manager::{ClientEvent, ClientManager, ServerEvent};

#[apply(main!)]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let timer = UtcTime::new(format_description!("[day]-[month]-[year] [hour]:[minute]:[second]"));
    tracing_subscriber::FmtSubscriber::builder()
        .with_env_filter(EnvFilter::new("wrath=debug,sqlx=warn"))
        .with_env_filter(EnvFilter::from_default_env())
        .with_timer(timer)
        .init();

    info!("Auth server starting");
    info!("Connecting to auth database");
    let db_connect_timeout = Duration::from_secs(std::env::var("DB_CONNECT_TIMEOUT_SECONDS")?.parse()?);
    let connect_string = std::env::var("AUTH_DATABASE_URL")?;
    let auth_db = std::sync::Arc::new(AuthDatabase::new(&connect_string, db_connect_timeout).await?);

    let (client_manager_sender, client_manager_receiver) = flume::unbounded();
    let client_manager = ClientManager::new(auth_db.clone());
    // The client manager runs on its own task and exclusively owns mutable authentication
    // state. Per-connection tasks exchange messages with the manager over Flume channels,
    // keeping I/O isolated from state updates and avoiding locks.
    smol::spawn(client_manager.run(client_manager_receiver)).detach();

    smol::spawn(realms::receive_realm_pings(auth_db.clone())).detach();
    smol::spawn(console_input::process_console_commands(auth_db.clone())).detach();

    let tcp_listener = TcpListener::bind("127.0.0.1:3724").await?;
    loop {
        let (stream, _) = tcp_listener.accept().await?;
        smol::spawn(handle_incoming_connection(stream, client_manager_sender.clone())).detach();
    }
}

/// Per-connection task: handles the socket and bridges messages to/from the client manager.
///
/// Design:
/// - Sends a `ClientEvent::Connection` to register with the manager and obtain a
///   `ServerEvent` receiver for replies.
/// - Races between reading a packet from the client and receiving a server message from the
///   manager, forwarding events to the other side.
///
/// Rationale for Flume:
/// - Channels decouple I/O from state handling and avoid mutex contention.
/// - The client manager serializes all state transitions on a single task.
async fn handle_incoming_connection(mut stream: TcpStream, client_manager_sender: Sender<ClientEvent>) -> Result<()> {
    let addr = stream.local_addr()?;
    let (client_sender, client_receiver) = flume::unbounded();
    let connection_event = ClientEvent::Connection { addr, client_sender };
    client_manager_sender.send_async(connection_event).await?;

    let mut buf = [0u8; 1024];
    loop {
        let event = smol::future::race(receive_from_client(&mut stream, &mut buf), receive_from_manager(&client_receiver)).await;

        if let Err(e) = event {
            error!("{e}");
            info!("disconnect!");
            stream.shutdown(smol::net::Shutdown::Both)?;
            break;
        };

        match event.unwrap() {
            ConnectionEvent::Client(packet) => {
                info!("Handling auth packet {} for client {}", packet, addr);
                let client_message = ClientEvent::Message {
                    addr,
                    packet: packet.clone(),
                };
                client_manager_sender.send_async(client_message).await?;
            }
            ConnectionEvent::Server(server_event) => match server_event {
                ServerEvent::AuthLogonProof(proof) => {
                    proof.astd_write(&mut stream).await?;
                }
                ServerEvent::AuthLogonChallenge(challenge) => {
                    challenge.astd_write(&mut stream).await?;
                }
                ServerEvent::AuthReconnectChallenge(reconnect_challenge) => {
                    reconnect_challenge.astd_write(&mut stream).await?;
                }
                ServerEvent::AuthReconnectProof(reconnect_proof) => {
                    reconnect_proof.astd_write(&mut stream).await?;
                }
                ServerEvent::RealmList(realm_list) => {
                    realm_list.astd_write(&mut stream).await?;
                }
                ServerEvent::Disconnect => {
                    info!("Disconnecting client {}", addr);
                    stream.shutdown(smol::net::Shutdown::Both)?;
                    break;
                }
            },
        }
    }
    Ok(())
}

/// Event multiplexing between client (socket) and server (manager) sides.
enum ConnectionEvent {
    Client(ClientOpcodeMessage),
    Server(ServerEvent),
}

/// Read the next client message from the socket. Returns when a full `ClientOpcodeMessage`
/// is available. Uses `peek` to avoid busy-waiting and then performs a framed read.
async fn receive_from_client(stream: &mut TcpStream, buf: &mut [u8; 1024]) -> Result<ConnectionEvent> {
    loop {
        let read_len = stream.peek(buf).await?;
        if read_len > 0 {
            let packet = ClientOpcodeMessage::astd_read(stream).await?;
            return Ok(ConnectionEvent::Client(packet));
        }
    }
}

/// Await the next `ServerEvent` from the client manager for this connection.
async fn receive_from_manager(receiver: &flume::Receiver<ServerEvent>) -> Result<ConnectionEvent> {
    Ok(ConnectionEvent::Server(receiver.recv_async().await?))
}
