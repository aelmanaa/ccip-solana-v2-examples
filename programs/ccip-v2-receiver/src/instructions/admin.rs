use anchor_lang::prelude::*;
use anchor_lang::system_program;
use anchor_spl::token_interface::{self, Mint, TokenAccount, TokenInterface, TransferChecked};

use crate::error::ReceiverError;
use crate::state::{
    ApprovedSender, Config, APPROVED_SENDER_SEED, BPS_DENOMINATOR, CONFIG_SEED, MAX_SENDER_LEN,
    RENT_PAYER_SEED, TOKEN_ADMIN_SEED,
};
use crate::ANCHOR_DISCRIMINATOR;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct SenderParams {
    pub chain_selector: u64,
    /// The sender exactly as the OffRamp delivers it: 32 bytes for Solana, and for EVM the
    /// 20-byte address left-padded with zeros to 32 bytes.
    pub remote_sender: Vec<u8>,
}

pub fn update_router(ctx: Context<UpdateConfig>, new_router: Pubkey) -> Result<()> {
    ctx.accounts
        .config
        .update_router(ctx.accounts.authority.key(), new_router)
}

pub fn transfer_ownership(ctx: Context<UpdateConfig>, proposed_owner: Pubkey) -> Result<()> {
    ctx.accounts
        .config
        .transfer_ownership(ctx.accounts.authority.key(), proposed_owner)
}

pub fn accept_ownership(ctx: Context<AcceptOwnership>) -> Result<()> {
    ctx.accounts
        .config
        .accept_ownership(ctx.accounts.authority.key())
}

pub fn set_forward_bps(ctx: Context<UpdateConfig>, forward_bps: u16) -> Result<()> {
    require_gte!(BPS_DENOMINATOR, forward_bps, ReceiverError::InvalidBps);
    ctx.accounts.config.forward_bps = forward_bps;
    Ok(())
}

/// The `approved_sender` marker is created by the `init` constraint; its existence is the approval.
pub fn approve_sender(_ctx: Context<ApproveSender>, _params: SenderParams) -> Result<()> {
    Ok(())
}

/// The `approved_sender` marker is closed by its `close` constraint, which revokes the approval.
pub fn unapprove_sender(_ctx: Context<UnapproveSender>, _params: SenderParams) -> Result<()> {
    Ok(())
}

/// Moves `amount` of any token held by the `receiver_token_admin` PDA to any token account of the
/// same mint. Works for SPL Token and Token-2022 mints alike.
pub fn withdraw_tokens(ctx: Context<WithdrawTokens>, amount: u64) -> Result<()> {
    let seeds: &[&[u8]] = &[TOKEN_ADMIN_SEED, &[ctx.bumps.token_admin]];
    token_interface::transfer_checked(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.key(),
            TransferChecked {
                from: ctx.accounts.program_token_account.to_account_info(),
                mint: ctx.accounts.mint.to_account_info(),
                to: ctx.accounts.to_token_account.to_account_info(),
                authority: ctx.accounts.token_admin.to_account_info(),
            },
            &[seeds],
        ),
        amount,
        ctx.accounts.mint.decimals,
    )
}

/// Moves lamports out of the `rent_payer` PDA, which funds beneficiary ATAs in forward mode.
pub fn withdraw_rent_payer(ctx: Context<WithdrawRentPayer>, lamports: u64) -> Result<()> {
    let seeds: &[&[u8]] = &[RENT_PAYER_SEED, &[ctx.bumps.rent_payer]];
    system_program::transfer(
        CpiContext::new_with_signer(
            ctx.accounts.system_program.key(),
            system_program::Transfer {
                from: ctx.accounts.rent_payer.to_account_info(),
                to: ctx.accounts.to.to_account_info(),
            },
            &[seeds],
        ),
        lamports,
    )
}

#[derive(Accounts)]
pub struct UpdateConfig<'info> {
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(address = config.owner @ ReceiverError::OnlyOwner)]
    pub authority: Signer<'info>,
}

#[derive(Accounts)]
pub struct AcceptOwnership<'info> {
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(address = config.proposed_owner @ ReceiverError::OnlyProposedOwner)]
    pub authority: Signer<'info>,
}

#[derive(Accounts)]
#[instruction(params: SenderParams)]
pub struct ApproveSender<'info> {
    // The length check sits on the first account so it runs before the seed derivation below,
    // which would otherwise abort with an opaque MaxSeedLengthExceeded.
    #[account(
        seeds = [CONFIG_SEED],
        bump = config.bump,
        constraint = params.remote_sender.len() <= MAX_SENDER_LEN @ ReceiverError::SenderAddressTooLong,
    )]
    pub config: Account<'info, Config>,
    #[account(
        init,
        seeds = [
            APPROVED_SENDER_SEED,
            params.chain_selector.to_le_bytes().as_ref(),
            &params.remote_sender,
        ],
        bump,
        payer = authority,
        space = ANCHOR_DISCRIMINATOR + ApprovedSender::INIT_SPACE,
    )]
    pub approved_sender: Account<'info, ApprovedSender>,
    #[account(mut, address = config.owner @ ReceiverError::OnlyOwner)]
    pub authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(params: SenderParams)]
pub struct UnapproveSender<'info> {
    #[account(
        seeds = [CONFIG_SEED],
        bump = config.bump,
        constraint = params.remote_sender.len() <= MAX_SENDER_LEN @ ReceiverError::SenderAddressTooLong,
    )]
    pub config: Account<'info, Config>,
    #[account(
        mut,
        seeds = [
            APPROVED_SENDER_SEED,
            params.chain_selector.to_le_bytes().as_ref(),
            &params.remote_sender,
        ],
        bump,
        close = authority,
    )]
    pub approved_sender: Account<'info, ApprovedSender>,
    #[account(mut, address = config.owner @ ReceiverError::OnlyOwner)]
    pub authority: Signer<'info>,
}

#[derive(Accounts)]
pub struct WithdrawTokens<'info> {
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(
        mut,
        token::mint = mint,
        token::authority = token_admin,
        token::token_program = token_program,
    )]
    pub program_token_account: InterfaceAccount<'info, TokenAccount>,
    #[account(mut, token::mint = mint, token::token_program = token_program)]
    pub to_token_account: InterfaceAccount<'info, TokenAccount>,
    #[account(mint::token_program = token_program)]
    pub mint: InterfaceAccount<'info, Mint>,
    pub token_program: Interface<'info, TokenInterface>,
    /// CHECK: signer-only PDA that owns this program's token accounts.
    #[account(seeds = [TOKEN_ADMIN_SEED], bump)]
    pub token_admin: UncheckedAccount<'info>,
    #[account(address = config.owner @ ReceiverError::OnlyOwner)]
    pub authority: Signer<'info>,
}

#[derive(Accounts)]
pub struct WithdrawRentPayer<'info> {
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    /// CHECK: system-owned PDA holding lamports for ATA rent.
    #[account(mut, seeds = [RENT_PAYER_SEED], bump)]
    pub rent_payer: UncheckedAccount<'info>,
    /// CHECK: any account may receive lamports.
    #[account(mut)]
    pub to: UncheckedAccount<'info>,
    #[account(address = config.owner @ ReceiverError::OnlyOwner)]
    pub authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}
