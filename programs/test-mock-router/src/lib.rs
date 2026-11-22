//! Test-only stand-in for the CCIP router. Never deployed. `ccip_send_v2` records the message
//! it was given, bills a flat native fee from the signer, and returns a message id. It also owns
//! the `allowed_offramp` PDAs the receivers check.
use anchor_lang::prelude::*;
use anchor_lang::solana_program::program::set_return_data;
use anchor_lang::system_program;

declare_id!("J1Jr3eXeTwskq5Tdw153Go4kGS5FV868iTh2JjH5uALz");

pub const LAST_SEND_SEED: &[u8] = b"last_send";
pub const MOCK_FEE_LAMPORTS: u64 = 10_000;

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, Default)]
pub struct TokenAmount {
    pub token: Pubkey,
    pub amount: u64,
}

/// Same Borsh layout as the router's `SVM2AnyMessage`.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, Default)]
pub struct Message {
    pub receiver: Vec<u8>,
    pub data: Vec<u8>,
    pub token_amounts: Vec<TokenAmount>,
    pub fee_token: Pubkey,
    pub extra_args: Vec<u8>,
}

/// Same Borsh layout as the router's `CcipSendV2Params`.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, Default)]
pub struct SendParams {
    pub dest_chain_selector: u64,
    pub message: Message,
    pub resolution_metadata: Vec<u8>,
}

#[account]
#[derive(Default, Debug)]
pub struct LastSend {
    pub count: u64,
    pub sender: Pubkey,
    pub params: SendParams,
}

#[program]
pub mod test_mock_router {
    use super::*;

    pub fn ccip_send_v2(ctx: Context<MockSend>, params: SendParams) -> Result<()> {
        system_program::transfer(
            CpiContext::new(
                ctx.accounts.system_program.key(),
                system_program::Transfer {
                    from: ctx.accounts.authority.to_account_info(),
                    to: ctx.accounts.fee_vault.to_account_info(),
                },
            ),
            MOCK_FEE_LAMPORTS,
        )?;
        let last = &mut ctx.accounts.last_send;
        last.count += 1;
        last.sender = ctx.accounts.authority.key();
        let mut id = [0u8; 32];
        id[..8].copy_from_slice(&last.count.to_le_bytes());
        last.params = params;
        set_return_data(&id);
        Ok(())
    }

    pub fn init_last_send(_ctx: Context<InitLastSend>) -> Result<()> {
        Ok(())
    }
}

#[derive(Accounts)]
pub struct MockSend<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,
    /// CHECK: receives the flat fee.
    #[account(mut)]
    pub fee_vault: UncheckedAccount<'info>,
    #[account(mut, seeds = [LAST_SEND_SEED], bump)]
    pub last_send: Account<'info, LastSend>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct InitLastSend<'info> {
    #[account(init, seeds = [LAST_SEND_SEED], bump, payer = payer, space = 4096)]
    pub last_send: Account<'info, LastSend>,
    #[account(mut)]
    pub payer: Signer<'info>,
    pub system_program: Program<'info, System>,
}
