//! Ping-pong: peer checks on receive, both reply strategies, owner-only ping, CCV policy.
mod common;

use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::{
    instruction::{AccountMeta, Instruction},
    system_program,
};
use anchor_lang::{AnchorDeserialize, InstructionData, ToAccountMetas};
use ccip_common::ccvs::{GetReceiverCcvsIn, GetReceiverCcvsOut};
use ccip_common::messages::FinalityConfig;
use ccip_v2_ping_pong::message::Any2SVMMessage;
use ccip_v2_ping_pong::state::{
    encode_payload, Peer, PingPongState, ReplyMode, KIND_PING, KIND_PONG, MAX_PENDING_REPLIES,
};
use ccip_v2_ping_pong::{CcvLists, SetCcvPolicyParams, SetPeerParams};
use common::*;
use solana_keypair::Keypair;
use solana_signer::Signer;
use test_mock_router::{LastSend, MOCK_FEE_LAMPORTS};

const PP: Pubkey = ccip_v2_ping_pong::ID;
const EXTRA_ARGS: [u8; 6] = [0xa6, 0x9d, 0xd4, 0xaa, 1, 2];

fn pda(seeds: &[&[u8]]) -> Pubkey {
    Pubkey::find_program_address(seeds, &PP).0
}
fn config() -> Pubkey {
    pda(&[b"config"])
}
fn state() -> Pubkey {
    pda(&[b"ping_pong_state"])
}
fn signer() -> Pubkey {
    pda(&[b"ccip_signer"])
}
fn peer() -> Pubkey {
    pda(&[b"peer", &EVM_SOURCE.to_le_bytes()])
}
fn pending(seq: u64) -> Pubkey {
    pda(&[
        b"pending_reply",
        &EVM_SOURCE.to_le_bytes(),
        &seq.to_le_bytes(),
    ])
}
fn ccv_config() -> Pubkey {
    pda(&[b"ccv_config", &EVM_SOURCE.to_le_bytes()])
}
fn last_send(h: &Harness) -> Pubkey {
    Pubkey::find_program_address(&[b"last_send"], &h.router).0
}
fn fee_vault() -> Pubkey {
    Pubkey::find_program_address(&[b"fee_vault"], &test_mock_router::ID).0
}

fn ix(data: impl InstructionData, accounts: impl ToAccountMetas) -> Instruction {
    Instruction {
        program_id: PP,
        accounts: accounts.to_account_metas(None),
        data: data.data(),
    }
}

fn peer_bytes() -> [u8; 32] {
    evm_sender(0xee).try_into().unwrap()
}

fn setup(mode: ReplyMode) -> Harness {
    let mut h = Harness::new();
    let owner = h.owner.pubkey();
    let mock = h.router;
    h.send_owner(&[
        ix(
            ccip_v2_ping_pong::instruction::Initialize {
                router: mock,
                reply_mode: mode,
            },
            ccip_v2_ping_pong::accounts::Initialize {
                config: config(),
                state: state(),
                ccip_signer: signer(),
                authority: owner,
                system_program: system_program::ID,
                program: PP,
                program_data: Harness::programdata(&PP),
            },
        ),
        ix(
            ccip_v2_ping_pong::instruction::SetPeer {
                params: SetPeerParams {
                    chain_selector: EVM_SOURCE,
                    address: peer_bytes(),
                    accepted_sender: peer_bytes(),
                    extra_args: EXTRA_ARGS.to_vec(),
                    reply_resolution_metadata: vec![7, 7],
                },
            },
            ccip_v2_ping_pong::accounts::SetPeer {
                config: config(),
                peer: peer(),
                authority: owner,
                system_program: system_program::ID,
            },
        ),
        Instruction {
            program_id: mock,
            accounts: test_mock_router::accounts::InitLastSend {
                last_send: Pubkey::find_program_address(&[b"last_send"], &mock).0,
                payer: owner,
                system_program: system_program::ID,
            }
            .to_account_metas(None),
            data: test_mock_router::instruction::InitLastSend {}.data(),
        },
    ])
    .unwrap();
    h.allow_offramp(EVM_SOURCE);
    h.fund(signer(), 1_000_000_000);
    h.fund(fee_vault(), 1_000_000);
    h
}

/// The mock router's `ccip_send_v2` accounts, as resolution would list them.
fn router_accounts(h: &Harness) -> Vec<AccountMeta> {
    vec![
        AccountMeta::new(signer(), false),
        AccountMeta::new(fee_vault(), false),
        AccountMeta::new(last_send(h), false),
        AccountMeta::new_readonly(system_program::ID, false),
    ]
}

fn receive_accounts() -> Vec<AccountMeta> {
    vec![
        AccountMeta::new_readonly(config(), false),
        AccountMeta::new(peer(), false),
        AccountMeta::new(state(), false),
        AccountMeta::new(signer(), false),
    ]
}

fn msg(sender: [u8; 32], data: Vec<u8>) -> Vec<u8> {
    ccip_v2_ping_pong::instruction::CcipReceiveV2 {
        message: Any2SVMMessage {
            message_id: message_id(3),
            source_chain_selector: EVM_SOURCE,
            sender: sender.to_vec(),
            data,
            token_transfer: None,
        },
    }
    .data()
}

#[test]
fn owner_pings_the_peer() {
    let mut h = setup(ReplyMode::Deferred);
    let owner = h.owner.pubkey();
    let mut ping = ix(
        ccip_v2_ping_pong::instruction::Ping {
            dest_chain_selector: EVM_SOURCE,
            resolution_metadata: vec![9],
        },
        ccip_v2_ping_pong::accounts::Ping {
            config: config(),
            state: state(),
            peer: peer(),
            ccip_signer: signer(),
            router: h.router,
            authority: owner,
        },
    );
    ping.accounts.extend(router_accounts(&h));
    let before = h.svm.get_account(&signer()).unwrap().lamports;
    h.send_owner(&[ping.clone()]).unwrap();
    let sent: LastSend = h.account(&last_send(&h));
    assert_eq!(sent.sender, signer(), "the PDA signs the send");
    assert_eq!(sent.params.dest_chain_selector, EVM_SOURCE);
    assert_eq!(sent.params.message.receiver, peer_bytes().to_vec());
    assert_eq!(sent.params.message.data, encode_payload(KIND_PING, 1));
    assert_eq!(sent.params.message.extra_args, EXTRA_ARGS.to_vec());
    assert_eq!(sent.params.message.fee_token, Pubkey::default());
    assert_eq!(sent.params.resolution_metadata, vec![9]);
    assert_eq!(
        before - h.svm.get_account(&signer()).unwrap().lamports,
        MOCK_FEE_LAMPORTS
    );
    let s: PingPongState = h.account(&state());
    assert_eq!(s.pings_sent, 1);

    let stranger = Keypair::new();
    h.fund(stranger.pubkey(), 1_000_000_000);
    ping.accounts[5] = AccountMeta::new_readonly(stranger.pubkey(), true);
    let err = h.send(&[ping], &[&stranger]).unwrap_err();
    assert!(err.contains("OnlyOwner"), "{err}");
}

#[test]
fn only_the_peer_is_accepted() {
    let mut h = setup(ReplyMode::Deferred);
    let accounts = receive_accounts();
    let err = h
        .deliver(
            PP,
            EVM_SOURCE,
            msg(
                evm_sender(0x11).try_into().unwrap(),
                encode_payload(KIND_PING, 1),
            ),
            accounts,
        )
        .unwrap_err();
    assert!(err.contains("NotPeer"), "{err}");
}

#[test]
fn pong_is_recorded() {
    let mut h = setup(ReplyMode::Deferred);
    h.deliver(
        PP,
        EVM_SOURCE,
        msg(peer_bytes(), encode_payload(KIND_PONG, 5)),
        receive_accounts(),
    )
    .unwrap();
    let s: PingPongState = h.account(&state());
    assert_eq!(s.pongs_received, 1);
    assert_eq!(s.last_sequence, 5);
    assert_eq!(s.last_kind, KIND_PONG);
}

#[test]
fn malformed_payload_is_refused() {
    let mut h = setup(ReplyMode::Deferred);
    let err = h
        .deliver(
            PP,
            EVM_SOURCE,
            msg(peer_bytes(), b"ping".to_vec()),
            receive_accounts(),
        )
        .unwrap_err();
    assert!(err.contains("InvalidPayload"), "{err}");
}

#[test]
fn deferred_replies_are_queued_then_sent_by_anyone() {
    let mut h = setup(ReplyMode::Deferred);
    for seq in [4, 5] {
        h.deliver(
            PP,
            EVM_SOURCE,
            msg(peer_bytes(), encode_payload(KIND_PING, seq)),
            receive_accounts(),
        )
        .unwrap();
    }
    let p: Peer = h.account(&peer());
    assert_eq!(p.pending_replies, vec![4, 5]);
    let s: PingPongState = h.account(&state());
    assert_eq!((s.pings_received, s.pongs_sent), (2, 0));

    let anyone = Keypair::new();
    h.fund(anyone.pubkey(), 1_000_000_000);
    let mut send = ix(
        ccip_v2_ping_pong::instruction::SendPendingReply {
            chain_selector: EVM_SOURCE,
            resolution_metadata: vec![],
        },
        ccip_v2_ping_pong::accounts::SendPendingReply {
            config: config(),
            state: state(),
            peer: peer(),
            ccip_signer: signer(),
            router: h.router,
        },
    );
    send.accounts.extend(router_accounts(&h));
    for expected in [4, 5] {
        h.send(&[send.clone()], &[&anyone]).unwrap();
        let sent: LastSend = h.account(&last_send(&h));
        assert_eq!(
            sent.params.message.data,
            encode_payload(KIND_PONG, expected),
            "oldest first"
        );
    }
    let p: Peer = h.account(&peer());
    assert!(p.pending_replies.is_empty());
    let s: PingPongState = h.account(&state());
    assert_eq!(s.pongs_sent, 2);
    let err = h.send(&[send], &[&anyone]).unwrap_err();
    assert!(err.contains("NoPendingReply"), "{err}");
}

#[test]
fn deferred_queue_is_bounded() {
    let mut h = setup(ReplyMode::Deferred);
    for seq in 0..MAX_PENDING_REPLIES as u64 {
        h.deliver(
            PP,
            EVM_SOURCE,
            msg(peer_bytes(), encode_payload(KIND_PING, seq)),
            receive_accounts(),
        )
        .unwrap();
    }
    let err = h
        .deliver(
            PP,
            EVM_SOURCE,
            msg(peer_bytes(), encode_payload(KIND_PING, 99)),
            receive_accounts(),
        )
        .unwrap_err();
    assert!(err.contains("PendingQueueFull"), "{err}");
}

#[test]
fn immediate_reply_is_sent_inside_receive() {
    let mut h = setup(ReplyMode::Immediate);
    let mut accounts = receive_accounts();
    accounts.push(AccountMeta::new_readonly(h.router, false));
    accounts.extend(router_accounts(&h));
    let mut data = encode_payload(KIND_PING, 8);
    data.extend_from_slice(&[1, 2, 3]); // resolution metadata for the reply
    let r = h
        .deliver(PP, EVM_SOURCE, msg(peer_bytes(), data), accounts)
        .unwrap();
    let sent: LastSend = h.account(&last_send(&h));
    assert_eq!(sent.sender, signer());
    assert_eq!(sent.params.message.data, encode_payload(KIND_PONG, 8));
    assert_eq!(sent.params.resolution_metadata, vec![1, 2, 3]);
    let s: PingPongState = h.account(&state());
    assert_eq!((s.pings_received, s.pongs_sent), (1, 1));
    assert!(r.compute_units > 0);
}

#[test]
fn immediate_reply_uses_stored_metadata_when_the_ping_has_none() {
    let mut h = setup(ReplyMode::Immediate);
    let mut accounts = receive_accounts();
    accounts.push(AccountMeta::new_readonly(h.router, false));
    accounts.extend(router_accounts(&h));
    h.deliver(
        PP,
        EVM_SOURCE,
        msg(peer_bytes(), encode_payload(KIND_PING, 9)),
        accounts,
    )
    .unwrap();
    let sent: LastSend = h.account(&last_send(&h));
    assert_eq!(sent.params.resolution_metadata, vec![7, 7]);
}

#[test]
fn immediate_reply_without_router_accounts_fails() {
    let mut h = setup(ReplyMode::Immediate);
    let err = h
        .deliver(
            PP,
            EVM_SOURCE,
            msg(peer_bytes(), encode_payload(KIND_PING, 1)),
            receive_accounts(),
        )
        .unwrap_err();
    assert!(err.contains("MissingRouterAccounts"), "{err}");
}

#[test]
fn ccv_policy_round_trip() {
    let mut h = setup(ReplyMode::Deferred);
    let owner = h.owner.pubkey();
    let ccv = Pubkey::new_unique();
    let set = |ccvs: Option<CcvLists>, finality: Option<FinalityConfig>| {
        ix(
            ccip_v2_ping_pong::instruction::SetCcvPolicy {
                params: SetCcvPolicyParams {
                    source_chain_selector: EVM_SOURCE,
                    ccvs,
                    allowed_finality_config: finality,
                },
            },
            ccip_v2_ping_pong::accounts::SetCcvPolicy {
                config: config(),
                source_chain_ccv_config: ccv_config(),
                authority: owner,
                system_program: system_program::ID,
            },
        )
    };
    let read = |h: &mut Harness| {
        let r = h
            .send_owner(&[ix(
                ccip_v2_ping_pong::instruction::GetCcvsAndFinalityConfig {
                    _input: GetReceiverCcvsIn {
                        source_chain_selector: EVM_SOURCE,
                        sender: peer_bytes().to_vec(),
                    },
                },
                ccip_v2_ping_pong::accounts::GetCcvsAndFinalityConfig {
                    source_chain_ccv_config: ccv_config(),
                },
            )])
            .unwrap();
        GetReceiverCcvsOut::try_from_slice(&r.return_data).unwrap()
    };
    let depth = FinalityConfig {
        flags: 0,
        block_depth: 1,
    };
    h.send_owner(&[set(None, Some(depth))]).unwrap();
    let out = read(&mut h);
    assert!(out.required_ccvs.is_empty());
    assert_eq!(out.allowed_finality_config, depth);
    h.send_owner(&[set(
        Some(CcvLists {
            required_ccvs: vec![ccv],
            optional_ccvs: vec![],
            optional_threshold: 0,
        }),
        None,
    )])
    .unwrap();
    let out = read(&mut h);
    assert_eq!(out.required_ccvs, vec![ccv]);
    assert_eq!(out.allowed_finality_config, depth);
}
