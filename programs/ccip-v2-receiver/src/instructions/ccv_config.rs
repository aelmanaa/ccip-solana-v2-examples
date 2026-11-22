use anchor_lang::prelude::*;
use ccip_common::messages::FinalityConfig;

use crate::error::ReceiverError;
use crate::event::{CcvPolicyRemoved, CcvPolicySet};
use crate::state::{
    validate_policy, CcvPolicy, Config, SenderCcvPolicy, SourceChainCcvConfig, CCV_CONFIG_SEED,
    CONFIG_SEED, MAX_SENDER_LEN, MAX_SENDER_POLICIES, SOURCE_CHAIN_CCV_CONFIG_V,
};
use crate::ANCHOR_DISCRIMINATOR;

/// The CCV half of a policy.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, Default)]
pub struct CcvLists {
    pub required_ccvs: Vec<Pubkey>,
    pub optional_ccvs: Vec<Pubkey>,
    pub optional_threshold: u8,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug)]
pub struct SetCcvPolicyParams {
    pub source_chain_selector: u64,
    /// `None` sets the chain policy; `Some(sender)` sets the policy for that sender only.
    pub sender: Option<Vec<u8>>,
    /// `None` keeps the stored CCV lists (empty, meaning lane defaults, on a new policy).
    pub ccvs: Option<CcvLists>,
    /// `None` keeps the stored finality (finalized only on a new policy).
    pub allowed_finality_config: Option<FinalityConfig>,
}

/// Creates or updates a CCV and finality policy. The CCV lists and the finality config can be set
/// together or independently: a field passed as `None` keeps its stored value.
pub fn set_ccv_policy(ctx: Context<SetCcvPolicy>, params: SetCcvPolicyParams) -> Result<()> {
    require!(
        params.ccvs.is_some() || params.allowed_finality_config.is_some(),
        ReceiverError::EmptyPolicyUpdate
    );
    let config = &mut ctx.accounts.source_chain_ccv_config;
    if config.version == 0 {
        config.version = SOURCE_CHAIN_CCV_CONFIG_V;
        config.bump = ctx.bumps.source_chain_ccv_config;
        config.source_chain_selector = params.source_chain_selector;
    }

    let slot: &mut CcvPolicy = match &params.sender {
        None => &mut config.chain_policy,
        Some(sender) => {
            require_gte!(
                MAX_SENDER_LEN,
                sender.len(),
                ReceiverError::SenderAddressTooLong
            );
            match config
                .sender_policies
                .iter()
                .position(|p| &p.sender == sender)
            {
                Some(i) => &mut config.sender_policies[i].policy,
                None => {
                    require_gt!(
                        MAX_SENDER_POLICIES,
                        config.sender_policies.len(),
                        ReceiverError::TooManySenderPolicies
                    );
                    config.sender_policies.push(SenderCcvPolicy {
                        sender: sender.clone(),
                        policy: CcvPolicy::default(),
                    });
                    let last = config.sender_policies.len() - 1;
                    &mut config.sender_policies[last].policy
                }
            }
        }
    };

    let mut next = slot.clone();
    if let Some(ccvs) = params.ccvs {
        next.required_ccvs = ccvs.required_ccvs;
        next.optional_ccvs = ccvs.optional_ccvs;
        next.optional_threshold = ccvs.optional_threshold;
    }
    if let Some(finality) = params.allowed_finality_config {
        next.allowed_finality_config = finality;
    }
    validate_policy(&next)?;
    *slot = next.clone();

    emit!(CcvPolicySet {
        source_chain_selector: params.source_chain_selector,
        sender: params.sender.unwrap_or_default(),
        policy: next,
    });
    Ok(())
}

/// Deletes the whole config of a source chain (chain policy and every sender policy) and returns
/// the rent. Messages from that chain then get the lane's default CCVs and finalized-only.
pub fn remove_ccv_config(_ctx: Context<RemoveCcvConfig>, source_chain_selector: u64) -> Result<()> {
    emit!(CcvPolicyRemoved {
        source_chain_selector,
        sender: vec![],
    });
    Ok(())
}

/// Deletes one sender policy; that sender falls back to the chain policy.
pub fn remove_sender_ccv_policy(
    ctx: Context<UpdateCcvConfig>,
    source_chain_selector: u64,
    sender: Vec<u8>,
) -> Result<()> {
    let policies = &mut ctx.accounts.source_chain_ccv_config.sender_policies;
    let i = policies
        .iter()
        .position(|p| p.sender == sender)
        .ok_or(ReceiverError::SenderPolicyNotFound)?;
    policies.remove(i);
    emit!(CcvPolicyRemoved {
        source_chain_selector,
        sender,
    });
    Ok(())
}

#[derive(Accounts)]
#[instruction(params: SetCcvPolicyParams)]
pub struct SetCcvPolicy<'info> {
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(
        init_if_needed,
        seeds = [CCV_CONFIG_SEED, params.source_chain_selector.to_le_bytes().as_ref()],
        bump,
        payer = authority,
        space = ANCHOR_DISCRIMINATOR + SourceChainCcvConfig::INIT_SPACE,
    )]
    pub source_chain_ccv_config: Account<'info, SourceChainCcvConfig>,
    #[account(mut, address = config.owner @ ReceiverError::OnlyOwner)]
    pub authority: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(source_chain_selector: u64)]
pub struct UpdateCcvConfig<'info> {
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(
        mut,
        seeds = [CCV_CONFIG_SEED, source_chain_selector.to_le_bytes().as_ref()],
        bump = source_chain_ccv_config.bump,
    )]
    pub source_chain_ccv_config: Account<'info, SourceChainCcvConfig>,
    #[account(address = config.owner @ ReceiverError::OnlyOwner)]
    pub authority: Signer<'info>,
}

#[derive(Accounts)]
#[instruction(source_chain_selector: u64)]
pub struct RemoveCcvConfig<'info> {
    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,
    #[account(
        mut,
        close = authority,
        seeds = [CCV_CONFIG_SEED, source_chain_selector.to_le_bytes().as_ref()],
        bump = source_chain_ccv_config.bump,
    )]
    pub source_chain_ccv_config: Account<'info, SourceChainCcvConfig>,
    #[account(mut, address = config.owner @ ReceiverError::OnlyOwner)]
    pub authority: Signer<'info>,
}
