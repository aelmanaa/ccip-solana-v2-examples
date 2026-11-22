//! LiteSVM harness shared by the program tests.
//!
//! The programs are loaded from `target/deploy` (run `anchor build` first). `test_mock_offramp`
//! stands in for the OffRamp (it signs as each receiver's `external_execution_config` PDA) and
//! `test_mock_router` for the router (it owns the `allowed_offramp` PDAs and answers
//! `ccip_send_v2`).
#![allow(dead_code)]

use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::{
    instruction::{AccountMeta, Instruction},
    program_option::COption,
    program_pack::Pack,
};
use anchor_lang::{AccountDeserialize, InstructionData, ToAccountMetas};
use litesvm::LiteSVM;
use solana_account::Account;
use solana_keypair::Keypair;
use solana_message::{Message, VersionedMessage};
use solana_signer::Signer;
use solana_transaction::versioned::VersionedTransaction;

pub const EVM_SOURCE: u64 = 16_015_286_601_757_825_753;
pub const BPF_UPGRADEABLE: Pubkey = anchor_lang::solana_program::bpf_loader_upgradeable::ID;
pub const SPL_TOKEN: Pubkey = anchor_spl::token::ID;
pub const TOKEN_2022: Pubkey = anchor_spl::token_2022::ID;

pub fn so(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/../../target/deploy/{name}.so",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e} (run `anchor build` first)"))
}

pub struct Harness {
    pub svm: LiteSVM,
    pub owner: Keypair,
    /// The mock OffRamp.
    pub mock: Pubkey,
    /// The mock router.
    pub router: Pubkey,
}

#[derive(Debug)]
pub struct TxResult {
    pub logs: Vec<String>,
    pub return_data: Vec<u8>,
    pub compute_units: u64,
}

impl Harness {
    pub fn new() -> Self {
        let mut svm = LiteSVM::new();
        let owner = Keypair::new();
        svm.airdrop(&owner.pubkey(), 100_000_000_000).unwrap();
        let mock = test_mock_offramp::ID;
        let router = test_mock_router::ID;
        svm.add_program(mock, &so("test_mock_offramp")).unwrap();
        svm.add_program(router, &so("test_mock_router")).unwrap();
        let mut h = Self {
            svm,
            owner,
            mock,
            router,
        };
        h.add_upgradeable(ccip_v2_receiver::ID, "ccip_v2_receiver");
        h.add_upgradeable(ccip_v2_ping_pong::ID, "ccip_v2_ping_pong");
        h
    }

    /// Loads a program and makes `owner` its upgrade authority, which `initialize` checks.
    fn add_upgradeable(&mut self, program_id: Pubkey, name: &str) {
        self.svm.add_program(program_id, &so(name)).unwrap();
        let programdata = Pubkey::find_program_address(&[program_id.as_ref()], &BPF_UPGRADEABLE).0;
        let mut account = self.svm.get_account(&programdata).unwrap_or_else(|| {
            let bytes = so(name);
            let mut data = vec![0u8; 45 + bytes.len()];
            data[0..4].copy_from_slice(&3u32.to_le_bytes());
            data[45..].copy_from_slice(&bytes);
            let program = Account {
                lamports: 1_000_000_000,
                data: {
                    let mut d = vec![0u8; 36];
                    d[0..4].copy_from_slice(&2u32.to_le_bytes());
                    d[4..36].copy_from_slice(programdata.as_ref());
                    d
                },
                owner: BPF_UPGRADEABLE,
                executable: true,
                rent_epoch: 0,
            };
            self.svm.set_account(program_id, program).unwrap();
            Account {
                lamports: 1_000_000_000,
                data,
                owner: BPF_UPGRADEABLE,
                executable: false,
                rent_epoch: 0,
            }
        });
        // ProgramData { slot: u64, upgrade_authority_address: Option<Pubkey> } after a u32 tag.
        account.data[12] = 1;
        account.data[13..45].copy_from_slice(self.owner.pubkey().as_ref());
        self.svm.set_account(programdata, account).unwrap();
    }

    pub fn programdata(program_id: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(&[program_id.as_ref()], &BPF_UPGRADEABLE).0
    }

    pub fn send(&mut self, ixs: &[Instruction], signers: &[&Keypair]) -> Result<TxResult, String> {
        self.svm.expire_blockhash();
        let payer = signers
            .first()
            .map(|k| k.pubkey())
            .unwrap_or(self.owner.pubkey());
        let msg = Message::new_with_blockhash(ixs, Some(&payer), &self.svm.latest_blockhash());
        let tx = VersionedTransaction::try_new(VersionedMessage::Legacy(msg), signers)
            .map_err(|e| e.to_string())?;
        match self.svm.send_transaction(tx) {
            Ok(meta) => Ok(TxResult {
                logs: meta.logs,
                return_data: meta.return_data.data,
                compute_units: meta.compute_units_consumed,
            }),
            Err(failed) => Err(format!("{:?} {}", failed.err, failed.meta.logs.join("\n"))),
        }
    }

    pub fn send_owner(&mut self, ixs: &[Instruction]) -> Result<TxResult, String> {
        let owner = self.owner.insecure_clone();
        self.send(ixs, &[&owner])
    }

    pub fn account<T: AccountDeserialize>(&self, key: &Pubkey) -> T {
        let account = self.svm.get_account(key).expect("account exists");
        T::try_deserialize(&mut account.data.as_slice()).expect("deserializes")
    }

    pub fn exists(&self, key: &Pubkey) -> bool {
        self.svm.get_account(key).is_some_and(|a| a.lamports > 0)
    }

    /// Creates the router-owned `allowed_offramp` PDA for the mock OffRamp and `source`.
    pub fn allow_offramp(&mut self, source: u64) -> Pubkey {
        let key = self.allowed_offramp(source);
        self.svm
            .set_account(
                key,
                Account {
                    lamports: 1_000_000,
                    data: vec![],
                    owner: self.router,
                    executable: false,
                    rent_epoch: 0,
                },
            )
            .unwrap();
        key
    }

    pub fn allowed_offramp(&self, source: u64) -> Pubkey {
        Pubkey::find_program_address(
            &[
                b"allowed_offramp",
                &source.to_le_bytes(),
                self.mock.as_ref(),
            ],
            &self.router,
        )
        .0
    }

    pub fn cpi_signer(&self, receiver: &Pubkey) -> Pubkey {
        Pubkey::find_program_address(
            &[b"external_execution_config", receiver.as_ref()],
            &self.mock,
        )
        .0
    }

    /// Delivers `receiver_ix_data` to `receiver` through the mock OffRamp. `accounts` are the
    /// receiver's own accounts after the OffRamp's three.
    pub fn deliver(
        &mut self,
        receiver: Pubkey,
        source: u64,
        receiver_ix_data: Vec<u8>,
        accounts: Vec<AccountMeta>,
    ) -> Result<TxResult, String> {
        let mut keys = test_mock_offramp::accounts::Deliver {
            cpi_signer: self.cpi_signer(&receiver),
        }
        .to_account_metas(None);
        keys.push(AccountMeta::new_readonly(receiver, false));
        keys.push(AccountMeta::new_readonly(self.mock, false));
        keys.push(AccountMeta::new_readonly(
            self.allowed_offramp(source),
            false,
        ));
        keys.extend(accounts);
        let ix = Instruction {
            program_id: self.mock,
            accounts: keys,
            data: test_mock_offramp::instruction::Deliver { receiver_ix_data }.data(),
        };
        self.send_owner(&[ix])
    }

    pub fn put_mint(&mut self, mint: Pubkey, token_program: Pubkey, decimals: u8) {
        let state = spl_token::state::Mint {
            mint_authority: COption::None,
            supply: 1_000_000_000_000,
            decimals,
            is_initialized: true,
            freeze_authority: COption::None,
        };
        let mut data = vec![0u8; spl_token::state::Mint::LEN];
        spl_token::state::Mint::pack(state, &mut data).unwrap();
        self.svm
            .set_account(
                mint,
                Account {
                    lamports: 1_000_000_000,
                    data,
                    owner: token_program,
                    executable: false,
                    rent_epoch: 0,
                },
            )
            .unwrap();
    }

    /// Creates (or overwrites) `owner`'s ATA for `mint` holding `amount`.
    pub fn put_ata(
        &mut self,
        owner: Pubkey,
        mint: Pubkey,
        token_program: Pubkey,
        amount: u64,
    ) -> Pubkey {
        let key = ata(&owner, &mint, &token_program);
        self.put_token_account(key, owner, mint, token_program, amount);
        key
    }

    pub fn put_token_account(
        &mut self,
        key: Pubkey,
        owner: Pubkey,
        mint: Pubkey,
        token_program: Pubkey,
        amount: u64,
    ) {
        let state = spl_token::state::Account {
            mint,
            owner,
            amount,
            delegate: COption::None,
            state: spl_token::state::AccountState::Initialized,
            is_native: COption::None,
            delegated_amount: 0,
            close_authority: COption::None,
        };
        let mut data = vec![0u8; spl_token::state::Account::LEN];
        spl_token::state::Account::pack(state, &mut data).unwrap();
        self.svm
            .set_account(
                key,
                Account {
                    lamports: 1_000_000_000,
                    data,
                    owner: token_program,
                    executable: false,
                    rent_epoch: 0,
                },
            )
            .unwrap();
    }

    /// Token balance read from the base layout (valid for SPL Token and Token-2022 accounts).
    pub fn balance(&self, key: &Pubkey) -> u64 {
        let data = self.svm.get_account(key).expect("token account").data;
        u64::from_le_bytes(data[64..72].try_into().unwrap())
    }

    pub fn fund(&mut self, key: Pubkey, lamports: u64) {
        self.svm.airdrop(&key, lamports).unwrap();
    }
}

pub fn ata(owner: &Pubkey, mint: &Pubkey, token_program: &Pubkey) -> Pubkey {
    anchor_spl::associated_token::get_associated_token_address_with_program_id(
        owner,
        mint,
        token_program,
    )
}

/// The EVM sender bytes as the OffRamp delivers them: the address left-padded to 32 bytes.
pub fn evm_sender(byte: u8) -> Vec<u8> {
    let mut s = vec![0u8; 12];
    s.extend_from_slice(&[byte; 20]);
    s
}

pub fn message_id(n: u8) -> [u8; 32] {
    [n; 32]
}
