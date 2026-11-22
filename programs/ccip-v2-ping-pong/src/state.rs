use anchor_lang::prelude::*;
use ccip_common::messages::FinalityConfig;

use crate::error::PingPongError;

pub const CONFIG_SEED: &[u8] = b"config";
pub const STATE_SEED: &[u8] = b"ping_pong_state";
/// Per-chain peer: the only sender accepted from that chain and the only destination sent to.
pub const PEER_SEED: &[u8] = b"peer";
/// System-owned PDA that signs the router CPI and pays fees (native SOL) and pending-reply rent.
pub const CCIP_SIGNER_SEED: &[u8] = b"ccip_signer";
pub const EXTERNAL_EXECUTION_CONFIG_SEED: &[u8] = b"external_execution_config";
pub const ALLOWED_OFFRAMP_SEED: &[u8] = b"allowed_offramp";
pub const CCV_CONFIG_SEED: &[u8] = ccip_common::ccvs::RECEIVER_CCV_CONFIG_SEED;

/// Peer address bytes: 32 for a Solana program, and for EVM the address left-padded to 32 bytes
/// (the same bytes the OffRamp delivers as the sender and the router expects as the receiver).
pub const PEER_ADDRESS_LEN: usize = 32;
pub const MAX_EXTRA_ARGS_LEN: usize = 512;
pub const MAX_CCVS: usize = 8;
/// Replies a peer account can queue in `Deferred` mode.
pub const MAX_PENDING_REPLIES: usize = 16;
pub const FINALITY_FLAG_SAFE: u16 = 0x1;

/// Payload: `abi.encode(uint8 kind, uint64 sequence)`, 64 bytes, readable by Solidity as is.
pub const PAYLOAD_LEN: usize = 64;
pub const KIND_PING: u8 = 1;
pub const KIND_PONG: u8 = 2;

pub const CONFIG_V: u8 = 1;
pub const CCV_CONFIG_V: u8 = 1;

/// How a received ping is answered.
#[derive(
    AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, Default, PartialEq, Eq, InitSpace,
)]
pub enum ReplyMode {
    /// Store a `PendingReply`; anyone sends it later with `send_pending_reply`.
    #[default]
    Deferred,
    /// Send the pong from inside `ccip_receive_v2`, with the router accounts appended to the
    /// message's accounts.
    Immediate,
}

#[account]
#[derive(InitSpace, Debug)]
pub struct Config {
    pub version: u8,
    pub bump: u8,
    pub ccip_signer_bump: u8,
    pub owner: Pubkey,
    pub proposed_owner: Pubkey,
    pub router: Pubkey,
    pub reply_mode: ReplyMode,
}

#[account]
#[derive(InitSpace, Debug)]
pub struct Peer {
    pub bump: u8,
    pub chain_selector: u64,
    /// Receiver bytes for messages to the peer.
    pub address: [u8; PEER_ADDRESS_LEN],
    /// The only sender accepted from the peer's chain. Equal to `address` for an EVM contract;
    /// for a Solana program it is the PDA that signs that program's sends.
    pub accepted_sender: [u8; PEER_ADDRESS_LEN],
    /// Tag-prefixed extra args for messages to this peer (GenericExtraArgsV3).
    #[max_len(MAX_EXTRA_ARGS_LEN)]
    pub extra_args: Vec<u8>,
    /// Sequences of pings waiting for `send_pending_reply`, oldest first.
    #[max_len(MAX_PENDING_REPLIES)]
    pub pending_replies: Vec<u64>,
}

#[account]
#[derive(InitSpace, Debug, Default)]
pub struct PingPongState {
    pub bump: u8,
    pub pings_sent: u64,
    pub pongs_received: u64,
    pub pings_received: u64,
    pub pongs_sent: u64,
    pub last_message_id: [u8; 32],
    pub last_kind: u8,
    pub last_sequence: u64,
    pub last_source_chain_selector: u64,
}

/// Per-source-chain CCV and finality policy (see the receiver example for the field semantics).
#[account]
#[derive(InitSpace, Debug)]
pub struct SourceChainCcvConfig {
    pub version: u8,
    pub bump: u8,
    #[max_len(MAX_CCVS)]
    pub required_ccvs: Vec<Pubkey>,
    #[max_len(MAX_CCVS)]
    pub optional_ccvs: Vec<Pubkey>,
    pub optional_threshold: u8,
    pub allowed_finality_config: FinalityConfig,
}

impl SourceChainCcvConfig {
    pub fn try_load(account_info: &AccountInfo) -> Result<Option<Self>> {
        if *account_info.owner != crate::ID {
            return Ok(None);
        }
        let data = account_info.try_borrow_data()?;
        let config = Self::try_deserialize(&mut &data[..])?;
        require_eq!(
            config.version,
            CCV_CONFIG_V,
            PingPongError::UnsupportedConfigVersion
        );
        Ok(Some(config))
    }

    pub fn validate(&self) -> Result<()> {
        require_gte!(
            MAX_CCVS,
            self.required_ccvs.len(),
            PingPongError::InvalidCcvPolicy
        );
        require_gte!(
            MAX_CCVS,
            self.optional_ccvs.len(),
            PingPongError::InvalidCcvPolicy
        );
        let mut seen: Vec<Pubkey> = Vec::new();
        for ccv in self.required_ccvs.iter().chain(self.optional_ccvs.iter()) {
            require!(!seen.contains(ccv), PingPongError::InvalidCcvPolicy);
            seen.push(*ccv);
        }
        require!(
            !self.optional_ccvs.contains(&Pubkey::default()),
            PingPongError::InvalidCcvPolicy
        );
        if self.optional_ccvs.is_empty() {
            require_eq!(self.optional_threshold, 0, PingPongError::InvalidCcvPolicy);
        } else {
            require!(
                self.optional_threshold > 0
                    && usize::from(self.optional_threshold) < self.optional_ccvs.len(),
                PingPongError::InvalidCcvPolicy
            );
        }
        require!(
            self.allowed_finality_config.flags & !FINALITY_FLAG_SAFE == 0,
            PingPongError::InvalidCcvPolicy
        );
        Ok(())
    }
}

/// Encodes `abi.encode(uint8 kind, uint64 sequence)`.
pub fn encode_payload(kind: u8, sequence: u64) -> Vec<u8> {
    let mut out = vec![0u8; PAYLOAD_LEN];
    out[31] = kind;
    out[56..64].copy_from_slice(&sequence.to_be_bytes());
    out
}

/// Decodes `abi.encode(uint8 kind, uint64 sequence)`, rejecting anything else.
pub fn decode_payload(data: &[u8]) -> Result<(u8, u64)> {
    require_eq!(data.len(), PAYLOAD_LEN, PingPongError::InvalidPayload);
    require!(
        data[..31].iter().all(|b| *b == 0) && data[32..56].iter().all(|b| *b == 0),
        PingPongError::InvalidPayload
    );
    let kind = data[31];
    require!(
        kind == KIND_PING || kind == KIND_PONG,
        PingPongError::InvalidPayload
    );
    let mut seq = [0u8; 8];
    seq.copy_from_slice(&data[56..64]);
    Ok((kind, u64::from_be_bytes(seq)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_round_trip() {
        let p = encode_payload(KIND_PING, 0x0102_0304_0506_0708);
        assert_eq!(p.len(), 64);
        assert_eq!(
            decode_payload(&p).unwrap(),
            (KIND_PING, 0x0102_0304_0506_0708)
        );
    }

    #[test]
    fn payload_rejects_garbage() {
        assert!(decode_payload(&[0u8; 63]).is_err());
        let mut p = encode_payload(KIND_PONG, 1);
        p[0] = 1;
        assert!(decode_payload(&p).is_err());
        assert!(decode_payload(&encode_payload(3, 1)).is_err());
    }

    #[test]
    fn ccv_policy_validation() {
        let mut c = SourceChainCcvConfig {
            version: CCV_CONFIG_V,
            bump: 0,
            required_ccvs: vec![],
            optional_ccvs: vec![],
            optional_threshold: 0,
            allowed_finality_config: FinalityConfig::default(),
        };
        c.validate().unwrap();
        c.optional_ccvs = vec![Pubkey::new_unique(), Pubkey::new_unique()];
        c.optional_threshold = 2;
        assert!(c.validate().is_err());
        c.optional_threshold = 1;
        c.validate().unwrap();
        c.allowed_finality_config.flags = 2;
        assert!(c.validate().is_err());
    }
}
