//! Receiver modes (data, tokens with data, forward on SPL Token and Token-2022), the caller and
//! sender checks, admin withdraw, and every shape of the CCV and finality policy.
mod common;

use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::{
    instruction::{AccountMeta, Instruction},
    system_program,
};
use anchor_lang::{AnchorDeserialize, InstructionData, ToAccountMetas};
use ccip_common::ccvs::{GetReceiverCcvsIn, GetReceiverCcvsOut};
use ccip_common::messages::{FinalityConfig, SVMTokenAmount, SVMTokenReceiverUsage};
use ccip_v2_receiver::instructions::{CcvLists, SenderParams, SetCcvPolicyParams};
use ccip_v2_receiver::message::{Any2SVMMessage, TokenTransfer};
use ccip_v2_receiver::state::{ReceiveMode, ReceiverState, SourceChainCcvConfig};
use common::*;
use solana_keypair::Keypair;
use solana_signer::Signer;

const RX: Pubkey = ccip_v2_receiver::ID;
const FORWARD_BPS: u16 = 250;

fn pda(seeds: &[&[u8]]) -> Pubkey {
    Pubkey::find_program_address(seeds, &RX).0
}
fn config() -> Pubkey {
    pda(&[b"config"])
}
fn state() -> Pubkey {
    pda(&[b"receiver_state"])
}
fn token_admin() -> Pubkey {
    pda(&[b"receiver_token_admin"])
}
fn rent_payer() -> Pubkey {
    pda(&[b"rent_payer"])
}
fn approved(source: u64, sender: &[u8]) -> Pubkey {
    pda(&[b"approved_ccip_sender", &source.to_le_bytes(), sender])
}
fn ccv_config(source: u64) -> Pubkey {
    pda(&[b"ccv_config", &source.to_le_bytes()])
}

fn ix(data: impl InstructionData, accounts: impl ToAccountMetas) -> Instruction {
    Instruction {
        program_id: RX,
        accounts: accounts.to_account_metas(None),
        data: data.data(),
    }
}

/// Initialized receiver with one approved EVM sender and a funded rent payer.
fn setup() -> (Harness, Vec<u8>) {
    let mut h = Harness::new();
    let owner = h.owner.pubkey();
    let mock = h.router;
    h.send_owner(&[ix(
        ccip_v2_receiver::instruction::Initialize {
            router: mock,
            forward_bps: FORWARD_BPS,
        },
        ccip_v2_receiver::accounts::Initialize {
            config: config(),
            receiver_state: state(),
            authority: owner,
            system_program: system_program::ID,
            program: RX,
            program_data: Harness::programdata(&RX),
        },
    )])
    .unwrap();
    let sender = evm_sender(0xaa);
    approve(&mut h, &sender);
    h.allow_offramp(EVM_SOURCE);
    h.fund(rent_payer(), 1_000_000_000);
    (h, sender)
}

fn approve(h: &mut Harness, sender: &[u8]) {
    let owner = h.owner.pubkey();
    h.send_owner(&[ix(
        ccip_v2_receiver::instruction::ApproveSender {
            params: SenderParams {
                chain_selector: EVM_SOURCE,
                remote_sender: sender.to_vec(),
            },
        },
        ccip_v2_receiver::accounts::ApproveSender {
            config: config(),
            approved_sender: approved(EVM_SOURCE, sender),
            authority: owner,
            system_program: system_program::ID,
        },
    )])
    .unwrap();
}

fn receive_accounts(sender: &[u8]) -> Vec<AccountMeta> {
    vec![
        AccountMeta::new_readonly(approved(EVM_SOURCE, sender), false),
        AccountMeta::new_readonly(config(), false),
        AccountMeta::new(state(), false),
    ]
}

fn msg(sender: &[u8], data: Vec<u8>, token: Option<TokenTransfer>) -> Vec<u8> {
    ccip_v2_receiver::instruction::CcipReceiveV2 {
        message: Any2SVMMessage {
            message_id: message_id(7),
            source_chain_selector: EVM_SOURCE,
            sender: sender.to_vec(),
            data,
            token_transfer: token,
        },
    }
    .data()
}

fn transfer(mint: Pubkey, amount: u64) -> TokenTransfer {
    TokenTransfer {
        token_amount: SVMTokenAmount {
            token: mint,
            amount,
        },
        token_receiver: token_admin(),
        token_receiver_usage: SVMTokenReceiverUsage::DeriveAtaAndCreate,
    }
}

fn forward_payload(beneficiary: &Pubkey) -> Vec<u8> {
    let mut d = b"FWD1".to_vec();
    d.extend_from_slice(beneficiary.as_ref());
    d
}

fn forward_accounts(mint: Pubkey, token_program: Pubkey, beneficiary: Pubkey) -> Vec<AccountMeta> {
    vec![
        AccountMeta::new_readonly(token_admin(), false),
        AccountMeta::new(ata(&token_admin(), &mint, &token_program), false),
        AccountMeta::new_readonly(mint, false),
        AccountMeta::new_readonly(token_program, false),
        AccountMeta::new_readonly(beneficiary, false),
        AccountMeta::new(ata(&beneficiary, &mint, &token_program), false),
        AccountMeta::new_readonly(anchor_spl::associated_token::ID, false),
        AccountMeta::new_readonly(system_program::ID, false),
        AccountMeta::new(rent_payer(), false),
    ]
}

#[test]
fn data_message_is_stored() {
    let (mut h, sender) = setup();
    let data = b"hello from sepolia".to_vec();
    h.deliver(
        RX,
        EVM_SOURCE,
        msg(&sender, data.clone(), None),
        receive_accounts(&sender),
    )
    .unwrap();
    let s: ReceiverState = h.account(&state());
    assert_eq!(s.received_count, 1);
    assert_eq!(s.last_mode, ReceiveMode::Data);
    assert_eq!(s.last_data, data);
    assert_eq!(s.last_sender, sender);
    assert_eq!(s.last_message_id, message_id(7));
}

#[test]
fn long_data_is_truncated_in_state() {
    let (mut h, sender) = setup();
    let data = vec![9u8; 600];
    h.deliver(
        RX,
        EVM_SOURCE,
        msg(&sender, data, None),
        receive_accounts(&sender),
    )
    .unwrap();
    let s: ReceiverState = h.account(&state());
    assert_eq!(s.last_data_len, 600);
    assert_eq!(s.last_data.len(), 256);
}

#[test]
fn tokens_with_data_stay_with_the_program() {
    let (mut h, sender) = setup();
    let mint = Pubkey::new_unique();
    h.put_mint(mint, SPL_TOKEN, 9);
    let admin_ata = h.put_ata(token_admin(), mint, SPL_TOKEN, 1_000);
    h.deliver(
        RX,
        EVM_SOURCE,
        msg(&sender, b"invoice 42".to_vec(), Some(transfer(mint, 1_000))),
        receive_accounts(&sender),
    )
    .unwrap();
    let s: ReceiverState = h.account(&state());
    assert_eq!(s.last_mode, ReceiveMode::TokensAndData);
    assert_eq!(s.last_amount, 1_000);
    assert_eq!(h.balance(&admin_ata), 1_000);
}

fn forward_case(token_program: Pubkey) {
    let (mut h, sender) = setup();
    let mint = Pubkey::new_unique();
    h.put_mint(mint, token_program, 6);
    let admin_ata = h.put_ata(token_admin(), mint, token_program, 10_000);
    let beneficiary = Pubkey::new_unique();
    let mut accounts = receive_accounts(&sender);
    accounts.extend(forward_accounts(mint, token_program, beneficiary));
    h.deliver(
        RX,
        EVM_SOURCE,
        msg(
            &sender,
            forward_payload(&beneficiary),
            Some(transfer(mint, 10_000)),
        ),
        accounts,
    )
    .unwrap();
    let kept = 10_000 * u64::from(FORWARD_BPS) / 10_000;
    assert_eq!(h.balance(&admin_ata), kept);
    assert_eq!(
        h.balance(&ata(&beneficiary, &mint, &token_program)),
        10_000 - kept
    );
    let s: ReceiverState = h.account(&state());
    assert_eq!(s.last_mode, ReceiveMode::Forward);
    assert_eq!(s.last_beneficiary, beneficiary);
    assert_eq!(s.last_kept, kept);
    assert_eq!(s.last_forwarded, 10_000 - kept);

    // A second forward to the same beneficiary reuses the ATA (idempotent creation).
    h.put_ata(token_admin(), mint, token_program, 5_000);
    let mut accounts = receive_accounts(&sender);
    accounts.extend(forward_accounts(mint, token_program, beneficiary));
    h.deliver(
        RX,
        EVM_SOURCE,
        msg(
            &sender,
            forward_payload(&beneficiary),
            Some(transfer(mint, 5_000)),
        ),
        accounts,
    )
    .unwrap();
    let kept2 = 5_000 * u64::from(FORWARD_BPS) / 10_000;
    assert_eq!(
        h.balance(&ata(&beneficiary, &mint, &token_program)),
        10_000 - kept + 5_000 - kept2
    );
}

#[test]
fn forward_spl_token() {
    forward_case(SPL_TOKEN);
}

#[test]
fn forward_token_2022() {
    forward_case(TOKEN_2022);
}

#[test]
fn forward_without_tokens_is_data() {
    // No token transfer: the forward tag is just data.
    let (mut h, sender) = setup();
    let payload = forward_payload(&Pubkey::new_unique());
    h.deliver(
        RX,
        EVM_SOURCE,
        msg(&sender, payload, None),
        receive_accounts(&sender),
    )
    .unwrap();
    let s: ReceiverState = h.account(&state());
    assert_eq!(s.last_mode, ReceiveMode::Data);
}

#[test]
fn forward_rejects_bad_payload_and_accounts() {
    let (mut h, sender) = setup();
    let mint = Pubkey::new_unique();
    h.put_mint(mint, SPL_TOKEN, 6);
    h.put_ata(token_admin(), mint, SPL_TOKEN, 100);
    let beneficiary = Pubkey::new_unique();

    let mut short = forward_payload(&beneficiary);
    short.pop();
    let mut accounts = receive_accounts(&sender);
    accounts.extend(forward_accounts(mint, SPL_TOKEN, beneficiary));
    let err = h
        .deliver(
            RX,
            EVM_SOURCE,
            msg(&sender, short, Some(transfer(mint, 100))),
            accounts,
        )
        .unwrap_err();
    assert!(err.contains("InvalidForwardPayload"), "{err}");

    // Beneficiary ATA for a different wallet than the payload names.
    let mut accounts = receive_accounts(&sender);
    accounts.extend(forward_accounts(mint, SPL_TOKEN, Pubkey::new_unique()));
    let err = h
        .deliver(
            RX,
            EVM_SOURCE,
            msg(
                &sender,
                forward_payload(&beneficiary),
                Some(transfer(mint, 100)),
            ),
            accounts,
        )
        .unwrap_err();
    assert!(err.contains("InvalidForwardAccounts"), "{err}");

    // Tokens credited somewhere else than this program's token admin.
    let mut elsewhere = transfer(mint, 100);
    elsewhere.token_receiver = Pubkey::new_unique();
    let mut accounts = receive_accounts(&sender);
    accounts.extend(forward_accounts(mint, SPL_TOKEN, beneficiary));
    let err = h
        .deliver(
            RX,
            EVM_SOURCE,
            msg(&sender, forward_payload(&beneficiary), Some(elsewhere)),
            accounts,
        )
        .unwrap_err();
    assert!(err.contains("TokensNotDeliveredHere"), "{err}");
}

#[test]
fn unapproved_sender_is_rejected() {
    let (mut h, _) = setup();
    let stranger = evm_sender(0xbb);
    let err = h
        .deliver(
            RX,
            EVM_SOURCE,
            msg(&stranger, b"x".to_vec(), None),
            receive_accounts(&stranger),
        )
        .unwrap_err();
    assert!(err.contains("AccountNotInitialized"), "{err}");
}

#[test]
fn caller_must_be_an_allowed_offramp() {
    let (mut h, sender) = setup();
    // Not signed by the OffRamp's PDA: call the receiver directly.
    let mut keys = vec![
        AccountMeta::new_readonly(h.cpi_signer(&RX), false),
        AccountMeta::new_readonly(h.mock, false),
        AccountMeta::new_readonly(h.allowed_offramp(EVM_SOURCE), false),
    ];
    keys.extend(receive_accounts(&sender));
    let direct = Instruction {
        program_id: RX,
        accounts: keys,
        data: msg(&sender, b"x".to_vec(), None),
    };
    assert!(h.send_owner(&[direct]).is_err());

    // Signed correctly, but the router never allowed this OffRamp for the source chain.
    let mut h2 = setup().0;
    let other_source = 1u64;
    let err = h2
        .deliver(
            RX,
            other_source,
            ccip_v2_receiver::instruction::CcipReceiveV2 {
                message: Any2SVMMessage {
                    message_id: message_id(1),
                    source_chain_selector: other_source,
                    sender: sender.clone(),
                    data: vec![],
                    token_transfer: None,
                },
            }
            .data(),
            receive_accounts(&sender),
        )
        .unwrap_err();
    assert!(
        err.contains("InvalidCaller") || err.contains("ConstraintSeeds"),
        "{err}"
    );
}

#[test]
fn unapprove_revokes() {
    let (mut h, sender) = setup();
    let owner = h.owner.pubkey();
    h.send_owner(&[ix(
        ccip_v2_receiver::instruction::UnapproveSender {
            params: SenderParams {
                chain_selector: EVM_SOURCE,
                remote_sender: sender.clone(),
            },
        },
        ccip_v2_receiver::accounts::UnapproveSender {
            config: config(),
            approved_sender: approved(EVM_SOURCE, &sender),
            authority: owner,
        },
    )])
    .unwrap();
    assert!(h
        .deliver(
            RX,
            EVM_SOURCE,
            msg(&sender, b"x".to_vec(), None),
            receive_accounts(&sender)
        )
        .is_err());
}

#[test]
fn owner_withdraws_any_token_and_others_cannot() {
    let (mut h, _) = setup();
    for token_program in [SPL_TOKEN, TOKEN_2022] {
        let mint = Pubkey::new_unique();
        h.put_mint(mint, token_program, 9);
        let from = h.put_ata(token_admin(), mint, token_program, 700);
        let to = h.put_ata(Pubkey::new_unique(), mint, token_program, 0);
        let withdraw = |authority: Pubkey| {
            ix(
                ccip_v2_receiver::instruction::WithdrawTokens { amount: 300 },
                ccip_v2_receiver::accounts::WithdrawTokens {
                    config: config(),
                    program_token_account: from,
                    to_token_account: to,
                    mint,
                    token_program,
                    token_admin: token_admin(),
                    authority,
                },
            )
        };
        let stranger = Keypair::new();
        h.fund(stranger.pubkey(), 1_000_000_000);
        let err = h
            .send(&[withdraw(stranger.pubkey())], &[&stranger])
            .unwrap_err();
        assert!(err.contains("OnlyOwner"), "{err}");
        let owner = h.owner.pubkey();
        h.send_owner(&[withdraw(owner)]).unwrap();
        assert_eq!(h.balance(&from), 400);
        assert_eq!(h.balance(&to), 300);
    }
}

#[test]
fn forward_bps_is_owner_only_and_bounded() {
    let (mut h, _) = setup();
    let owner = h.owner.pubkey();
    let set = |bps: u16, authority: Pubkey| {
        ix(
            ccip_v2_receiver::instruction::SetForwardBps { forward_bps: bps },
            ccip_v2_receiver::accounts::UpdateConfig {
                config: config(),
                authority,
            },
        )
    };
    assert!(h
        .send_owner(&[set(10_001, owner)])
        .unwrap_err()
        .contains("InvalidBps"));
    h.send_owner(&[set(500, owner)]).unwrap();
    let stranger = Keypair::new();
    h.fund(stranger.pubkey(), 1_000_000_000);
    assert!(h
        .send(&[set(1, stranger.pubkey())], &[&stranger])
        .unwrap_err()
        .contains("OnlyOwner"));
}

// ---- CCV and finality policy ----

fn set_policy(
    h: &mut Harness,
    sender: Option<Vec<u8>>,
    ccvs: Option<CcvLists>,
    finality: Option<FinalityConfig>,
) -> Result<(), String> {
    let owner = h.owner.pubkey();
    h.send_owner(&[ix(
        ccip_v2_receiver::instruction::SetCcvPolicy {
            params: SetCcvPolicyParams {
                source_chain_selector: EVM_SOURCE,
                sender,
                ccvs,
                allowed_finality_config: finality,
            },
        },
        ccip_v2_receiver::accounts::SetCcvPolicy {
            config: config(),
            source_chain_ccv_config: ccv_config(EVM_SOURCE),
            authority: owner,
            system_program: system_program::ID,
        },
    )])
    .map(|_| ())
}

fn get_ccvs(h: &mut Harness, sender: &[u8]) -> GetReceiverCcvsOut {
    let r = h
        .send_owner(&[ix(
            ccip_v2_receiver::instruction::GetCcvsAndFinalityConfig {
                input: GetReceiverCcvsIn {
                    source_chain_selector: EVM_SOURCE,
                    sender: sender.to_vec(),
                },
            },
            ccip_v2_receiver::accounts::GetCcvsAndFinalityConfig {
                source_chain_ccv_config: ccv_config(EVM_SOURCE),
            },
        )])
        .unwrap();
    GetReceiverCcvsOut::try_from_slice(&r.return_data).unwrap()
}

fn lists(required: Vec<Pubkey>, optional: Vec<Pubkey>, threshold: u8) -> CcvLists {
    CcvLists {
        required_ccvs: required,
        optional_ccvs: optional,
        optional_threshold: threshold,
    }
}

const DEPTH_1: FinalityConfig = FinalityConfig {
    flags: 0,
    block_depth: 1,
};
const SAFE: FinalityConfig = FinalityConfig {
    flags: 1,
    block_depth: 0,
};

#[test]
fn no_config_means_lane_defaults_and_finalized_only() {
    let (mut h, sender) = setup();
    let out = get_ccvs(&mut h, &sender);
    assert!(out.required_ccvs.is_empty() && out.optional_ccvs.is_empty());
    assert_eq!(out.optional_threshold, 0);
    assert_eq!(out.allowed_finality_config, FinalityConfig::default());
}

#[test]
fn finality_only() {
    let (mut h, sender) = setup();
    set_policy(&mut h, None, None, Some(DEPTH_1)).unwrap();
    let out = get_ccvs(&mut h, &sender);
    assert!(out.required_ccvs.is_empty(), "CCVs stay at lane defaults");
    assert_eq!(out.allowed_finality_config, DEPTH_1);
}

#[test]
fn ccvs_only() {
    let (mut h, sender) = setup();
    let ccv = Pubkey::new_unique();
    set_policy(&mut h, None, Some(lists(vec![ccv], vec![], 0)), None).unwrap();
    let out = get_ccvs(&mut h, &sender);
    assert_eq!(out.required_ccvs, vec![ccv]);
    assert_eq!(
        out.allowed_finality_config,
        FinalityConfig::default(),
        "finalized only"
    );
}

#[test]
fn ccvs_and_finality_then_independent_updates() {
    let (mut h, sender) = setup();
    let (a, b, c) = (
        Pubkey::new_unique(),
        Pubkey::new_unique(),
        Pubkey::new_unique(),
    );
    set_policy(
        &mut h,
        None,
        Some(lists(vec![Pubkey::default(), a], vec![b, c], 1)),
        Some(SAFE),
    )
    .unwrap();
    let out = get_ccvs(&mut h, &sender);
    assert_eq!(out.required_ccvs, vec![Pubkey::default(), a]);
    assert_eq!(out.optional_ccvs, vec![b, c]);
    assert_eq!(out.optional_threshold, 1);
    assert_eq!(out.allowed_finality_config, SAFE);

    // Finality update keeps the CCVs.
    set_policy(&mut h, None, None, Some(DEPTH_1)).unwrap();
    let out = get_ccvs(&mut h, &sender);
    assert_eq!(out.required_ccvs, vec![Pubkey::default(), a]);
    assert_eq!(out.allowed_finality_config, DEPTH_1);

    // CCV update keeps the finality.
    set_policy(&mut h, None, Some(lists(vec![c], vec![], 0)), None).unwrap();
    let out = get_ccvs(&mut h, &sender);
    assert_eq!(out.required_ccvs, vec![c]);
    assert!(out.optional_ccvs.is_empty());
    assert_eq!(out.allowed_finality_config, DEPTH_1);
}

#[test]
fn sender_policy_overrides_chain_policy() {
    let (mut h, sender) = setup();
    let other = evm_sender(0xcc);
    let (a, b) = (Pubkey::new_unique(), Pubkey::new_unique());
    set_policy(&mut h, None, Some(lists(vec![a], vec![], 0)), None).unwrap();
    set_policy(
        &mut h,
        Some(sender.clone()),
        Some(lists(vec![b], vec![], 0)),
        Some(DEPTH_1),
    )
    .unwrap();

    let mine = get_ccvs(&mut h, &sender);
    assert_eq!(mine.required_ccvs, vec![b]);
    assert_eq!(mine.allowed_finality_config, DEPTH_1);
    let theirs = get_ccvs(&mut h, &other);
    assert_eq!(theirs.required_ccvs, vec![a]);
    assert_eq!(theirs.allowed_finality_config, FinalityConfig::default());

    // Removing the sender policy falls back to the chain policy.
    let owner = h.owner.pubkey();
    h.send_owner(&[ix(
        ccip_v2_receiver::instruction::RemoveSenderCcvPolicy {
            source_chain_selector: EVM_SOURCE,
            sender: sender.clone(),
        },
        ccip_v2_receiver::accounts::UpdateCcvConfig {
            config: config(),
            source_chain_ccv_config: ccv_config(EVM_SOURCE),
            authority: owner,
        },
    )])
    .unwrap();
    assert_eq!(get_ccvs(&mut h, &sender).required_ccvs, vec![a]);
    let c: SourceChainCcvConfig = h.account(&ccv_config(EVM_SOURCE));
    assert!(c.sender_policies.is_empty());
}

#[test]
fn remove_config_restores_defaults() {
    let (mut h, sender) = setup();
    set_policy(
        &mut h,
        None,
        Some(lists(vec![Pubkey::new_unique()], vec![], 0)),
        Some(SAFE),
    )
    .unwrap();
    let owner = h.owner.pubkey();
    h.send_owner(&[ix(
        ccip_v2_receiver::instruction::RemoveCcvConfig {
            source_chain_selector: EVM_SOURCE,
        },
        ccip_v2_receiver::accounts::RemoveCcvConfig {
            config: config(),
            source_chain_ccv_config: ccv_config(EVM_SOURCE),
            authority: owner,
        },
    )])
    .unwrap();
    assert!(!h.exists(&ccv_config(EVM_SOURCE)));
    let out = get_ccvs(&mut h, &sender);
    assert!(out.required_ccvs.is_empty());
    assert_eq!(out.allowed_finality_config, FinalityConfig::default());
}

#[test]
fn invalid_policies_are_refused() {
    let (mut h, _) = setup();
    let a = Pubkey::new_unique();
    let cases: Vec<(Option<CcvLists>, Option<FinalityConfig>, &str)> = vec![
        (None, None, "EmptyPolicyUpdate"),
        (Some(lists(vec![a, a], vec![], 0)), None, "DuplicateCcv"),
        (
            Some(lists(vec![], vec![Pubkey::default(), a], 1)),
            None,
            "DefaultMarkerNotAllowedInOptional",
        ),
        (
            Some(lists(vec![], vec![a], 1)),
            None,
            "InvalidOptionalThreshold",
        ),
        (
            None,
            Some(FinalityConfig {
                flags: 4,
                block_depth: 0,
            }),
            "InvalidFinalityConfig",
        ),
    ];
    for (ccvs, finality, want) in cases {
        let err = set_policy(&mut h, None, ccvs, finality).unwrap_err();
        assert!(err.contains(want), "want {want}: {err}");
    }
    let stranger = Keypair::new();
    h.fund(stranger.pubkey(), 1_000_000_000);
    let err = h
        .send(
            &[ix(
                ccip_v2_receiver::instruction::SetCcvPolicy {
                    params: SetCcvPolicyParams {
                        source_chain_selector: EVM_SOURCE,
                        sender: None,
                        ccvs: None,
                        allowed_finality_config: Some(DEPTH_1),
                    },
                },
                ccip_v2_receiver::accounts::SetCcvPolicy {
                    config: config(),
                    source_chain_ccv_config: ccv_config(EVM_SOURCE),
                    authority: stranger.pubkey(),
                    system_program: system_program::ID,
                },
            )],
            &[&stranger],
        )
        .unwrap_err();
    assert!(err.contains("OnlyOwner"), "{err}");
}

#[test]
fn initialize_is_upgrade_authority_only() {
    let mut h = Harness::new();
    let stranger = Keypair::new();
    h.fund(stranger.pubkey(), 1_000_000_000);
    let mock = h.router;
    let err = h
        .send(
            &[ix(
                ccip_v2_receiver::instruction::Initialize {
                    router: mock,
                    forward_bps: 0,
                },
                ccip_v2_receiver::accounts::Initialize {
                    config: config(),
                    receiver_state: state(),
                    authority: stranger.pubkey(),
                    system_program: system_program::ID,
                    program: RX,
                    program_data: Harness::programdata(&RX),
                },
            )],
            &[&stranger],
        )
        .unwrap_err();
    assert!(err.contains("Unauthorized"), "{err}");
}
