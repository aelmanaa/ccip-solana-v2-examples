use anchor_lang::prelude::*;
use ccip_common::messages::FinalityConfig;

use crate::error::ReceiverError;

/// Seed of the OffRamp's CPI signer PDA, derived under the OffRamp from this program's id.
pub const EXTERNAL_EXECUTION_CONFIG_SEED: &[u8] = b"external_execution_config";
/// Seed of the router PDA that proves an OffRamp is allowed for a source chain.
pub const ALLOWED_OFFRAMP_SEED: &[u8] = b"allowed_offramp";
/// Seed of the singleton [`Config`].
pub const CONFIG_SEED: &[u8] = b"config";
/// Seed of an [`ApprovedSender`] marker, with the source chain selector and the sender bytes.
pub const APPROVED_SENDER_SEED: &[u8] = b"approved_ccip_sender";
/// Seed of the PDA that owns the tokens this program holds (its ATAs are the token destination).
pub const TOKEN_ADMIN_SEED: &[u8] = b"receiver_token_admin";
/// Seed of the system-owned PDA that pays rent for beneficiary ATAs created in forward mode.
pub const RENT_PAYER_SEED: &[u8] = b"rent_payer";
/// Seed of the singleton [`ReceiverState`].
pub const RECEIVER_STATE_SEED: &[u8] = b"receiver_state";
/// Seed of a [`SourceChainCcvConfig`], with the source chain selector. Shared with the OffRamp,
/// which derives the same address when it asks this program for its CCV and finality policy.
pub const CCV_CONFIG_SEED: &[u8] = ccip_common::ccvs::RECEIVER_CCV_CONFIG_SEED;

/// Maximum length of a remote sender address. The address is a PDA seed, and Solana caps each
/// seed at 32 bytes. EVM senders arrive left-padded to 32 bytes.
pub const MAX_SENDER_LEN: usize = 32;
/// Bytes of message data kept in [`ReceiverState`]. Longer data is still emitted in full.
pub const MAX_STORED_DATA: usize = 256;
/// Maximum CCVs in each of the required and optional lists of a [`CcvPolicy`].
pub const MAX_CCVS: usize = 8;
/// Maximum per-sender overrides in a [`SourceChainCcvConfig`].
pub const MAX_SENDER_POLICIES: usize = 4;
/// Basis-point denominator for the forward-mode cut.
pub const BPS_DENOMINATOR: u16 = 10_000;
/// Finality flag that accepts messages sent with `safe` finality (bit 0).
pub const FINALITY_FLAG_SAFE: u16 = 0x1;

/// Forward-mode payload tag: `"FWD1"`, followed by the 32-byte beneficiary wallet.
pub const FORWARD_TAG: &[u8; 4] = b"FWD1";
/// Exact length of a forward-mode payload.
pub const FORWARD_PAYLOAD_LEN: usize = 36;

pub const CONFIG_V: u8 = 1;
pub const RECEIVER_STATE_V: u8 = 1;
pub const SOURCE_CHAIN_CCV_CONFIG_V: u8 = 1;

#[account]
#[derive(InitSpace, Default, Debug)]
pub struct Config {
    pub version: u8,
    pub bump: u8,
    pub owner: Pubkey,
    pub proposed_owner: Pubkey,
    pub router: Pubkey,
    /// Share of a forwarded token amount this program keeps, in basis points.
    pub forward_bps: u16,
}

impl Config {
    pub fn transfer_ownership(&mut self, owner: Pubkey, proposed_owner: Pubkey) -> Result<()> {
        require_keys_eq!(self.owner, owner, ReceiverError::OnlyOwner);
        require_keys_neq!(
            proposed_owner,
            self.owner,
            ReceiverError::InvalidProposedOwner
        );
        require_keys_neq!(
            proposed_owner,
            Pubkey::default(),
            ReceiverError::InvalidProposedOwner
        );
        self.proposed_owner = proposed_owner;
        Ok(())
    }

    pub fn accept_ownership(&mut self, proposed_owner: Pubkey) -> Result<()> {
        require_keys_eq!(
            self.proposed_owner,
            proposed_owner,
            ReceiverError::OnlyProposedOwner
        );
        self.owner = std::mem::take(&mut self.proposed_owner);
        Ok(())
    }

    pub fn update_router(&mut self, owner: Pubkey, router: Pubkey) -> Result<()> {
        require_keys_eq!(self.owner, owner, ReceiverError::OnlyOwner);
        require_keys_neq!(router, Pubkey::default(), ReceiverError::InvalidRouter);
        self.router = router;
        Ok(())
    }
}

/// Marker PDA: its existence approves one (source chain, sender) pair. It holds no data.
#[account]
#[derive(InitSpace, Default, Debug)]
pub struct ApprovedSender {}

/// How the last delivered message was handled.
#[derive(
    AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, Default, PartialEq, Eq, InitSpace,
)]
pub enum ReceiveMode {
    #[default]
    None,
    /// No token transfer: the data is stored and emitted.
    Data,
    /// A token transfer with data that is not a forward payload: the tokens stay with this program.
    TokensAndData,
    /// A token transfer with a forward payload: the cut stays, the rest goes to the beneficiary.
    Forward,
}

/// The last message this program received, kept so it can be read back without an indexer.
#[account]
#[derive(InitSpace, Debug)]
pub struct ReceiverState {
    pub version: u8,
    pub bump: u8,
    pub received_count: u64,
    pub last_mode: ReceiveMode,
    pub last_message_id: [u8; 32],
    pub last_source_chain_selector: u64,
    #[max_len(MAX_SENDER_LEN)]
    pub last_sender: Vec<u8>,
    /// Full length of the last message's data; only the first [`MAX_STORED_DATA`] bytes are kept.
    pub last_data_len: u32,
    #[max_len(MAX_STORED_DATA)]
    pub last_data: Vec<u8>,
    pub last_token: Pubkey,
    pub last_amount: u64,
    pub last_beneficiary: Pubkey,
    pub last_forwarded: u64,
    pub last_kept: u64,
}

/// One CCV and finality policy, in the shape the OffRamp reads it.
///
/// - `required_ccvs`: every listed CCV must verify the message. `Pubkey::default()` is a marker
///   for "also the lane's default CCVs".
/// - `optional_ccvs` and `optional_threshold`: at least `optional_threshold` of them must verify.
/// - All three empty (and threshold 0) means "the lane's default CCVs".
/// - `allowed_finality_config`: the finality a message may have been sent with. `{0, 0}` accepts
///   finalized messages only; `flags & FINALITY_FLAG_SAFE` also accepts `safe`; `block_depth = n`
///   also accepts a requested depth of `n` blocks or more. Finalized messages are always accepted.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, Default, PartialEq, Eq, InitSpace)]
pub struct CcvPolicy {
    #[max_len(MAX_CCVS)]
    pub required_ccvs: Vec<Pubkey>,
    #[max_len(MAX_CCVS)]
    pub optional_ccvs: Vec<Pubkey>,
    pub optional_threshold: u8,
    pub allowed_finality_config: FinalityConfig,
}

/// A policy that replaces the chain-wide one for a single sender.
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Debug, Default, PartialEq, Eq, InitSpace)]
pub struct SenderCcvPolicy {
    #[max_len(MAX_SENDER_LEN)]
    pub sender: Vec<u8>,
    pub policy: CcvPolicy,
}

/// Per-source-chain CCV and finality policy, read by the OffRamp through
/// `get_ccvs_and_finality_config`. Keyed by source chain only, because that is the one account
/// the OffRamp passes; per-sender overrides therefore live inside it.
#[account]
#[derive(InitSpace, Debug)]
pub struct SourceChainCcvConfig {
    pub version: u8,
    pub bump: u8,
    pub source_chain_selector: u64,
    pub chain_policy: CcvPolicy,
    #[max_len(MAX_SENDER_POLICIES)]
    pub sender_policies: Vec<SenderCcvPolicy>,
}

impl SourceChainCcvConfig {
    /// The policy that applies to `sender`: its override if one exists, else the chain policy.
    pub fn policy_for(&self, sender: &[u8]) -> &CcvPolicy {
        self.sender_policies
            .iter()
            .find(|p| p.sender == sender)
            .map(|p| &p.policy)
            .unwrap_or(&self.chain_policy)
    }

    /// Loads the config from an account that may not exist. `None` when the account is not owned
    /// by this program, which is how a never-configured source chain looks to the OffRamp.
    pub fn try_load(account_info: &AccountInfo) -> Result<Option<Self>> {
        if *account_info.owner != crate::ID {
            return Ok(None);
        }
        let data = account_info.try_borrow_data()?;
        let config = Self::try_deserialize(&mut &data[..])?;
        require_eq!(
            config.version,
            SOURCE_CHAIN_CCV_CONFIG_V,
            ReceiverError::UnsupportedConfigVersion
        );
        Ok(Some(config))
    }
}

/// Validates a policy against the rules the OffRamp applies when it reads it back, so a policy
/// that would make every message unexecutable is refused when it is set.
pub fn validate_policy(policy: &CcvPolicy) -> Result<()> {
    require_gte!(
        MAX_CCVS,
        policy.required_ccvs.len(),
        ReceiverError::TooManyCcvs
    );
    require_gte!(
        MAX_CCVS,
        policy.optional_ccvs.len(),
        ReceiverError::TooManyCcvs
    );
    for ccv in &policy.optional_ccvs {
        require_keys_neq!(
            *ccv,
            Pubkey::default(),
            ReceiverError::DefaultMarkerNotAllowedInOptional
        );
    }
    let mut seen = Vec::with_capacity(policy.required_ccvs.len() + policy.optional_ccvs.len());
    for ccv in policy
        .required_ccvs
        .iter()
        .chain(policy.optional_ccvs.iter())
    {
        require!(!seen.contains(ccv), ReceiverError::DuplicateCcv);
        seen.push(*ccv);
    }
    if policy.optional_ccvs.is_empty() {
        require_eq!(
            policy.optional_threshold,
            0,
            ReceiverError::InvalidOptionalThreshold
        );
    } else {
        // A threshold equal to the list length would make every optional CCV required.
        require_gt!(
            policy.optional_ccvs.len(),
            usize::from(policy.optional_threshold),
            ReceiverError::InvalidOptionalThreshold
        );
        require_gt!(
            policy.optional_threshold,
            0,
            ReceiverError::InvalidOptionalThreshold
        );
    }
    require!(
        policy.allowed_finality_config.flags & !FINALITY_FLAG_SAFE == 0,
        ReceiverError::InvalidFinalityConfig
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pk(seed: u8) -> Pubkey {
        Pubkey::new_from_array([seed; 32])
    }

    fn policy(required: Vec<Pubkey>, optional: Vec<Pubkey>, threshold: u8) -> CcvPolicy {
        CcvPolicy {
            required_ccvs: required,
            optional_ccvs: optional,
            optional_threshold: threshold,
            allowed_finality_config: FinalityConfig::default(),
        }
    }

    #[test]
    fn ownership() {
        let owner = pk(1);
        let mut config = Config {
            owner,
            ..Config::default()
        };
        assert_eq!(
            config.transfer_ownership(pk(9), pk(2)).unwrap_err(),
            ReceiverError::OnlyOwner.into()
        );
        assert_eq!(
            config.transfer_ownership(owner, owner).unwrap_err(),
            ReceiverError::InvalidProposedOwner.into()
        );
        config.transfer_ownership(owner, pk(2)).unwrap();
        assert_eq!(
            config.accept_ownership(pk(3)).unwrap_err(),
            ReceiverError::OnlyProposedOwner.into()
        );
        config.accept_ownership(pk(2)).unwrap();
        assert_eq!(config.owner, pk(2));
        assert_eq!(config.proposed_owner, Pubkey::default());
    }

    #[test]
    fn router() {
        let owner = pk(1);
        let mut config = Config {
            owner,
            ..Config::default()
        };
        assert_eq!(
            config.update_router(owner, Pubkey::default()).unwrap_err(),
            ReceiverError::InvalidRouter.into()
        );
        assert_eq!(
            config.update_router(pk(2), pk(3)).unwrap_err(),
            ReceiverError::OnlyOwner.into()
        );
        config.update_router(owner, pk(3)).unwrap();
    }

    #[test]
    fn valid_policies() {
        validate_policy(&policy(vec![], vec![], 0)).unwrap();
        validate_policy(&policy(vec![Pubkey::default(), pk(1)], vec![], 0)).unwrap();
        validate_policy(&policy(vec![pk(1)], vec![pk(2), pk(3)], 1)).unwrap();
        validate_policy(&policy(vec![], vec![pk(2), pk(3)], 1)).unwrap();
        let mut safe = policy(vec![], vec![], 0);
        safe.allowed_finality_config = FinalityConfig {
            flags: FINALITY_FLAG_SAFE,
            block_depth: 5,
        };
        validate_policy(&safe).unwrap();
    }

    #[test]
    fn invalid_policies() {
        let cases = [
            (
                policy(vec![], vec![Pubkey::default()], 0),
                ReceiverError::DefaultMarkerNotAllowedInOptional,
            ),
            (
                policy(vec![pk(1), pk(1)], vec![], 0),
                ReceiverError::DuplicateCcv,
            ),
            (
                policy(vec![pk(1)], vec![pk(1), pk(2)], 1),
                ReceiverError::DuplicateCcv,
            ),
            (
                policy(vec![], vec![pk(1), pk(2)], 2),
                ReceiverError::InvalidOptionalThreshold,
            ),
            (
                policy(vec![], vec![pk(1), pk(2)], 0),
                ReceiverError::InvalidOptionalThreshold,
            ),
            (
                policy(vec![pk(1)], vec![], 1),
                ReceiverError::InvalidOptionalThreshold,
            ),
            (
                policy((1..=9).map(pk).collect(), vec![], 0),
                ReceiverError::TooManyCcvs,
            ),
        ];
        for (p, err) in cases {
            assert_eq!(validate_policy(&p).unwrap_err(), err.into(), "{p:?}");
        }
        let mut bad_flags = policy(vec![], vec![], 0);
        bad_flags.allowed_finality_config.flags = 0x2;
        assert_eq!(
            validate_policy(&bad_flags).unwrap_err(),
            ReceiverError::InvalidFinalityConfig.into()
        );
    }

    #[test]
    fn sender_override_takes_precedence() {
        let chain_policy = policy(vec![pk(1)], vec![], 0);
        let sender_policy = policy(vec![pk(2)], vec![], 0);
        let config = SourceChainCcvConfig {
            version: SOURCE_CHAIN_CCV_CONFIG_V,
            bump: 0,
            source_chain_selector: 1,
            chain_policy: chain_policy.clone(),
            sender_policies: vec![SenderCcvPolicy {
                sender: vec![7; 32],
                policy: sender_policy.clone(),
            }],
        };
        assert_eq!(config.policy_for(&[7; 32]), &sender_policy);
        assert_eq!(config.policy_for(&[8; 32]), &chain_policy);
    }
}
