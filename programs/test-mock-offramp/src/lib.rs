//! Test-only stand-in for the CCIP OffRamp. Never deployed. `deliver` invokes a receiver with
//! arbitrary instruction data while signing as the receiver's `external_execution_config` PDA,
//! as the real OffRamp does.
use anchor_lang::prelude::*;
use anchor_lang::solana_program::{instruction::Instruction, program::invoke_signed};

declare_id!("32U3WgKXLktgW6p9MKdgizs7zgQ4wLmfAqXCkfVLUfL8");

pub const EXTERNAL_EXECUTION_CONFIG_SEED: &[u8] = b"external_execution_config";
#[program]
pub mod test_mock_offramp {
    use super::*;

    /// remaining_accounts[0] is the receiver program; the rest are its accounts after the signer.
    pub fn deliver<'info>(
        ctx: Context<'info, Deliver<'info>>,
        receiver_ix_data: Vec<u8>,
    ) -> Result<()> {
        let (receiver, rest) = ctx
            .remaining_accounts
            .split_first()
            .ok_or(ProgramError::NotEnoughAccountKeys)?;
        let mut infos = vec![ctx.accounts.cpi_signer.to_account_info()];
        infos.extend_from_slice(rest);
        let metas = infos
            .iter()
            .map(|a| AccountMeta {
                pubkey: a.key(),
                is_signer: a.key() == ctx.accounts.cpi_signer.key(),
                is_writable: a.is_writable,
            })
            .collect();
        let ix = Instruction {
            program_id: receiver.key(),
            accounts: metas,
            data: receiver_ix_data,
        };
        infos.push(receiver.clone());
        let (_, bump) = Pubkey::find_program_address(
            &[EXTERNAL_EXECUTION_CONFIG_SEED, receiver.key().as_ref()],
            &crate::ID,
        );
        let receiver_key = receiver.key();
        let seeds: &[&[u8]] = &[
            EXTERNAL_EXECUTION_CONFIG_SEED,
            receiver_key.as_ref(),
            &[bump],
        ];
        invoke_signed(&ix, &infos, &[seeds])?;
        Ok(())
    }
}

#[derive(Accounts)]
pub struct Deliver<'info> {
    /// CHECK: PDA of this program for the receiver; signed for in `deliver`.
    pub cpi_signer: UncheckedAccount<'info>,
}
