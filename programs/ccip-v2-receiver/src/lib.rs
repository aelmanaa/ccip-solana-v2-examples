//! Example CCIP 2.0 receiver program for Solana.
//!
//! Derived from the MIT-licensed `example-ccip-receiver-v2` in
//! smartcontractkit/chainlink-ccip-solana. See the repository README for the account layout, the
//! payload format and the CCV and finality policy.
use anchor_lang::prelude::*;
use ccip_common::ccvs::{GetReceiverCcvsIn, GetReceiverCcvsOut};

pub mod error;
pub mod event;
pub mod instructions;
pub mod message;
pub mod state;

use instructions::*;
use message::Any2SVMMessage;

declare_id!("FgFQwXGVeKDNawwer8Toa1xSKih1DgQp9JMjp3ft7SkA");

pub const ANCHOR_DISCRIMINATOR: usize = 8;

#[program]
pub mod ccip_v2_receiver {
    use super::*;

    /// Creates the config (owner, router, forward cut) and the message record. Upgrade authority
    /// only.
    pub fn initialize(ctx: Context<Initialize>, router: Pubkey, forward_bps: u16) -> Result<()> {
        instructions::initialize::initialize(ctx, router, forward_bps)
    }

    /// Registers this program with the router as a CCIP 2.0 receiver, so the OffRamp calls
    /// `ccip_receive_v2` and asks `get_ccvs_and_finality_config` for its policy. Owner only.
    pub fn register_with_router(ctx: Context<RegisterWithRouter>) -> Result<()> {
        instructions::register::register_with_router(ctx)
    }

    /// Delivers a message. Callable only by an OffRamp the router allows for the source chain.
    pub fn ccip_receive_v2<'info>(
        ctx: Context<'info, CcipReceiveV2<'info>>,
        message: Any2SVMMessage,
    ) -> Result<()> {
        instructions::ccip_receive_v2::ccip_receive_v2(ctx, message)
    }

    /// CCIP 2.0 receiver interface: the CCVs and finality this receiver requires for a message
    /// from `input.source_chain_selector` sent by `input.sender`.
    pub fn get_ccvs_and_finality_config(
        ctx: Context<GetCcvsAndFinalityConfig>,
        input: GetReceiverCcvsIn,
    ) -> Result<GetReceiverCcvsOut> {
        instructions::get_ccvs::get_ccvs_and_finality_config(ctx, input)
    }

    /// Creates or updates the chain policy, or one sender's policy, for a source chain. CCV lists
    /// and finality can be set together or one at a time. Owner only.
    pub fn set_ccv_policy(ctx: Context<SetCcvPolicy>, params: SetCcvPolicyParams) -> Result<()> {
        instructions::ccv_config::set_ccv_policy(ctx, params)
    }

    /// Removes one sender's policy; the sender falls back to the chain policy. Owner only.
    pub fn remove_sender_ccv_policy(
        ctx: Context<UpdateCcvConfig>,
        source_chain_selector: u64,
        sender: Vec<u8>,
    ) -> Result<()> {
        instructions::ccv_config::remove_sender_ccv_policy(ctx, source_chain_selector, sender)
    }

    /// Removes every policy for a source chain and closes its account. Owner only.
    pub fn remove_ccv_config(
        ctx: Context<RemoveCcvConfig>,
        source_chain_selector: u64,
    ) -> Result<()> {
        instructions::ccv_config::remove_ccv_config(ctx, source_chain_selector)
    }

    /// Approves a (source chain, sender) pair. Owner only.
    pub fn approve_sender(ctx: Context<ApproveSender>, params: SenderParams) -> Result<()> {
        instructions::admin::approve_sender(ctx, params)
    }

    /// Revokes a (source chain, sender) pair. Owner only.
    pub fn unapprove_sender(ctx: Context<UnapproveSender>, params: SenderParams) -> Result<()> {
        instructions::admin::unapprove_sender(ctx, params)
    }

    /// Sets the share of forwarded tokens this program keeps, in basis points. Owner only.
    pub fn set_forward_bps(ctx: Context<UpdateConfig>, forward_bps: u16) -> Result<()> {
        instructions::admin::set_forward_bps(ctx, forward_bps)
    }

    /// Moves tokens held by the token admin PDA to any token account. Owner only.
    pub fn withdraw_tokens(ctx: Context<WithdrawTokens>, amount: u64) -> Result<()> {
        instructions::admin::withdraw_tokens(ctx, amount)
    }

    /// Moves lamports out of the rent payer PDA. Owner only.
    pub fn withdraw_rent_payer(ctx: Context<WithdrawRentPayer>, lamports: u64) -> Result<()> {
        instructions::admin::withdraw_rent_payer(ctx, lamports)
    }

    pub fn update_router(ctx: Context<UpdateConfig>, new_router: Pubkey) -> Result<()> {
        instructions::admin::update_router(ctx, new_router)
    }

    pub fn transfer_ownership(ctx: Context<UpdateConfig>, proposed_owner: Pubkey) -> Result<()> {
        instructions::admin::transfer_ownership(ctx, proposed_owner)
    }

    pub fn accept_ownership(ctx: Context<AcceptOwnership>) -> Result<()> {
        instructions::admin::accept_ownership(ctx)
    }
}
