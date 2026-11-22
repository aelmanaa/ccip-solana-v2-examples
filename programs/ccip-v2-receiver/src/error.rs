use anchor_lang::prelude::*;

#[error_code]
pub enum ReceiverError {
    #[msg("Invalid router address")]
    InvalidRouter,
    #[msg("Address is not owner")]
    OnlyOwner,
    #[msg("Address is not proposed_owner")]
    OnlyProposedOwner,
    #[msg("Caller is not an allowed OffRamp")]
    InvalidCaller,
    #[msg("Proposed owner is invalid")]
    InvalidProposedOwner,
    #[msg("Remote sender address is longer than 32 bytes")]
    SenderAddressTooLong,
    #[msg("Address is not the program upgrade authority")]
    Unauthorized,
    #[msg("Basis points must be at most 10000")]
    InvalidBps,
    #[msg("Arithmetic overflow")]
    Overflow,
    #[msg("A forward payload needs a token transfer")]
    ForwardWithoutTokens,
    #[msg("A forward payload is \"FWD1\" followed by a 32-byte wallet")]
    InvalidForwardPayload,
    #[msg("The forward accounts are missing or do not match the message")]
    InvalidForwardAccounts,
    #[msg("The tokens were not delivered to this program's token admin ATA")]
    TokensNotDeliveredHere,
    #[msg("Duplicate CCV in the policy")]
    DuplicateCcv = 100,
    #[msg("Optional threshold must be between 1 and the number of optional CCVs minus one")]
    InvalidOptionalThreshold,
    #[msg("The default-CCV marker is only valid in the required list")]
    DefaultMarkerNotAllowedInOptional,
    #[msg("Unknown finality flag")]
    InvalidFinalityConfig,
    #[msg("Too many CCVs in one list")]
    TooManyCcvs,
    #[msg("Unsupported CCV config account version")]
    UnsupportedConfigVersion,
    #[msg("Too many sender policies for this source chain")]
    TooManySenderPolicies,
    #[msg("No policy is stored for this sender")]
    SenderPolicyNotFound,
    #[msg("Nothing to set: pass CCV lists, a finality config, or both")]
    EmptyPolicyUpdate,
}
