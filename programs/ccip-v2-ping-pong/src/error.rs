use anchor_lang::prelude::*;

#[error_code]
pub enum PingPongError {
    #[msg("Invalid router address")]
    InvalidRouter,
    #[msg("Address is not owner")]
    OnlyOwner,
    #[msg("Address is not proposed_owner")]
    OnlyProposedOwner,
    #[msg("Proposed owner is invalid")]
    InvalidProposedOwner,
    #[msg("Caller is not an allowed OffRamp")]
    InvalidCaller,
    #[msg("Address is not the program upgrade authority")]
    Unauthorized,
    #[msg("The message sender is not this chain's peer")]
    NotPeer,
    #[msg("The payload is not abi.encode(uint8 kind, uint64 sequence)")]
    InvalidPayload,
    #[msg("Extra args are too long")]
    ExtraArgsTooLong,
    #[msg("Arithmetic overflow")]
    Overflow,
    #[msg("Router accounts are missing")]
    MissingRouterAccounts,
    #[msg("No reply is waiting for this chain")]
    NoPendingReply,
    #[msg("The reply queue for this chain is full")]
    PendingQueueFull,
    #[msg("The router returned no message id")]
    MissingMessageId,
    #[msg("Invalid CCV or finality policy")]
    InvalidCcvPolicy,
    #[msg("Unsupported CCV config account version")]
    UnsupportedConfigVersion,
}
