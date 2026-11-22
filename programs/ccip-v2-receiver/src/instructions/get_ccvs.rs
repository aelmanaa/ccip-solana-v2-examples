use anchor_lang::prelude::*;
use ccip_common::ccvs::{GetReceiverCcvsIn, GetReceiverCcvsOut};

use crate::state::{SourceChainCcvConfig, CCV_CONFIG_SEED};

/// The CCIP 2.0 receiver interface: the OffRamp calls this (by CPI, before it executes a message)
/// to learn which CCVs must have verified the message and which finality the receiver accepts.
///
/// Returns the sender's own policy when one is stored, otherwise the chain policy, otherwise (no
/// config for the source chain) an empty response, which the OffRamp reads as "lane default CCVs,
/// finalized messages only".
pub fn get_ccvs_and_finality_config(
    ctx: Context<GetCcvsAndFinalityConfig>,
    input: GetReceiverCcvsIn,
) -> Result<GetReceiverCcvsOut> {
    let Some(config) = SourceChainCcvConfig::try_load(&ctx.accounts.source_chain_ccv_config)?
    else {
        return Ok(GetReceiverCcvsOut::default());
    };
    let policy = config.policy_for(&input.sender);
    Ok(GetReceiverCcvsOut {
        required_ccvs: policy.required_ccvs.clone(),
        optional_ccvs: policy.optional_ccvs.clone(),
        optional_threshold: policy.optional_threshold,
        allowed_finality_config: policy.allowed_finality_config,
    })
}

#[derive(Accounts)]
#[instruction(input: GetReceiverCcvsIn)]
pub struct GetCcvsAndFinalityConfig<'info> {
    /// CHECK: the source chain's config PDA. It may not exist; `try_load` handles that. This is
    /// the only account the OffRamp passes.
    #[account(seeds = [CCV_CONFIG_SEED, input.source_chain_selector.to_le_bytes().as_ref()], bump)]
    pub source_chain_ccv_config: UncheckedAccount<'info>,
}
