//! Client connection and authentication manager for the auth server.
//!
//! This module coordinates the login workflow using SRP6:
//! - Tracks connected clients and their per-connection state.
//! - Performs the SRP6 logon handshake (challenge -> proof) and persists the session key.
//! - Supports fast reconnect via the SRP6 reconnect challenge/proof flow.
//! - Serves the realm list after successful authentication.
//!
//! High-level flow:
//! 1. Client sends `CMD_AUTH_LOGON_CHALLENGE` -> the server loads the account (v, s),
//!    builds an `SrpVerifier` and sends back server public key, salt, generator and prime.
//! 2. Client sends `CMD_AUTH_LOGON_PROOF` -> the server verifies the SRP proof, stores the
//!    session key, marks the client authenticated and allows realm list requests.
//! 3. Optional: client reconnects using `CMD_AUTH_RECONNECT_CHALLENGE/PROOF`, which is
//!    validated against the stored `SrpServer` state.
//! 4. Authenticated client requests `CMD_REALM_LIST` and receives the available realms.
//!
//! Cleanup: connections and authenticated addresses are pruned periodically based on the
//! `AUTH_RECONNECT_LIFETIME` environment variable (seconds).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use std::{collections::HashMap, env, time::Duration};

use anyhow::{anyhow, Result};
use flume::Receiver;
use smol::future;
use smol::Timer;
use tracing::error;
use tracing::info;
use wow_login_messages::all::*;
use wow_login_messages::version_8::opcodes::ClientOpcodeMessage;
use wow_login_messages::version_8::*;
use wow_srp::normalized_string::NormalizedString;
use wow_srp::server::{SrpServer, SrpVerifier};
use wow_srp::{PublicKey, GENERATOR, LARGE_SAFE_PRIME_LITTLE_ENDIAN, PASSWORD_VERIFIER_LENGTH, SALT_LENGTH};
use wrath_auth_db::AuthDatabase;

use crate::realms::get_realm_list;
use crate::state::ClientState;

/// Internal events consumed by the `ClientManager` loop.
#[allow(clippy::large_enum_variant)]
enum ClientManagerEvent {
    Msg(ClientEvent),
    Tick,
    Closed,
}

/// Events produced by the network/IO layer and consumed by the client manager.
pub enum ClientEvent {
    Connection {
        addr: SocketAddr,
        client_sender: flume::Sender<ServerEvent>,
    },
    Message {
        addr: SocketAddr,
        packet: ClientOpcodeMessage,
    },
}

/// Events sent by the client manager back to the connection writer for delivery to the client.
pub enum ServerEvent {
    AuthLogonProof(CMD_AUTH_LOGON_PROOF_Server),
    AuthLogonChallenge(CMD_AUTH_LOGON_CHALLENGE_Server),
    AuthReconnectChallenge(CMD_AUTH_RECONNECT_CHALLENGE_Server),
    AuthReconnectProof(CMD_AUTH_RECONNECT_PROOF_Server),
    RealmList(CMD_REALM_LIST_Server),
    Disconnect,
}

/// Represents a single network connection and its outbound channel.
pub struct Connection {
    sender: flume::Sender<ServerEvent>,
    created_at: Instant,
}

impl Connection {
    /// Create a new connection wrapper around the `ServerEvent` sender.
    pub fn new(sender: flume::Sender<ServerEvent>) -> Self {
        Self {
            sender,
            created_at: Instant::now(),
        }
    }
}

/// Per-client state kept while the connection is alive.
pub struct Client {
    connection: Connection,

    /// Reflects the progress in the login/reconnect flows.
    state: Option<ClientState>,

    /// Populated once SRP6 verification succeeds and is used for reconnects.
    authentication: Option<Authentication>,
}

impl Client {
    /// Initialize a newly connected (unauthenticated) client.
    pub fn new(connection: Connection) -> Self {
        Self {
            connection,
            state: Some(ClientState::Connected),
            authentication: None,
        }
    }

    /// Mark the client as authenticated and store SRP context for future reconnects.
    pub fn authenticate(&mut self, srp_server: SrpServer, username: String) {
        self.authentication = Some(Authentication { srp_server, username });
        self.state = Some(ClientState::LogOnProof);
    }
}

/// Holds SRP6 context and associated username for an authenticated session.
pub struct Authentication {
    srp_server: SrpServer,
    username: String,
}

/// Manages connected clients and drives authentication and realm list flows.
pub struct ClientManager {
    /// Clients in the process of connecting
    connected_clients: HashMap<SocketAddr, Client>,

    /// Addresses that are fully connected and authenticated
    authenticated_addresses: HashMap<String, SocketAddr>,

    auth_reconnect_lifetime: Duration,
    auth_database: Arc<AuthDatabase>,
}

impl ClientManager {
    /// Create a new client manager with the provided auth database.
    pub fn new(auth_database: Arc<AuthDatabase>) -> Self {
        let auth_reconnect_lifetime = get_auth_reconnect_lifetime();
        Self {
            connected_clients: HashMap::new(),
            authenticated_addresses: HashMap::new(),
            auth_reconnect_lifetime,
            auth_database,
        }
    }

    /// Main event loop: consumes incoming `ClientEvent`s and performs periodic cleanup ticks.
    pub async fn run(mut self, receiver: Receiver<ClientEvent>) {
        loop {
            match future::race(receive_messages(&receiver), message_timeout()).await {
                ClientManagerEvent::Msg(event) => match event {
                    ClientEvent::Connection { addr, client_sender } => {
                        info!("Client connected from: {addr}");
                        self.connected_clients.insert(addr, Client::new(Connection::new(client_sender)));
                    }
                    ClientEvent::Message { addr, packet } => {
                        if let Err(e) = self.handle_message(&addr, packet).await {
                            error!("Error handling message from {addr}: {e}");
                            let server_event = ServerEvent::Disconnect;
                            let client = self.connected_clients.remove(&addr).unwrap();
                            client.connection.sender.send_async(server_event).await.unwrap();
                        }
                    }
                },
                ClientManagerEvent::Tick => {
                    self.reconnect_clients_cleaner().await;
                }
                ClientManagerEvent::Closed => {
                    break;
                }
            }
        }
    }

    /// Prune stale connections and authenticated addresses beyond the reconnect lifetime.
    async fn reconnect_clients_cleaner(&mut self) {
        self.authenticated_addresses.retain(|_, addr| {
            if let Some(client) = self.connected_clients.get(addr) {
                client.connection.created_at.elapsed() < self.auth_reconnect_lifetime
            } else {
                // No connected client associated to this authentication, just remove it
                false
            }
        });
        self.connected_clients
            .retain(|_, client| client.connection.created_at.elapsed() < self.auth_reconnect_lifetime);
    }

    /// Dispatch a client opcode to the appropriate handler based on the login protocol.
    async fn handle_message(&mut self, addr: &SocketAddr, packet: ClientOpcodeMessage) -> Result<()> {
        match packet {
            ClientOpcodeMessage::CMD_AUTH_LOGON_CHALLENGE(challenge) => {
                self.handle_auth_logon_challenge(addr, challenge).await?;
            }
            ClientOpcodeMessage::CMD_AUTH_LOGON_PROOF(logon_proof) => {
                self.handle_auth_logon_proof(addr, logon_proof).await?;
            }
            ClientOpcodeMessage::CMD_AUTH_RECONNECT_CHALLENGE(reconnect_challenge) => {
                self.handle_reconnect_challenge(addr, reconnect_challenge).await?;
            }
            ClientOpcodeMessage::CMD_AUTH_RECONNECT_PROOF(reconnect_proof) => {
                self.handle_reconnect_proof(addr, reconnect_proof).await?;
            }
            ClientOpcodeMessage::CMD_REALM_LIST(_realm_list) => {
                self.handle_realm_list(addr).await?;
            }
        }
        Ok(())
    }

    /// Handle `CMD_AUTH_LOGON_CHALLENGE`:
    /// - Validates the account exists and is not banned.
    /// - Loads SRP verifier values (v, s) and constructs the server proof response.
    /// - Sends `CMD_AUTH_LOGON_CHALLENGE_Server` and transitions to `ChallengeProof` state.
    async fn handle_auth_logon_challenge(&mut self, addr: &SocketAddr, challenge: CMD_AUTH_LOGON_CHALLENGE_Client) -> Result<()> {
        let client = self.connected_clients.get_mut(addr).unwrap();

        let account = match self.auth_database.get_account_by_username(&challenge.account_name).await? {
            Some(acc) if acc.banned != 0 => {
                client.state.replace(ClientState::Connected);
                self.reject_logon_challenge(addr, CMD_AUTH_LOGON_CHALLENGE_Server_LoginResult::FailBanned)
                    .await?;
                return Ok(());
            }
            Some(acc) if acc.v.is_empty() || acc.s.is_empty() => {
                client.state.replace(ClientState::Connected);
                self.reject_logon_challenge(addr, CMD_AUTH_LOGON_CHALLENGE_Server_LoginResult::FailUnknownAccount)
                    .await?;
                return Ok(());
            }
            Some(acc) => acc,
            None => {
                client.state.replace(ClientState::Connected);
                self.reject_logon_challenge(addr, CMD_AUTH_LOGON_CHALLENGE_Server_LoginResult::FailUnknownAccount)
                    .await?;
                return Ok(());
            }
        };

        let username = NormalizedString::from(&account.username)?;
        let mut password_verifier: [u8; PASSWORD_VERIFIER_LENGTH as usize] = Default::default();
        let mut salt: [u8; SALT_LENGTH as usize] = Default::default();

        hex::decode_to_slice(account.v.as_bytes(), &mut password_verifier)?;
        hex::decode_to_slice(account.s.as_bytes(), &mut salt)?;

        let srp_verifier = SrpVerifier::from_database_values(username, password_verifier, salt);
        let srp_proof = srp_verifier.into_proof();

        let auth_logon_challenge = CMD_AUTH_LOGON_CHALLENGE_Server {
            result: CMD_AUTH_LOGON_CHALLENGE_Server_LoginResult::Success {
                crc_salt: [
                    0xBA, 0xA3, 0x1E, 0x99, 0xA0, 0x0B, 0x21, 0x57, 0xFC, 0x37, 0x3F, 0xB3, 0x69, 0xCD, 0xD2, 0xF1,
                ],
                generator: vec![GENERATOR],
                large_safe_prime: Vec::from(LARGE_SAFE_PRIME_LITTLE_ENDIAN),
                salt: *srp_proof.salt(),
                // https://github.com/TrinityCore/TrinityCore/blob/3.3.5/src/server/authserver/Server/AuthSession.cpp:117
                security_flag: CMD_AUTH_LOGON_CHALLENGE_Server_SecurityFlag::empty(),
                server_public_key: *srp_proof.server_public_key(),
            },
        };

        client
            .connection
            .sender
            .send_async(ServerEvent::AuthLogonChallenge(auth_logon_challenge))
            .await?;
        client.state.replace(ClientState::ChallengeProof {
            srp_proof,
            username: account.username,
        });

        Ok(())
    }

    /// Handle `CMD_AUTH_LOGON_PROOF`:
    /// - Validates state and parses client public key.
    /// - Verifies the SRP proof; on success, stores session key in the auth DB.
    /// - Marks the client authenticated and allows subsequent realm list requests.
    async fn handle_auth_logon_proof(&mut self, addr: &SocketAddr, logon_proof: CMD_AUTH_LOGON_PROOF_Client) -> Result<()> {
        let client_public_key = match PublicKey::from_le_bytes(logon_proof.client_public_key) {
            Ok(key) => key,
            Err(_) => {
                self.reject_logon_proof(addr, CMD_AUTH_LOGON_PROOF_Server_LoginResult::FailIncorrectPassword)
                    .await?;
                return Err(anyhow!("Invalid client public key. This is likely a result of malformed packets."));
            }
        };

        let client = self.connected_clients.get_mut(addr).unwrap();

        // Verify its state
        let Some(ClientState::ChallengeProof { srp_proof, username }) = client.state.take() else {
            self.reject_logon_proof(addr, CMD_AUTH_LOGON_PROOF_Server_LoginResult::FailUnknownAccount)
                .await?;
            return Err(anyhow!("Client is not in ChallengeProof state."));
        };

        let (srp_server, server_proof) = match srp_proof.into_server(client_public_key, logon_proof.client_proof) {
            Ok(s) => s,
            Err(e) => {
                self.reject_logon_proof(addr, CMD_AUTH_LOGON_PROOF_Server_LoginResult::FailIncorrectPassword)
                    .await?;
                return Err(anyhow!(e));
            }
        };

        self.auth_database
            .set_account_sessionkey(&username, &hex::encode(srp_server.session_key()))
            .await?;

        let auth_logon_proof = CMD_AUTH_LOGON_PROOF_Server {
            result: CMD_AUTH_LOGON_PROOF_Server_LoginResult::Success {
                account_flag: AccountFlag::empty(),
                hardware_survey_id: 0,
                server_proof,
                unknown_flags: 0,
            },
        };

        client.connection.sender.send_async(ServerEvent::AuthLogonProof(auth_logon_proof)).await?;
        client.authenticate(srp_server, username.clone());

        if let Some(other_address) = self.authenticated_addresses.insert(username, *addr) {
            // Disconnect the other client that was connected with this account
            if let Some(other_client) = self.connected_clients.remove(&other_address) {
                other_client.connection.sender.send_async(ServerEvent::Disconnect).await?;
            }
        }

        Ok(())
    }

    /// Send a failed logon proof result to the client.
    async fn reject_logon_proof(&mut self, addr: &SocketAddr, result: CMD_AUTH_LOGON_PROOF_Server_LoginResult) -> Result<()> {
        let client = self.connected_clients.get(addr).unwrap();
        let auth_logon_proof = CMD_AUTH_LOGON_PROOF_Server { result };
        client.connection.sender.send_async(ServerEvent::AuthLogonProof(auth_logon_proof)).await?;
        Ok(())
    }

    /// Send a failed logon challenge result to the client.
    async fn reject_logon_challenge(&mut self, addr: &SocketAddr, result: CMD_AUTH_LOGON_CHALLENGE_Server_LoginResult) -> Result<()> {
        let client = self.connected_clients.get(addr).unwrap();
        let auth_logon_challenge = CMD_AUTH_LOGON_CHALLENGE_Server { result };
        client
            .connection
            .sender
            .send_async(ServerEvent::AuthLogonChallenge(auth_logon_challenge))
            .await?;
        Ok(())
    }

    /// Handle `CMD_AUTH_RECONNECT_CHALLENGE`:
    /// - Looks up an existing authenticated session for the username.
    /// - If found, sends to the client the reconnect challenge data bound to the stored `SrpServer`.
    async fn handle_reconnect_challenge(&mut self, addr: &SocketAddr, challenge: CMD_AUTH_RECONNECT_CHALLENGE_Client) -> Result<()> {
        // When this command is received, there should be a corresponding client that is authenticated.
        let authenticated_address = self.authenticated_addresses.get(&challenge.account_name);

        if authenticated_address.is_none() {
            error!("Failed to find authenticated address for account {}", challenge.account_name);

            let auth_reconnect_challenge = CMD_AUTH_RECONNECT_CHALLENGE_Server {
                result: CMD_AUTH_RECONNECT_CHALLENGE_Server_LoginResult::FailUnknown0,
            };

            let reconnecting_client = self.connected_clients.get(addr).unwrap();
            reconnecting_client
                .connection
                .sender
                .send_async(ServerEvent::AuthReconnectChallenge(auth_reconnect_challenge))
                .await?;
        }

        let authenticated_address = authenticated_address.unwrap();
        let authenticated_client = self.connected_clients.get(authenticated_address).unwrap();
        let challenge_data = *authenticated_client
            .authentication
            .as_ref()
            .unwrap()
            .srp_server
            .reconnect_challenge_data();

        let auth_reconnect_challenge = CMD_AUTH_RECONNECT_CHALLENGE_Server {
            result: CMD_AUTH_RECONNECT_CHALLENGE_Server_LoginResult::Success {
                challenge_data,
                checksum_salt: [
                    0xBA, 0xA3, 0x1E, 0x99, 0xA0, 0x0B, 0x21, 0x57, 0xFC, 0x37, 0x3F, 0xB3, 0x69, 0xCD, 0xD2, 0xF1,
                ],
            },
        };
        let server_event = ServerEvent::AuthReconnectChallenge(auth_reconnect_challenge);

        // Response should go to the reconnecting client, not to the authenticated one which might be stale
        let reconnecting_client = self.connected_clients.get_mut(addr).unwrap();
        reconnecting_client.connection.sender.send_async(server_event).await?;
        reconnecting_client.state.replace(ClientState::ReconnectProof {
            username: challenge.account_name.to_string(),
        });
        Ok(())
    }

    /// Handle `CMD_AUTH_RECONNECT_PROOF`:
    /// - Verifies the reconnect proof against the stored `SrpServer` of the authenticated client.
    /// - On success, transfers authentication to the reconnecting connection and refreshes state.
    pub async fn handle_reconnect_proof(&mut self, addr: &SocketAddr, reconnect_proof: CMD_AUTH_RECONNECT_PROOF_Client) -> Result<()> {
        // Verify state
        let username = {
            let reconnecting_client = self.connected_clients.get(addr).unwrap();
            let Some(ClientState::ReconnectProof { username }) = &reconnecting_client.state else {
                self.reject_logon_proof(addr, CMD_AUTH_LOGON_PROOF_Server_LoginResult::FailUnknownAccount)
                    .await?;
                return Err(anyhow!("Client is not in ReconnectProof state."));
            };
            username.clone()
        };

        // Verify against the authenticated client
        let result = {
            let authenticated_address = self.authenticated_addresses.get_mut(&username).unwrap();
            let authenticated_client = self.connected_clients.get_mut(authenticated_address).unwrap();
            authenticated_client
                .authentication
                .as_mut()
                .unwrap()
                .srp_server
                .verify_reconnection_attempt(reconnect_proof.proof_data, reconnect_proof.client_proof)
        };

        let auth_reconnect_proof = CMD_AUTH_RECONNECT_PROOF_Server {
            result: if result {
                LoginResult::Success
            } else {
                LoginResult::FailIncorrectPassword
            },
        };

        {
            let reconnecting_client = self.connected_clients.get_mut(addr).unwrap();
            reconnecting_client
                .connection
                .sender
                .send_async(ServerEvent::AuthReconnectProof(auth_reconnect_proof))
                .await?;
        }

        if result {
            // Update reconnecting client
            let authenticated_address = self.authenticated_addresses.get_mut(&username).unwrap();
            let stale_client = self.connected_clients.remove(authenticated_address).unwrap();
            let reconnecting_client = self.connected_clients.get_mut(addr).unwrap();
            reconnecting_client.authentication = stale_client.authentication;
            reconnecting_client.state.replace(ClientState::LogOnProof);
        } else {
            let reconnecting_client = self.connected_clients.get_mut(addr).unwrap();
            reconnecting_client.state.replace(ClientState::Connected);
        }
        Ok(())
    }

    /// Handle `CMD_REALM_LIST` for an authenticated client and send the realm list.
    pub async fn handle_realm_list(&mut self, addr: &SocketAddr) -> Result<()> {
        let client = self.connected_clients.get_mut(addr).unwrap();

        // Verify state
        if !matches!(client.state, Some(ClientState::LogOnProof)) {
            client.connection.sender.send_async(ServerEvent::Disconnect).await?;
            return Err(anyhow!("Client is not in LogOnProof state."));
        }

        let username = client.authentication.as_ref().unwrap().username.clone();

        let account = match self.auth_database.get_account_by_username(&username).await? {
            Some(acc) => acc,
            None => return Err(anyhow!("Username is not in database")),
        };
        let realms = get_realm_list(&self.auth_database, account.id).await?;

        let realm_list = CMD_REALM_LIST_Server { realms };
        let server_message = ServerEvent::RealmList(realm_list);
        client.connection.sender.send_async(server_message).await?;
        client.state.replace(ClientState::LogOnProof);
        Ok(())
    }
}

/// Receive the next `ClientEvent` or signal closure.
async fn receive_messages(receiver: &Receiver<ClientEvent>) -> ClientManagerEvent {
    match receiver.recv_async().await {
        Ok(event) => ClientManagerEvent::Msg(event),
        Err(e) => {
            error!("Client manager receiver error: {}", e);
            ClientManagerEvent::Closed
        }
    }
}

/// Tick used to periodically trigger cleanup of stale connections.
async fn message_timeout() -> ClientManagerEvent {
    Timer::after(Duration::from_secs(60)).await;
    ClientManagerEvent::Tick
}

/// Read `AUTH_RECONNECT_LIFETIME` from the environment and convert to `Duration`.
/// Defaults to 500 seconds if missing or invalid.
fn get_auth_reconnect_lifetime() -> Duration {
    let secs = env::var("AUTH_RECONNECT_LIFETIME").map_or(500, |x| x.parse::<u64>().unwrap_or(500));
    Duration::from_secs(secs)
}
