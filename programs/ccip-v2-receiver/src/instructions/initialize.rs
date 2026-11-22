use anchor_lang::prelude::*;

use crate::error::ReceiverError;
use crate::program::CcipV2Receiver;
use crate::state::{
    Config, ReceiveMode, ReceiverState, BPS_DENOMINATOR, CONFIG_SEED, CONFIG_V,
    RECEIVER_STATE_SEED, RECEIVER_STATE_V,
};
use crate::ANCHOR_DISCRIMINATOR;

/// Creates the `Config` and `ReceiverState` PDAs. Only the program's upgrade authority may call
/// it, so nobody can front-run the deployer and take ownership.
///
/// The `receiver_token_admin` and `rent_payer` PDAs are not created here: the first only signs
/// (its ATAs are created by the OffRamp or by `withdraw_tokens` callers), and the second is a
/// plain system account that exists once someone sends it lamports.
pub fn initialize(ctx: Context<Initialize>, router: Pubkey, forward_bps: u16) -> Result<()> {
    require_keys_neq!(router, Pubkey::default(), ReceiverError::InvalidRouter);
    require_gte!(BPS_DENOMINATOR, forward_bps, ReceiverError::InvalidBps);
    ctx.accounts.config.set_inner(Config {
        version: CONFIG_V,
        bump: ctx.bumps.config,
        owner: ctx.accounts.authority.key(),
        proposed_owner: Pubkey::default(),
        router,
        forward_bps,
    });
    ctx.accounts.receiver_state.set_inner(ReceiverState {
        version: RECEIVER_STATE_V,
        bump: ctx.bumps.receiver_state,
        received_count: 0,
        last_mode: ReceiveMode::None,
        last_message_id: [0; 32],
        last_source_chain_selector: 0,
        last_sender: vec![],
        last_data_len: 0,
        last_data: vec![],
        last_token: Pubkey::default(),
        last_amount: 0,
        last_beneficiary: Pubkey::default(),
        last_forwarded: 0,
        last_kept: 0,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(
        init,
        seeds = [CONFIG_SEED],
        bump,
        payer = authority,
        space = ANCHOR_DISCRIMINATOR + Config::INIT_SPACE,
    )]
    pub config: Account<'info, Config>,
    #[account(
        init,
        seeds = [RECEIVER_STATE_SEED],
        bump,
        payer = authority,
        space = ANCHOR_DISCRIMINATOR + ReceiverState::INIT_SPACE,
    )]
    pub receiver_state: Account<'info, ReceiverState>,
    #[account(mut)]
    pub authority: Signer<'info>,
    pub system_program: Program<'info, System>,
    #[account(constraint = program.programdata_address()? == Some(program_data.key()))]
    pub program: Program<'info, CcipV2Receiver>,
    #[account(
        constraint = program_data.upgrade_authority_address == Some(authority.key())
            @ ReceiverError::Unauthorized
    )]
    pub program_data: Account<'info, ProgramData>,
}
