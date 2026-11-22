use anchor_lang::prelude::*;
use anchor_spl::associated_token::{
    self, get_associated_token_address_with_program_id, AssociatedToken,
};
use anchor_spl::token_interface::{self, Mint, TransferChecked};

use crate::error::ReceiverError;
use crate::event::{MessageReceived, TokensForwarded};
use crate::message::{Any2SVMMessage, TokenTransfer};
use crate::state::{
    ApprovedSender, Config, ReceiveMode, ReceiverState, ALLOWED_OFFRAMP_SEED, APPROVED_SENDER_SEED,
    BPS_DENOMINATOR, CONFIG_SEED, EXTERNAL_EXECUTION_CONFIG_SEED, FORWARD_PAYLOAD_LEN, FORWARD_TAG,
    MAX_STORED_DATA, RECEIVER_STATE_SEED, RENT_PAYER_SEED, TOKEN_ADMIN_SEED,
};

/// Remaining accounts a forward-mode message must carry, in this order.
pub const FORWARD_ACCOUNTS: usize = 9;

struct Forwarded {
    beneficiary: Pubkey,
    forwarded: u64,
    kept: u64,
}

/// Called by the CCIP OffRamp to deliver a message.
///
/// - No token transfer: the data is stored in `ReceiverState` and emitted.
/// - A token transfer with a forward payload (`"FWD1" || beneficiary`): `forward_bps` of the
///   amount stays with this program and the rest is sent to the beneficiary's ATA, which is
///   created if needed.
/// - A token transfer with any other data: the tokens stay in the token admin's ATA.
///
/// A token transfer with no data and no receiver gas never reaches this instruction: the OffRamp
/// delivers it as a token-only transfer.
pub fn ccip_receive_v2<'info>(
    ctx: Context<'info, CcipReceiveV2<'info>>,
    message: Any2SVMMessage,
) -> Result<()> {
    let (mode, forwarded) = match &message.token_transfer {
        None => (ReceiveMode::Data, None),
        Some(transfer) if message.data.starts_with(FORWARD_TAG) => (
            ReceiveMode::Forward,
            Some(forward_tokens(&ctx, &message, transfer)?),
        ),
        Some(_) => (ReceiveMode::TokensAndData, None),
    };

    let (token, amount) = message
        .token_transfer
        .as_ref()
        .map(|t| (t.token_amount.token, t.token_amount.amount))
        .unwrap_or_default();

    let state = &mut ctx.accounts.receiver_state;
    state.received_count = state
        .received_count
        .checked_add(1)
        .ok_or(ReceiverError::Overflow)?;
    state.last_mode = mode;
    state.last_message_id = message.message_id;
    state.last_source_chain_selector = message.source_chain_selector;
    state.last_sender = message.sender.clone();
    state.last_data_len = u32::try_from(message.data.len()).map_err(|_| ReceiverError::Overflow)?;
    state.last_data = message.data[..message.data.len().min(MAX_STORED_DATA)].to_vec();
    state.last_token = token;
    state.last_amount = amount;
    let f = forwarded.unwrap_or(Forwarded {
        beneficiary: Pubkey::default(),
        forwarded: 0,
        kept: 0,
    });
    state.last_beneficiary = f.beneficiary;
    state.last_forwarded = f.forwarded;
    state.last_kept = f.kept;

    emit!(MessageReceived {
        message_id: message.message_id,
        source_chain_selector: message.source_chain_selector,
        sender: message.sender,
        mode,
        data: message.data,
        token,
        amount,
    });
    Ok(())
}

fn forward_tokens<'info>(
    ctx: &Context<'info, CcipReceiveV2<'info>>,
    message: &Any2SVMMessage,
    transfer: &TokenTransfer,
) -> Result<Forwarded> {
    require_eq!(
        message.data.len(),
        FORWARD_PAYLOAD_LEN,
        ReceiverError::InvalidForwardPayload
    );
    let beneficiary = Pubkey::try_from(&message.data[FORWARD_TAG.len()..])
        .map_err(|_| ReceiverError::InvalidForwardPayload)?;

    let accs = ctx.remaining_accounts;
    require_gte!(
        accs.len(),
        FORWARD_ACCOUNTS,
        ReceiverError::InvalidForwardAccounts
    );
    let [token_admin, admin_ata, mint, token_program, beneficiary_info, beneficiary_ata, ata_program, system_program, rent_payer] =
        &accs[..FORWARD_ACCOUNTS]
    else {
        return err!(ReceiverError::InvalidForwardAccounts);
    };

    let (token_admin_key, token_admin_bump) =
        Pubkey::find_program_address(&[TOKEN_ADMIN_SEED], &crate::ID);
    let (rent_payer_key, rent_payer_bump) =
        Pubkey::find_program_address(&[RENT_PAYER_SEED], &crate::ID);
    let mint_key = transfer.token_amount.token;
    let token_program_key = token_program.key();

    // The OffRamp credited the ATA of `token_receiver`; forwarding only makes sense from ours.
    require_keys_eq!(
        transfer.token_receiver,
        token_admin_key,
        ReceiverError::TokensNotDeliveredHere
    );
    require!(
        transfer.token_receiver_usage.should_derive_ata(),
        ReceiverError::TokensNotDeliveredHere
    );
    require!(
        token_program_key == anchor_spl::token::ID
            || token_program_key == anchor_spl::token_2022::ID,
        ReceiverError::InvalidForwardAccounts
    );
    require_keys_eq!(
        *mint.owner,
        token_program_key,
        ReceiverError::InvalidForwardAccounts
    );
    let expected = [
        (token_admin.key(), token_admin_key),
        (
            admin_ata.key(),
            get_associated_token_address_with_program_id(
                &token_admin_key,
                &mint_key,
                &token_program_key,
            ),
        ),
        (mint.key(), mint_key),
        (beneficiary_info.key(), beneficiary),
        (
            beneficiary_ata.key(),
            get_associated_token_address_with_program_id(
                &beneficiary,
                &mint_key,
                &token_program_key,
            ),
        ),
        (ata_program.key(), AssociatedToken::id()),
        (system_program.key(), System::id()),
        (rent_payer.key(), rent_payer_key),
    ];
    for (got, want) in expected {
        require_keys_eq!(got, want, ReceiverError::InvalidForwardAccounts);
    }

    let amount = transfer.token_amount.amount;
    let bps = ctx.accounts.config.forward_bps;
    let kept = u64::try_from(
        u128::from(amount)
            .checked_mul(u128::from(bps))
            .ok_or(ReceiverError::Overflow)?
            / u128::from(BPS_DENOMINATOR),
    )
    .map_err(|_| ReceiverError::Overflow)?;
    let forwarded = amount.checked_sub(kept).ok_or(ReceiverError::Overflow)?;

    let rent_payer_seeds: &[&[u8]] = &[RENT_PAYER_SEED, &[rent_payer_bump]];
    associated_token::create_idempotent(CpiContext::new_with_signer(
        ata_program.key(),
        associated_token::Create {
            payer: rent_payer.clone(),
            associated_token: beneficiary_ata.clone(),
            authority: beneficiary_info.clone(),
            mint: mint.clone(),
            system_program: system_program.clone(),
            token_program: token_program.clone(),
        },
        &[rent_payer_seeds],
    ))?;

    if forwarded > 0 {
        let decimals = {
            let data = mint.try_borrow_data()?;
            Mint::try_deserialize(&mut &data[..])?.decimals
        };
        let token_admin_seeds: &[&[u8]] = &[TOKEN_ADMIN_SEED, &[token_admin_bump]];
        token_interface::transfer_checked(
            CpiContext::new_with_signer(
                token_program_key,
                TransferChecked {
                    from: admin_ata.clone(),
                    mint: mint.clone(),
                    to: beneficiary_ata.clone(),
                    authority: token_admin.clone(),
                },
                &[token_admin_seeds],
            ),
            forwarded,
            decimals,
        )?;
    }

    emit!(TokensForwarded {
        message_id: message.message_id,
        beneficiary,
        mint: mint_key,
        amount,
        forwarded,
        kept,
    });
    Ok(Forwarded {
        beneficiary,
        forwarded,
        kept,
    })
}

#[derive(Accounts)]
#[instruction(message: Any2SVMMessage)]
pub struct CcipReceiveV2<'info> {
    /// The OffRamp's CPI signer for this receiver, derived under the OffRamp program. Only that
    /// program can sign for it, so its signature proves who is calling. Must be first.
    #[account(
        seeds = [EXTERNAL_EXECUTION_CONFIG_SEED, crate::ID.as_ref()],
        bump,
        seeds::program = offramp_program.key(),
    )]
    pub authority: Signer<'info>,

    /// CHECK: the OffRamp program; used to derive `authority` and `allowed_offramp`. Must be second.
    pub offramp_program: UncheckedAccount<'info>,

    /// CHECK: router PDA that exists, owned by the configured router, only for OffRamps the router
    /// allows for this source chain. Must be third.
    #[account(
        owner = config.router @ ReceiverError::InvalidCaller,
        seeds = [
            ALLOWED_OFFRAMP_SEED,
            message.source_chain_selector.to_le_bytes().as_ref(),
            offramp_program.key().as_ref(),
        ],
        bump,
        seeds::program = config.router,
    )]
    pub allowed_offramp: UncheckedAccount<'info>,

    // From here on, the accounts are this receiver's own. The sender lists them, in this order,
    // in the message's `SVMExecutorArgsV1.accounts`.
    /// Exists only for approved (source chain, sender) pairs.
    #[account(
        seeds = [
            APPROVED_SENDER_SEED,
            message.source_chain_selector.to_le_bytes().as_ref(),
            &message.sender,
        ],
        bump,
    )]
    pub approved_sender: Account<'info, ApprovedSender>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(mut, seeds = [RECEIVER_STATE_SEED], bump = receiver_state.bump)]
    pub receiver_state: Account<'info, ReceiverState>,
    // Forward mode appends FORWARD_ACCOUNTS remaining accounts: token_admin, its ATA (writable),
    // mint, token program, beneficiary, beneficiary ATA (writable), associated token program,
    // system program, rent_payer (writable).
}
