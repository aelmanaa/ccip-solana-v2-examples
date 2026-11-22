// Sends a message from Ethereum Sepolia (an EOA, through the Router) to the receiver program.
//
//   node src/send-to-solana.ts data    [--text "hello"]
//   node src/send-to-solana.ts token   --token <erc20> --amount <base units>        token-only, no receiver call
//   node src/send-to-solana.ts forward --token <erc20> --mint <solana mint> --amount <base units> [--beneficiary <wallet>]
//   common options: [--finality finalized|safe|<depth>] [--gas <units>] [--fee-token <erc20>] [--dry-run]
//
// The sender must be approved on the receiver first (`receiver.ts approve <sepoliaSelector> <EOA>`).
import { Keypair, PublicKey } from '@solana/web3.js'
import { hexlify } from 'ethers'

import { EVMChain, SolanaChain } from '@chainlink/ccip-sdk'

import {
    RECEIVER_PROGRAM,
    SEPOLIA_ROUTER,
    SEPOLIA_SELECTOR,
    SOLANA_DEVNET_SELECTOR,
    args,
    ccipExplorer,
    evmRpc,
    evmWallet,
    quietLogger,
} from './lib/common.ts'
import { RX, forwardPayload, receiverAccounts, senderBytes } from './lib/receiver.ts'

const { positional, opts } = args()
const mode = positional[0] ?? 'data'
const wallet = evmWallet()
const evm = await EVMChain.fromUrl(evmRpc(), { logger: quietLogger })

const finalityOpt = opts.get('finality') ?? 'finalized'
const finality = finalityOpt === 'finalized' || finalityOpt === 'safe' ? finalityOpt : Number(finalityOpt)
const token = opts.get('token')
const amount = BigInt(opts.get('amount') ?? '0')
const sender = senderBytes(wallet.address)

let receiver: string
let data: string
let gasLimit: bigint
let accounts: PublicKey[] = []
let writableBitmap = 0n
let tokenReceiver: string | undefined
switch (mode) {
    case 'data': {
        receiver = RECEIVER_PROGRAM.toBase58()
        data = hexlify(Buffer.from(opts.get('text') ?? 'hello from sepolia', 'utf8'))
        gasLimit = BigInt(opts.get('gas') ?? '200000')
        ;({ accounts, writableBitmap } = receiverAccounts(SEPOLIA_SELECTOR, sender))
        break
    }
    case 'token': {
        // No data and no gas: the OffRamp delivers the tokens without calling the program. With the
        // program as `receiver` and its token admin PDA as `tokenReceiver`, they land in that PDA's ATA.
        if (token === undefined) throw new Error('--token required')
        receiver = RECEIVER_PROGRAM.toBase58()
        data = '0x'
        gasLimit = 0n
        tokenReceiver = RX.tokenAdmin.toBase58()
        break
    }
    case 'forward': {
        if (token === undefined) throw new Error('--token required')
        const beneficiary = new PublicKey(opts.get('beneficiary') ?? Keypair.generate().publicKey.toBase58())
        const mintOpt = opts.get('mint')
        if (mintOpt === undefined) throw new Error('--mint <solana mint of the token> required')
        const mint = new PublicKey(mintOpt)
        receiver = RECEIVER_PROGRAM.toBase58()
        data = hexlify(forwardPayload(beneficiary))
        gasLimit = BigInt(opts.get('gas') ?? '300000')
        tokenReceiver = RX.tokenAdmin.toBase58()
        ;({ accounts, writableBitmap } = receiverAccounts(SEPOLIA_SELECTOR, sender, { mint, beneficiary }))
        console.log({ beneficiary: beneficiary.toBase58(), mint: mint.toBase58() })
        break
    }
    default:
        throw new Error(`unknown mode ${mode}: data | token | forward`)
}

const feeToken = opts.get('fee-token')
const message = SolanaChain.buildMessageForDest({
    receiver,
    data,
    tokenAmounts: token ? [{ token, amount }] : [],
    ...(feeToken ? { feeToken } : {}),
    extraArgs: {
        gasLimit,
        finality,
        accounts: accounts.map((a) => a.toBase58()),
        accountIsWritableBitmap: writableBitmap,
        ...(tokenReceiver ? { tokenReceiver } : {}),
    },
})
const fee = await evm.getFee({ router: SEPOLIA_ROUTER, destChainSelector: SOLANA_DEVNET_SELECTOR, message })
console.log({
    mode,
    receiver,
    data,
    gasLimit: gasLimit.toString(),
    finality,
    accounts: accounts.length,
    fee: fee.toString(),
})
if (opts.has('dry-run')) process.exit(0)

const request = await evm.sendMessage({
    router: SEPOLIA_ROUTER,
    destChainSelector: SOLANA_DEVNET_SELECTOR,
    message: { ...message, fee },
    wallet,
})
console.log({
    messageId: request.message.messageId,
    tx: request.tx.hash,
    explorer: ccipExplorer(request.message.messageId),
})
