use wow_srp::server::SrpProof;

pub enum ClientState {
    Connected,
    ChallengeProof { srp_proof: SrpProof, username: String },
    ReconnectProof { username: String },
    LogOnProof,
}
