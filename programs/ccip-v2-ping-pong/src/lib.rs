//! Example CCIP 2.0 program for Solana that both sends and receives.
//!
//! `ping` sends `abi.encode(uint8 1, uint64 seq)` to the peer configured for a chain. A ping from
//! that peer is answered with `abi.encode(uint8 2, uint64 seq)`, either from inside
//! `ccip_receive_v2` (`ReplyMode::Immediate`) or later through the permissionless
//! `send_pending_reply` (`ReplyMode::Deferred`). The program's `ccip_signer` PDA signs every send
//! and pays its fee in native SOL. See the repository README for the trade-offs.
use anchor_lang::prelude::*;
use anchor_lang::solana_program::{
    instruction::{AccountMeta, Instruction},
    program::{get_return_data, invoke_signed},
};
use anchor_lang::system_program;
use anchor_lang::InstructionData;
use ccip_common::ccvs::{GetReceiverCcvsIn, GetReceiverCcvsOut};
use ccip_common::messages::{FinalityConfig, SVM2AnyMessage};
use ccip_router::instruction::CcipSendV2 as RouterCcipSendV2;
use ccip_router::CcipSendV2Params;

pub mod error;
pub mod message;
pub mod register;
pub mod state;

use error::PingPongError;
use message::Any2SVMMessage;
use register::*;
use state::*;

declare_id!("6A4neXDjavAyHnkGcVBckV2hQpBQhp7bXT2BS5JPR7Mh");

pub const ANCHOR_DISCRIMINATOR: usize = 8;

#[event]
pub struct PingSent {
    pub dest_chain_selector: u64,
    pub sequence: u64,
    pub message_id: [u8; 32],
}

#[event]
pub struct PongSent {
    pub dest_chain_selector: u64,
    pub sequence: u64,
    pub message_id: [u8; 32],
    pub immediate: bool,
}

#[event]
pub struct PingReceived {
    pub source_chain_selector: u64,
    pub sequence: u64,
    pub message_id: [u8; 32],
    pub reply_mode: ReplyMode,
}

#[event]
pub struct PongReceived {
    pub source_chain_selector: u64,
    pub sequence: u64,
    pub message_id: [u8; 32],
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct SetPeerParams {
    pub chain_selector: u64,
    pub address: [u8; PEER_ADDRESS_LEN],
    pub accepted_sender: [u8; PEER_ADDRESS_LEN],
    pub extra_args: Vec<u8>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct CcvLists {
    pub required_ccvs: Vec<Pubkey>,
    pub optional_ccvs: Vec<Pubkey>,
    pub optional_threshold: u8,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct SetCcvPolicyParams {
    pub source_chain_selector: u64,
    pub ccvs: Option<CcvLists>,
    pub allowed_finality_config: Option<FinalityConfig>,
}

#[program]
pub mod ccip_v2_ping_pong {
    use super::*;

    /// Upgrade authority only.
    pub fn initialize(
        ctx: Context<Initialize>,
        router: Pubkey,
        reply_mode: ReplyMode,
    ) -> Result<()> {
        require_keys_neq!(router, Pubkey::default(), PingPongError::InvalidRouter);
        ctx.accounts.config.set_inner(Config {
            version: CONFIG_V,
            bump: ctx.bumps.config,
            ccip_signer_bump: ctx.bumps.ccip_signer,
            owner: ctx.accounts.authority.key(),
            proposed_owner: Pubkey::default(),
            router,
            reply_mode,
        });
        ctx.accounts.state.bump = ctx.bumps.state;
        Ok(())
    }

    pub fn register_with_router(ctx: Context<RegisterWithRouter>) -> Result<()> {
        register::register_with_router(ctx)
    }

    /// Sets the peer for a chain: where pings and pongs go, the only sender accepted from that
    /// chain, and the extra args for messages to it. Owner only.
    pub fn set_peer(ctx: Context<SetPeer>, params: SetPeerParams) -> Result<()> {
        require_gte!(
            MAX_EXTRA_ARGS_LEN,
            params.extra_args.len(),
            PingPongError::ExtraArgsTooLong
        );
        let peer = &mut ctx.accounts.peer;
        peer.bump = ctx.bumps.peer;
        peer.chain_selector = params.chain_selector;
        peer.address = params.address;
        peer.accepted_sender = params.accepted_sender;
        peer.extra_args = params.extra_args;
        Ok(())
    }

    /// Removes a chain's peer: no more sends to it, and its messages are refused. Owner only.
    pub fn remove_peer(_ctx: Context<RemovePeer>, _chain_selector: u64) -> Result<()> {
        Ok(())
    }

    pub fn set_reply_mode(ctx: Context<UpdateConfig>, reply_mode: ReplyMode) -> Result<()> {
        ctx.accounts.config.reply_mode = reply_mode;
        Ok(())
    }

    /// Sends a ping to the chain's peer. Owner only; the fee comes from the `ccip_signer` PDA.
    /// `remaining_accounts` are the router's `ccip_send_v2` accounts resolved for that PDA.
    pub fn ping<'info>(
        ctx: Context<'info, Ping<'info>>,
        dest_chain_selector: u64,
        resolution_metadata: Vec<u8>,
    ) -> Result<()> {
        let state = &mut ctx.accounts.state;
        let sequence = state
            .pings_sent
            .checked_add(1)
            .ok_or(PingPongError::Overflow)?;
        let message_id = send_to_peer(
            &ctx.accounts.router,
            &ctx.accounts.ccip_signer,
            ctx.remaining_accounts,
            ctx.accounts.config.ccip_signer_bump,
            &ctx.accounts.peer,
            encode_payload(KIND_PING, sequence),
            resolution_metadata,
        )?;
        state.pings_sent = sequence;
        emit!(PingSent {
            dest_chain_selector,
            sequence,
            message_id,
        });
        Ok(())
    }

    /// Sends the oldest queued reply for a chain. Anyone may call it; the fee comes from the
    /// `ccip_signer` PDA. `remaining_accounts` are the router's `ccip_send_v2` accounts.
    pub fn send_pending_reply<'info>(
        ctx: Context<'info, SendPendingReply<'info>>,
        chain_selector: u64,
        resolution_metadata: Vec<u8>,
    ) -> Result<()> {
        require!(
            !ctx.accounts.peer.pending_replies.is_empty(),
            PingPongError::NoPendingReply
        );
        let sequence = ctx.accounts.peer.pending_replies[0];
        let message_id = send_to_peer(
            &ctx.accounts.router,
            &ctx.accounts.ccip_signer,
            ctx.remaining_accounts,
            ctx.accounts.config.ccip_signer_bump,
            &ctx.accounts.peer,
            encode_payload(KIND_PONG, sequence),
            resolution_metadata,
        )?;
        ctx.accounts.peer.pending_replies.remove(0);
        let state = &mut ctx.accounts.state;
        state.pongs_sent = state
            .pongs_sent
            .checked_add(1)
            .ok_or(PingPongError::Overflow)?;
        emit!(PongSent {
            dest_chain_selector: chain_selector,
            sequence,
            message_id,
            immediate: false,
        });
        Ok(())
    }

    /// Delivers a message from the peer.
    ///
    /// A pong is recorded. A ping is recorded and answered: in `Deferred` mode its sequence is
    /// queued on the peer account for `send_pending_reply`, in `Immediate` mode the pong is sent here
    /// (remaining accounts: the router program, then its `ccip_send_v2` accounts). Bytes after
    /// the 64-byte payload are passed to the router as resolution metadata in `Immediate` mode.
    pub fn ccip_receive_v2<'info>(
        ctx: Context<'info, CcipReceiveV2<'info>>,
        message: Any2SVMMessage,
    ) -> Result<()> {
        require!(
            message.data.len() >= PAYLOAD_LEN,
            PingPongError::InvalidPayload
        );
        let (kind, sequence) = decode_payload(&message.data[..PAYLOAD_LEN])?;
        let source = message.source_chain_selector;
        let state = &mut ctx.accounts.state;
        state.last_message_id = message.message_id;
        state.last_kind = kind;
        state.last_sequence = sequence;
        state.last_source_chain_selector = source;

        if kind == KIND_PONG {
            state.pongs_received = state
                .pongs_received
                .checked_add(1)
                .ok_or(PingPongError::Overflow)?;
            emit!(PongReceived {
                source_chain_selector: source,
                sequence,
                message_id: message.message_id,
            });
            return Ok(());
        }

        state.pings_received = state
            .pings_received
            .checked_add(1)
            .ok_or(PingPongError::Overflow)?;
        let reply_mode = ctx.accounts.config.reply_mode;
        emit!(PingReceived {
            source_chain_selector: source,
            sequence,
            message_id: message.message_id,
            reply_mode,
        });

        let signer_bump = ctx.accounts.config.ccip_signer_bump;
        match reply_mode {
            ReplyMode::Deferred => {
                let peer = &mut ctx.accounts.peer;
                require_gt!(
                    MAX_PENDING_REPLIES,
                    peer.pending_replies.len(),
                    PingPongError::PendingQueueFull
                );
                peer.pending_replies.push(sequence);
                Ok(())
            }
            ReplyMode::Immediate => {
                let (router, router_accounts) = ctx
                    .remaining_accounts
                    .split_first()
                    .ok_or(PingPongError::MissingRouterAccounts)?;
                require_keys_eq!(
                    router.key(),
                    ctx.accounts.config.router,
                    PingPongError::InvalidRouter
                );
                let message_id = send_to_peer(
                    router,
                    &ctx.accounts.ccip_signer,
                    router_accounts,
                    signer_bump,
                    &ctx.accounts.peer,
                    encode_payload(KIND_PONG, sequence),
                    message.data[PAYLOAD_LEN..].to_vec(),
                )?;
                let state = &mut ctx.accounts.state;
                state.pongs_sent = state
                    .pongs_sent
                    .checked_add(1)
                    .ok_or(PingPongError::Overflow)?;
                emit!(PongSent {
                    dest_chain_selector: source,
                    sequence,
                    message_id,
                    immediate: true,
                });
                Ok(())
            }
        }
    }

    /// CCIP 2.0 receiver interface: the CCVs and finality required for messages from a chain.
    pub fn get_ccvs_and_finality_config(
        ctx: Context<GetCcvsAndFinalityConfig>,
        _input: GetReceiverCcvsIn,
    ) -> Result<GetReceiverCcvsOut> {
        Ok(
            match SourceChainCcvConfig::try_load(&ctx.accounts.source_chain_ccv_config)? {
                None => GetReceiverCcvsOut::default(),
                Some(c) => GetReceiverCcvsOut {
                    required_ccvs: c.required_ccvs,
                    optional_ccvs: c.optional_ccvs,
                    optional_threshold: c.optional_threshold,
                    allowed_finality_config: c.allowed_finality_config,
                },
            },
        )
    }

    /// Creates or updates a source chain's CCV and finality policy; `None` keeps a field. Owner only.
    pub fn set_ccv_policy(ctx: Context<SetCcvPolicy>, params: SetCcvPolicyParams) -> Result<()> {
        let c = &mut ctx.accounts.source_chain_ccv_config;
        c.version = CCV_CONFIG_V;
        c.bump = ctx.bumps.source_chain_ccv_config;
        if let Some(ccvs) = params.ccvs {
            c.required_ccvs = ccvs.required_ccvs;
            c.optional_ccvs = ccvs.optional_ccvs;
            c.optional_threshold = ccvs.optional_threshold;
        }
        if let Some(f) = params.allowed_finality_config {
            c.allowed_finality_config = f;
        }
        c.validate()
    }

    /// Removes a source chain's policy (back to lane defaults, finalized only). Owner only.
    pub fn remove_ccv_config(
        _ctx: Context<RemoveCcvConfig>,
        _source_chain_selector: u64,
    ) -> Result<()> {
        Ok(())
    }

    /// Moves lamports out of the `ccip_signer` PDA. Owner only.
    pub fn withdraw_signer(ctx: Context<WithdrawSigner>, lamports: u64) -> Result<()> {
        let seeds: &[&[u8]] = &[CCIP_SIGNER_SEED, &[ctx.accounts.config.ccip_signer_bump]];
        system_program::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.system_program.key(),
                system_program::Transfer {
                    from: ctx.accounts.ccip_signer.to_account_info(),
                    to: ctx.accounts.to.to_account_info(),
                },
                &[seeds],
            ),
            lamports,
        )
    }

    pub fn transfer_ownership(ctx: Context<UpdateConfig>, proposed_owner: Pubkey) -> Result<()> {
        require_keys_neq!(
            proposed_owner,
            Pubkey::default(),
            PingPongError::InvalidProposedOwner
        );
        require_keys_neq!(
            proposed_owner,
            ctx.accounts.config.owner,
            PingPongError::InvalidProposedOwner
        );
        ctx.accounts.config.proposed_owner = proposed_owner;
        Ok(())
    }

    pub fn accept_ownership(ctx: Context<AcceptOwnership>) -> Result<()> {
        let config = &mut ctx.accounts.config;
        config.owner = std::mem::take(&mut config.proposed_owner);
        Ok(())
    }
}

/// CPIs the router's `ccip_send_v2` with the `ccip_signer` PDA as the sender, paying in native
/// SOL, and returns the message id.
fn send_to_peer<'info>(
    router: &AccountInfo<'info>,
    ccip_signer: &AccountInfo<'info>,
    router_accounts: &[AccountInfo<'info>],
    signer_bump: u8,
    peer: &Peer,
    data: Vec<u8>,
    resolution_metadata: Vec<u8>,
) -> Result<[u8; 32]> {
    require!(
        !router_accounts.is_empty(),
        PingPongError::MissingRouterAccounts
    );
    let signer_key = ccip_signer.key();
    let ix = Instruction {
        program_id: router.key(),
        accounts: router_accounts
            .iter()
            .map(|a| AccountMeta {
                pubkey: a.key(),
                is_signer: a.key() == signer_key,
                is_writable: a.is_writable,
            })
            .collect(),
        data: RouterCcipSendV2 {
            params: CcipSendV2Params {
                dest_chain_selector: peer.chain_selector,
                message: SVM2AnyMessage {
                    receiver: peer.address.to_vec(),
                    data,
                    token_amounts: vec![],
                    fee_token: Pubkey::default(),
                    extra_args: peer.extra_args.clone(),
                },
                resolution_metadata,
            },
        }
        .data(),
    };
    let mut infos = Vec::with_capacity(router_accounts.len() + 1);
    infos.push(router.clone());
    infos.extend_from_slice(router_accounts);
    let seeds: &[&[u8]] = &[CCIP_SIGNER_SEED, &[signer_bump]];
    invoke_signed(&ix, &infos, &[seeds])?;

    let (program, ret) = get_return_data().ok_or(PingPongError::MissingMessageId)?;
    require_keys_eq!(program, router.key(), PingPongError::MissingMessageId);
    <[u8; 32]>::try_from(ret.as_slice()).map_err(|_| error!(PingPongError::MissingMessageId))
}

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(init, seeds = [CONFIG_SEED], bump, payer = authority,
        space = ANCHOR_DISCRIMINATOR + Config::INIT_SPACE)]
    pub config: Account<'info, Config>,
    #[account(init, seeds = [STATE_SEED], bump, payer = authority,
        space = ANCHOR_DISCRIMINATOR + PingPongState::INIT_SPACE)]
    pub state: Account<'info, PingPongState>,
    /// CHECK: only its bump is read.
    #[account(seeds = [CCIP_SIGNER_SEED], bump)]
    pub ccip_signer: UncheckedAccount<'info>,
    #[account(mut)]
    pub authority: Signer<'info>,
    pub system_program: Program<'info, System>,
    #[account(constraint = program.programdata_address()? == Some(program_data.key()))]
    pub program: Program<'info, program::CcipV2PingPong>,
    #[account(constraint = program_data.upgrade_authority_address == Some(authority.key())
        @ PingPongError::Unauthorized)]
    pub program_data: Account<'info, ProgramData>,
}

#[derive(Accounts)]
pub struct UpdateConfig<'info> {
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(address = config.owner @ PingPongError::OnlyOwner)]
    pub authority: Signer<'info>,
}

#[derive(Accounts)]
pub struct AcceptOwnership<'info> {
    #[account(mut, seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(address = config.proposed_owner @ PingPongError::OnlyProposedOwner)]
    pub authority: Signer<'info>,
}

#[derive(Accounts)]
#[instruction(params: SetPeerParams)]
pub struct SetPeer<'info> {
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(init_if_needed, seeds = [PEER_SEED, params.chain_selector.to_le_bytes().as_ref()],
        bump, payer = authority, space = ANCHOR_DISCRIMINATOR + Peer::INIT_SPACE)]
    pub peer: Account<'info, Peer>,
    #[account(mut, address = config.owner @ PingPongError::OnlyOwner)]
    pub authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(chain_selector: u64)]
pub struct RemovePeer<'info> {
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(mut, close = authority, seeds = [PEER_SEED, chain_selector.to_le_bytes().as_ref()],
        bump = peer.bump)]
    pub peer: Account<'info, Peer>,
    #[account(mut, address = config.owner @ PingPongError::OnlyOwner)]
    pub authority: Signer<'info>,
}

#[derive(Accounts)]
#[instruction(dest_chain_selector: u64)]
pub struct Ping<'info> {
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(mut, seeds = [STATE_SEED], bump = state.bump)]
    pub state: Account<'info, PingPongState>,
    #[account(seeds = [PEER_SEED, dest_chain_selector.to_le_bytes().as_ref()], bump = peer.bump)]
    pub peer: Account<'info, Peer>,
    /// CHECK: system-owned PDA that signs the send and pays its fee.
    #[account(mut, seeds = [CCIP_SIGNER_SEED], bump = config.ccip_signer_bump)]
    pub ccip_signer: UncheckedAccount<'info>,
    /// CHECK: the configured router, target of the CPI.
    #[account(address = config.router @ PingPongError::InvalidRouter)]
    pub router: UncheckedAccount<'info>,
    #[account(address = config.owner @ PingPongError::OnlyOwner)]
    pub authority: Signer<'info>,
}

#[derive(Accounts)]
#[instruction(chain_selector: u64)]
pub struct SendPendingReply<'info> {
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(mut, seeds = [STATE_SEED], bump = state.bump)]
    pub state: Account<'info, PingPongState>,
    #[account(mut, seeds = [PEER_SEED, chain_selector.to_le_bytes().as_ref()], bump = peer.bump)]
    pub peer: Account<'info, Peer>,
    /// CHECK: system-owned PDA that signs the send and pays its fee.
    #[account(mut, seeds = [CCIP_SIGNER_SEED], bump = config.ccip_signer_bump)]
    pub ccip_signer: UncheckedAccount<'info>,
    /// CHECK: the configured router, target of the CPI.
    #[account(address = config.router @ PingPongError::InvalidRouter)]
    pub router: UncheckedAccount<'info>,
}

#[derive(Accounts)]
#[instruction(message: Any2SVMMessage)]
pub struct CcipReceiveV2<'info> {
    #[account(
        seeds = [EXTERNAL_EXECUTION_CONFIG_SEED, crate::ID.as_ref()],
        bump,
        seeds::program = offramp_program.key(),
    )]
    pub authority: Signer<'info>,
    /// CHECK: the OffRamp program.
    pub offramp_program: UncheckedAccount<'info>,
    /// CHECK: router PDA proving the OffRamp is allowed for the source chain.
    #[account(
        owner = config.router @ PingPongError::InvalidCaller,
        seeds = [ALLOWED_OFFRAMP_SEED, message.source_chain_selector.to_le_bytes().as_ref(), offramp_program.key().as_ref()],
        bump,
        seeds::program = config.router,
    )]
    pub allowed_offramp: UncheckedAccount<'info>,
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(
        mut,
        seeds = [PEER_SEED, message.source_chain_selector.to_le_bytes().as_ref()],
        bump = peer.bump,
        constraint = peer.accepted_sender.as_slice() == message.sender.as_slice() @ PingPongError::NotPeer,
    )]
    pub peer: Account<'info, Peer>,
    #[account(mut, seeds = [STATE_SEED], bump = state.bump)]
    pub state: Account<'info, PingPongState>,
    /// CHECK: system-owned PDA that signs and pays for immediate pongs.
    #[account(mut, seeds = [CCIP_SIGNER_SEED], bump = config.ccip_signer_bump)]
    pub ccip_signer: UncheckedAccount<'info>,
}

#[derive(Accounts)]
#[instruction(input: GetReceiverCcvsIn)]
pub struct GetCcvsAndFinalityConfig<'info> {
    /// CHECK: may not exist; see `try_load`.
    #[account(seeds = [CCV_CONFIG_SEED, input.source_chain_selector.to_le_bytes().as_ref()], bump)]
    pub source_chain_ccv_config: UncheckedAccount<'info>,
}

#[derive(Accounts)]
#[instruction(params: SetCcvPolicyParams)]
pub struct SetCcvPolicy<'info> {
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(init_if_needed,
        seeds = [CCV_CONFIG_SEED, params.source_chain_selector.to_le_bytes().as_ref()],
        bump, payer = authority, space = ANCHOR_DISCRIMINATOR + SourceChainCcvConfig::INIT_SPACE)]
    pub source_chain_ccv_config: Account<'info, SourceChainCcvConfig>,
    #[account(mut, address = config.owner @ PingPongError::OnlyOwner)]
    pub authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(source_chain_selector: u64)]
pub struct RemoveCcvConfig<'info> {
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(mut, close = authority,
        seeds = [CCV_CONFIG_SEED, source_chain_selector.to_le_bytes().as_ref()],
        bump = source_chain_ccv_config.bump)]
    pub source_chain_ccv_config: Account<'info, SourceChainCcvConfig>,
    #[account(mut, address = config.owner @ PingPongError::OnlyOwner)]
    pub authority: Signer<'info>,
}

#[derive(Accounts)]
pub struct WithdrawSigner<'info> {
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    /// CHECK: system-owned PDA.
    #[account(mut, seeds = [CCIP_SIGNER_SEED], bump = config.ccip_signer_bump)]
    pub ccip_signer: UncheckedAccount<'info>,
    /// CHECK: any account may receive lamports.
    #[account(mut)]
    pub to: UncheckedAccount<'info>,
    #[account(address = config.owner @ PingPongError::OnlyOwner)]
    pub authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}
