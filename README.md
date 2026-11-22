# CCIP 2.0 Solana examples

Three Anchor programs that use [Chainlink CCIP](https://docs.chain.link/ccip) 2.0 on Solana, an EVM
contract they talk to, TypeScript scripts built on [`@chainlink/ccip-sdk`](https://www.npmjs.com/package/@chainlink/ccip-sdk),
and LiteSVM tests.

| Program             | What it does                                                                                                             |
| ------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| `ccip_v2_sender`    | Sends CCIP messages (data, one token, fee in SOL or an SPL token) by CPI into the router's `ccip_send_v2`.               |
| `ccip_v2_receiver`  | Receives messages: stores data, keeps tokens, or forwards tokens to a beneficiary; declares its CCV and finality policy. |
| `ccip_v2_ping_pong` | Sends and receives: pings a peer on another chain and answers the peer's pings.                                          |
| `evm/PingPongPeer`  | The ping-pong peer on an EVM chain (Foundry, `@chainlink/contracts-ccip` 2.0.0).                                         |

The sender is derived from `example-ccip-sender-v2` and the receiver from `example-ccip-receiver-v2` in
[smartcontractkit/chainlink-ccip-solana](https://github.com/smartcontractkit/chainlink-ccip-solana) (MIT). The
programs depend on that repository's `ccip_common` and `ccip_router` crates at commit `3f5d182`.

## Devnet deployment

| Program             | Program id                                     |
| ------------------- | ---------------------------------------------- |
| `ccip_v2_sender`    | `5jYRxh4wyaupjbxJ4s5y1ypZCV1qhheZWaAY8MgYuNc5` |
| `ccip_v2_receiver`  | `FgFQwXGVeKDNawwer8Toa1xSKih1DgQp9JMjp3ft7SkA` |
| `ccip_v2_ping_pong` | `6A4neXDjavAyHnkGcVBckV2hQpBQhp7bXT2BS5JPR7Mh` |

`PingPongPeer` on Ethereum Sepolia: [`0x3D947A958fB28BA875D3275763d58F2ECD7721Df`](https://sepolia.etherscan.io/address/0x3d947a958fb28ba875d3275763d58f2ecd7721df#code).

Lane used throughout: Solana Devnet (router `Ccip842gzYHhvdDkSyi2YVCoAWPbYJoApMFzSxQroE9C`, selector
`16423721717087811551`) and Ethereum Sepolia (router `0x0BF3dE8c5D3e8A2B34D2BEeB17ABfCeBaf363A59`, selector
`16015286601757825753`). Deploy your own copies for anything beyond trying them out; the scripts read the program ids
from `scripts/.env`.

## Repository layout

```
programs/ccip-v2-sender      sender program
programs/ccip-v2-receiver    receiver program
programs/ccip-v2-ping-pong   send-and-receive program
programs/test-mock-*         stand-ins for the OffRamp and router, used only by the tests
tests/program-tests          LiteSVM tests (cargo test)
evm/                         PingPongPeer contract and its Foundry tests
scripts/                     TypeScript: setup, sends, policy, reads
```

## Sender program

Anyone can send through the program. The caller chooses the data, at most one token transfer and the fee token; the
owner fixes everything else per destination chain in a `RemoteChainConfig` (seeds `["remote_chain_config", selector]`):
the recipient and the extra args (`GenericExtraArgsV3`: gas limit, requested finality, CCVs, executor, token args).
`ccip_send` takes no recipient or extra args parameter, so a caller cannot change them.

- **Sender identity.** The program's `ccip_sender` PDA (seeds `["ccip_sender"]`) signs the CPI, so the destination sees
  that PDA as `message.sender` for every caller. The CCIP API reports the PDA as `sender` and the signing wallet as
  `origin`. A destination that allowlists senders approves the PDA once; a wallet sending directly is a different sender.
- **Tokens.** The caller first approves the `ccip_sender` PDA as delegate on its ATA. `ccip_send` moves the amount into
  the PDA's ATA (create it once with `init_ccip_sender_ata`) and delegates it to the router, which takes it.
- **Fees.** The caller passes `fee_token` (`Pubkey::default()` for native SOL) and `max_fee`. The program deposits
  `max_fee`, the router bills the quoted fee, the remainder goes back to the caller in the same instruction, and
  `MessageSent.fee_amount` reports what was billed. With a `max_fee` below the fee the transaction fails: in SOL with
  the System Program's insufficient lamports error, in a token with the token program's insufficient funds error.
  Quote with the SDK's `getFee` and add a margin.
- **Accounts.** The router's account list depends on the message, so the program implements the CCIP account
  resolution interface (`resolve_accounts_start`, `resolve_accounts_forward_router_stage`) and forwards the router's
  stages. `scripts/src/lib/common.ts` `resolveAccounts` runs the rounds by simulation; the final round's metadata is
  `ccip_send`'s `resolution_metadata`.
- **Cost.** A data message through the program uses about 32k more compute units than the same message sent directly
  by a wallet, for the PDA deposit, refund and event.

| Instruction                                                                  | Who               |
| ---------------------------------------------------------------------------- | ----------------- |
| `initialize(router)`                                                         | upgrade authority |
| `init_chain_config`, `update_chain_config`, `remove_chain_config`            | owner             |
| `init_ccip_sender_ata`                                                       | owner             |
| `ccip_send(params)`                                                          | anyone            |
| `withdraw_tokens`, `update_router`, `transfer_ownership`, `accept_ownership` | owner             |

## Receiver program

### Messages

What happens depends on the message:

| Message                                      | Receiver call | Result                                                                                                  |
| -------------------------------------------- | ------------- | ------------------------------------------------------------------------------------------------------- |
| data, no tokens                              | yes           | data stored in `ReceiverState` (first 256 bytes, full length recorded) and emitted in `MessageReceived` |
| tokens, empty data, gas limit 0              | no            | the OffRamp delivers the tokens; the program is not called and its policy is not consulted              |
| tokens and any data except a forward payload | yes           | tokens stay in the token admin ATA, data stored                                                         |
| tokens and a forward payload                 | yes           | `forward_bps` of the amount stays, the rest goes to the beneficiary's ATA                               |

Forward payload: `"FWD1"` (`0x46574431`) followed by the 32-byte beneficiary wallet, 36 bytes in total. The beneficiary
ATA is created if missing (rent paid by the `rent_payer` PDA, which the owner funds) and the transfer uses
`transfer_checked` through whichever token program owns the mint, so SPL Token and Token-2022 mints both work.
Mints with a transfer hook or a transfer fee are not supported in forward mode: the hook's extra accounts are not
passed, and a fee would leave less in the token admin ATA than the message amount.

For tokens to land with the program, the sender sets the token receiver (`tokenReceiver` in the extra args) to the
`receiver_token_admin` PDA. Its token accounts are what `withdraw_tokens` empties.

### Token receiver modes (`useATA`)

The Solana executor args (`SVMExecutorArgsV1`) carry `useATA`, which tells the OffRamp where to credit tokens:

| Value | Name                  | Token account credited                                   |
| ----- | --------------------- | -------------------------------------------------------- |
| 0     | `DeriveAtaAndCreate`  | ATA of the token receiver wallet, created if missing     |
| 1     | `DeriveAtaDontCreate` | ATA of the token receiver wallet, which must exist       |
| 2     | `UseAsIs`             | the token receiver itself, which must be a token account |

With 0, the OffRamp creates the ATA idempotently before verification and its transaction signer pays the rent. The
SDK uses 0 by default. The forward mode requires one of the ATA modes, so the token admin PDA's ATA is the one credited.

### Accounts

The OffRamp passes its own three accounts first (its CPI signer, its program id, the router's `allowed_offramp`
PDA). The sender lists the rest in `SVMExecutorArgsV1.accounts`, with a writable bitmap over that list:

| #   | Account                                                                   | Writable |
| --- | ------------------------------------------------------------------------- | -------- |
| 0   | `approved_sender`: `["approved_ccip_sender", source selector LE, sender]` |          |
| 1   | `config`: `["config"]`                                                    |          |
| 2   | `receiver_state`: `["receiver_state"]`                                    | yes      |

Forward mode appends nine: `receiver_token_admin`, its ATA (writable), the mint, the token program, the beneficiary,
the beneficiary ATA (writable), the associated token program, the system program, `rent_payer` (writable). The bitmap
for a forward is `(1 << 2) | (1 << 4) | (1 << 8) | (1 << 11)`.

The sender bytes are what the OffRamp delivers: 32 bytes for a Solana sender, and for an EVM sender the 20-byte address
left-padded with zeros to 32 bytes. Approve that form with `approve_sender`.

### CCV and finality policy

Before it executes a message that calls the program, the OffRamp asks `get_ccvs_and_finality_config` which CCVs must
have verified it and which finality the receiver accepts. The program answers from a `SourceChainCcvConfig` PDA
(`["ccv_config", source selector LE]`), the only account the OffRamp passes. It holds a chain policy and up to four
sender policies; a message from a sender with its own policy gets that one, every other sender gets the chain policy.

A policy has two independent halves:

- **CCVs.** `required_ccvs` must all verify the message; at least `optional_threshold` of `optional_ccvs` must too.
  `Pubkey::default()` in `required_ccvs` means "and the lane's default CCVs". With both lists empty and threshold 0 the
  lane defaults apply. Up to 8 per list; no duplicates; the threshold is 0 with no optional CCVs, otherwise between 1
  and the list length minus one.
- **Finality.** `allowed_finality_config: { flags: u16, block_depth: u16 }`. A message sent with full finality is always
  accepted. `flags` bit 0 (`0x1`) also accepts messages sent with `safe` finality. `block_depth = n` (n > 0) also accepts
  messages that requested `n` or more block confirmations. `{0, 0}` means finalized only.

| Spec in the scripts | `flags` | `block_depth` | Accepts                    |
| ------------------- | ------- | ------------- | -------------------------- |
| `finalized`         | 0       | 0             | finalized                  |
| `safe`              | 1       | 0             | finalized, safe            |
| `depth:5`           | 0       | 5             | finalized, depth 5 or more |
| `safe+depth:1`      | 1       | 1             | finalized, safe, any depth |

With no config for a source chain the answer is "lane defaults, finalized only". A message whose finality the policy
does not accept is not executed; it stays pending at the OffRamp until the policy allows it, and the executor then
delivers it. Token-only transfers skip the receiver and its policy.

| Instruction                                             | Effect                                                                                                           |
| ------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------- |
| `set_ccv_policy({ source, sender?, ccvs?, finality? })` | create or update the chain policy (`sender` none) or one sender's policy; a field passed as none keeps its value |
| `remove_sender_ccv_policy(source, sender)`              | that sender falls back to the chain policy                                                                       |
| `remove_ccv_config(source)`                             | deletes the account: lane defaults, finalized only                                                               |
| `get_ccvs_and_finality_config({ source, sender })`      | what the OffRamp reads; callable by anyone, e.g. in a simulation                                                 |

### Other instructions

`initialize(router, forward_bps)` (upgrade authority), `register_with_router` (registers the program with the router
as a CCIP 2.0 receiver, so the OffRamp uses `ccip_receive_v2` and the policy), `approve_sender`, `unapprove_sender`,
`set_forward_bps`, `withdraw_tokens(amount)` (any mint held by the token admin, SPL Token or Token-2022),
`withdraw_rent_payer`, `update_router`, `transfer_ownership`, `accept_ownership`. All owner only except where noted.

`ccip_receive_v2` checks that the OffRamp signed (its `external_execution_config` PDA), that the router allows that
OffRamp for the source chain, and that the (source chain, sender) pair is approved.

## Ping-pong program

`ccip_v2_ping_pong` both sends and receives. Each remote chain has a `Peer` (`["peer", selector]`, set by the owner):
the receiver bytes and extra args for messages to it, and the only sender accepted from it. Payloads are
`abi.encode(uint8 kind, uint64 sequence)`, kind 1 for a ping and 2 for a pong, so Solidity decodes them directly.

The `ccip_signer` PDA (`["ccip_signer"]`) signs every send and pays its fee in native SOL; the owner funds it. That PDA
is also what the peer sees as the sender, while messages to this program use the program id as the receiver. The EVM
peer therefore stores two values for a Solana peer: the program id to send to and the `ccip_signer` to accept.

- `ping(dest, resolution_metadata)`: owner only; remaining accounts are the router's `ccip_send_v2` accounts resolved
  for the `ccip_signer` PDA.
- `ccip_receive_v2`: records a pong; answers a ping according to `ReplyMode`.
- `send_pending_reply(chain, resolution_metadata)`: anyone; sends the oldest queued pong.
- `set_peer`, `remove_peer`, `set_reply_mode`, `set_ccv_policy`, `remove_ccv_config`, `withdraw_signer`, ownership.

### Reply strategies

`ReplyMode::Deferred` (the default) splits the reply from the delivery. `ccip_receive_v2` appends the ping's sequence
to the peer's queue (up to 16) and returns; `send_pending_reply` sends the oldest queued pong in its own transaction,
signed by anyone. The incoming message lists four accounts (`config`, `peer` writable, `state` writable,
`ccip_signer` writable) and its delivery never depends on the outgoing send. The price is a second transaction that
someone has to submit.

`ReplyMode::Immediate` sends the pong from inside `ccip_receive_v2`. The incoming message then lists the router and
its `ccip_send_v2` accounts after the four above (36 accounts on the Devnet lane), and the router needs the pong's
resolution metadata. A ping from the EVM peer carries only the 64-byte payload, so the program uses the metadata stored
on the peer (`set_peer`, `reply_resolution_metadata`; `ping-pong.ts evm-args --immediate` prints it). The send runs one
level below the OffRamp: OffRamp, receiver, router, and the router's own calls at the fourth level (`invoke [4]` in the
logs). The runtime caps how deep calls nest, so keep the reply path free of extra intermediate programs. The
receiver and its send use about 190k compute units (the whole execution about 510k), so set the ping's gas limit,
which becomes the receiver's compute budget, well above that; `evm-args --immediate --gas 600000` does.

If the pong cannot be sent (missing or stale metadata or router accounts, `ccip_signer` out of SOL), the whole
incoming message fails and is not delivered until the cause is fixed and the message is executed again, for example
with `ccip-cli manual-exec <sourceTx>`. Prefer `Deferred` unless the reply must be atomic with the delivery.

The EVM peer always replies from `ccipReceive`. Its messages need a gas limit that covers the callback and the
router send: about 585k gas on Sepolia (`ping-pong.ts peer --gas 700000`).

### When to combine sending and receiving in one program

Use one program when the replies are part of the protocol (acknowledgements, request and response): the peer table
gives one place for both directions and one identity to allowlist on the other side. Keep them separate when the two
sides have different owners, upgrade cadences or risk profiles, or when receiving must never spend funds: a program
that can send holds a fee balance that a bug in the receive path could drain.

## Build, test, deploy

Requirements: Rust, the Solana CLI 3.1 (`cargo build-sbf`), Anchor CLI 1.1.2, Node 22, Foundry, Docker (verifiable
builds).

```bash
anchor build                     # or: cargo build-sbf per program
cargo test --workspace           # LiteSVM tests load target/deploy/*.so
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings
(cd scripts && npm ci && npm run check)
(cd evm && npm ci && forge fmt --check && forge build --deny warnings && forge test)
```

Deploy your own copy: generate program keypairs, put their addresses in `declare_id!` and `Anchor.toml` (`anchor keys
sync`), build, then:

```bash
solana program deploy target/deploy/ccip_v2_receiver.so --program-id <keypair.json> -u devnet
```

### Verifiable build

The Devnet programs were built with [solana-verify](https://github.com/Ellipsis-Labs/solana-verifiable-build) in the
`solanafoundation/solana-verifiable-build:4.3.0` image:

```bash
solana-verify build -b solanafoundation/solana-verifiable-build:4.3.0 --library-name ccip_v2_receiver
solana-verify get-executable-hash target/deploy/ccip_v2_receiver.so
solana-verify get-program-hash -u devnet FgFQwXGVeKDNawwer8Toa1xSKih1DgQp9JMjp3ft7SkA
```

| Program             | Executable hash                                                    |
| ------------------- | ------------------------------------------------------------------ |
| `ccip_v2_sender`    | `4307779ca595b8be3159ad0d8392d50f6ee9fc9aa7b977404273814e282c30b7` |
| `ccip_v2_receiver`  | `521324756a146c95121e743d01d5b7fe2be4a787576bc0b80327ee14712b9771` |
| `ccip_v2_ping_pong` | `468ed16586d7b3bb099d869ab27dbeee7ed284ed4ce10cba6cf7bf916d9e69bc` |

Run the build from a clean checkout: a `target/` directory produced by a local toolchain makes the container build
fail.

To check a deployed program against this repository, run `verify-from-repo`. It rebuilds the program at the given
commit, compares the hash with the on-chain program and, when signed by the upgrade authority, uploads a verification
record that anyone can read back:

```bash
solana-verify verify-from-repo https://github.com/aelmanaa/ccip-solana-v2-examples --current-dir \
  --program-id FgFQwXGVeKDNawwer8Toa1xSKih1DgQp9JMjp3ft7SkA -u https://api.devnet.solana.com \
  --commit-hash <commit> --library-name ccip_v2_receiver -b solanafoundation/solana-verifiable-build:4.3.0
solana-verify get-program-pda --program-id FgFQwXGVeKDNawwer8Toa1xSKih1DgQp9JMjp3ft7SkA -u https://api.devnet.solana.com
```

Run it from a clone of this repository under your home directory (`--current-dir`): on macOS, Docker cannot see a
clone under `/tmp`. The remote verification service behind the explorers' "Verified Build" badge only accepts
mainnet programs, so on Devnet the verification is checked with `solana-verify` itself.

### IDL

Upload each IDL to the Program Metadata program so explorers decode instructions, accounts and arguments:

```bash
anchor idl build -p ccip_v2_receiver -o target/idl/ccip_v2_receiver.json
anchor idl init -f target/idl/ccip_v2_receiver.json FgFQwXGVeKDNawwer8Toa1xSKih1DgQp9JMjp3ft7SkA --provider.cluster devnet
anchor idl fetch FgFQwXGVeKDNawwer8Toa1xSKih1DgQp9JMjp3ft7SkA --provider.cluster devnet
```

## Scripts

```bash
cd scripts && npm ci && cp .env.example .env   # fill SEPOLIA_RPC_URL, EVM_PRIVATE_KEY, SOLANA_KEYPAIR
```

Run them with `node src/<script>.ts` (Node 22.18 or later runs TypeScript directly). Admin commands must be signed by
the program owner (`SOLANA_KEYPAIR`).

### Receiver

```bash
node src/receiver.ts init --forward-bps 250
node src/receiver.ts approve 16015286601757825753 0xYourSepoliaSender
node src/receiver.ts policy set 16015286601757825753 --finality depth:1            # finality only
node src/receiver.ts policy set 16015286601757825753 --required <ccv>              # CCVs only
node src/receiver.ts policy set 16015286601757825753 --sender 0xOne --required default,<ccv> --finality safe
node src/receiver.ts policy get 16015286601757825753 [--sender 0xOne]
node src/receiver.ts policy remove 16015286601757825753 [--sender 0xOne]
node src/receiver.ts state
node src/receiver.ts withdraw <mint> <amount> <destination token account>
```

### Sending to the receiver from Sepolia

```bash
node src/send-to-solana.ts data --text "hello"
node src/send-to-solana.ts token --token <erc20> --amount <wei>
node src/send-to-solana.ts forward --token <erc20> --mint <solana mint> --amount <wei> [--beneficiary <wallet>]
# options: --finality finalized|safe|<depth>  --gas <compute units>  --fee-token <erc20>  --dry-run
```

The script builds the message with `SolanaChain.buildMessageForDest` (receiver accounts and writable bitmap in the
executor args), quotes it with `EVMChain.getFee` and sends it with `EVMChain.sendMessage`.

The same token-only transfer with [`ccip-cli`](https://www.npmjs.com/package/@chainlink/ccip-cli):

```bash
ccip-cli send -s ethereum-testnet-sepolia -d solana-devnet -r 0x0BF3dE8c5D3e8A2B34D2BEeB17ABfCeBaf363A59 \
  --to FgFQwXGVeKDNawwer8Toa1xSKih1DgQp9JMjp3ft7SkA \
  --token-receiver <receiver_token_admin PDA> -t <erc20>=1 -L 0 [--fee-token LINK] [-x finality=1]
```

Data and forward messages into the receiver are sent with the SDK script above.

### Sending from Solana through the sender program

```bash
node src/sender.ts init
node src/sender.ts ata <mint>                                   # for tokens sent or fees paid in that mint
node src/sender.ts config 16015286601757825753 --recipient 0xReceiver --gas 200000 [--finality 5] [--ccvs 0x..]
node src/sender.ts send 16015286601757825753 --data "hello" [--token <mint> --amount <units>] [--fee-token <mint>]
```

### Ping-pong

```bash
node src/ping-pong.ts init --mode deferred
node src/ping-pong.ts peer 16015286601757825753 --address 0xPeer --gas 700000
node src/ping-pong.ts evm-args            # receiver, accepted sender and extra args for PingPongPeer.setPeer
node src/ping-pong.ts ping 16015286601757825753
node src/ping-pong.ts reply 16015286601757825753
node src/ping-pong.ts state 16015286601757825753
```

### Reading results

Every send prints the message id and a link to the CCIP explorer. `ccip-cli show <messageId>` shows the message and its
execution, and `ccip-cli show <sourceTx> --no-api` reads it from the chains alone. On Solana, `receiver.ts state` and
`ping-pong.ts state` decode the programs' records.

## Limits to keep in mind

- Every account a Solana receiver needs is listed in the message's executor args, and the whole execution must fit
  one transaction. The executor loads them through an address lookup table, so a few dozen fit: the receiver's forward
  mode lists 12, the immediate ping-pong reply 36.
- The receiver program gets the message's gas limit as its compute budget, up to about 1M compute units.
- Receiver data is stored up to 256 bytes; longer data is still emitted in `MessageReceived`.
- A message whose callback runs out of gas fails on the destination (execution state 3) and can be executed again
  manually with a higher gas limit, for example `ccip-cli manual-exec <sourceTx> -L <gas>`. Size the gas limit of
  messages to contracts that send in their callback (the EVM ping-pong peer) for both the callback and its send.

## License

MIT. See [LICENSE](LICENSE).
