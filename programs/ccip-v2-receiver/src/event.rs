use anchor_lang::prelude::*;

use crate::state::{CcvPolicy, ReceiveMode};

/// Emitted for every delivered message, with the full data.
#[event]
pub struct MessageReceived {
    pub message_id: [u8; 32],
    pub source_chain_selector: u64,
    pub sender: Vec<u8>,
    pub mode: ReceiveMode,
    pub data: Vec<u8>,
    pub token: Pubkey,
    pub amount: u64,
}

/// Emitted when a forward-mode message pays out to its beneficiary.
#[event]
pub struct TokensForwarded {
    pub message_id: [u8; 32],
    pub beneficiary: Pubkey,
    pub mint: Pubkey,
    pub amount: u64,
    pub forwarded: u64,
    pub kept: u64,
}

/// Emitted when a chain policy or a sender policy is set or updated.
#[event]
pub struct CcvPolicySet {
    pub source_chain_selector: u64,
    /// Empty for the chain policy, the sender bytes for a sender policy.
    pub sender: Vec<u8>,
    pub policy: CcvPolicy,
}

/// Emitted when a chain's whole config, or one sender policy, is removed.
#[event]
pub struct CcvPolicyRemoved {
    pub source_chain_selector: u64,
    pub sender: Vec<u8>,
}
