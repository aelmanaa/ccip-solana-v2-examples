// Sender program admin and sends from Solana.
//
//   node src/sender.ts init                                   initialize (upgrade authority only)
//   node src/sender.ts ata <mint>                             the program's ATA for a token it sends or pays fees in
//   node src/sender.ts config <destSelector> --recipient <0x address> [--gas 200000]
//                     [--finality finalized|safe|<depth>] [--ccvs <0x a,b>]      create or update the chain config
//   node src/sender.ts send <destSelector> [--data <text>] [--token <mint> --amount <base units>]
//                     [--fee-token native|<mint>] [--slippage-bps 2000] [--max-fee <base units>]
//   node src/sender.ts direct <destSelector> [same options]   the same message sent by the wallet itself (no program)
//
// Recipient and extra args (gas, finality, CCVs) are fixed per destination by the owner's chain config;
// `send` only chooses data, the token transfer and the fee token.
import {
    ComputeBudgetProgram,
    type Keypair,
    PublicKey,
    SystemProgram,
    type Transaction,
    TransactionInstruction,
    type VersionedTransaction,
} from '@solana/web3.js'
import { hexlify } from 'ethers'

import { ChainFamily, type ExtraArgs, SolanaChain, decodeExtraArgs, encodeExtraArgs } from '@chainlink/ccip-sdk'

import {
    ATA_PROGRAM,
    ROUTER_SEND_LUT,
    SENDER_PROGRAM,
    SOLANA_ROUTER,
    WSOL,
    args,
    ata,
    ccipExplorer,
    conn,
    disc,
    evmAddressBytes,
    explorerTx,
    fetchLuts,
    le64,
    loadKeypair,
    meta,
    option,
    pda,
    programData,
    resolveAccounts,
    routerConfig,
    sendV0,
    solanaRpc,
    vecBytes,
    quietLogger,
} from './lib/common.ts'

const P = SENDER_PROGRAM
const TX = {
    state: pda([Buffer.from('state')], P),
    ccipSender: pda([Buffer.from('ccip_sender')], P),
    eventAuthority: pda([Buffer.from('__event_authority')], P),
    chainConfig: (sel: bigint): PublicKey => pda([Buffer.from('remote_chain_config'), le64(sel)], P),
}

const payer = loadKeypair()
const me = payer.publicKey
const { positional, opts } = args()
const [cmd, destArg] = positional
const dest = cmd === 'ata' ? 0n : BigInt(destArg ?? '0')

async function run(name: string, ixs: TransactionInstruction[]): Promise<void> {
    const r = await sendV0(payer, ixs)
    console.log(`${name}: ${explorerTx(r.signature)}`)
}

/** SPL `Approve` (instruction 4): lets `delegate` move `amount` out of `source`. */
function approve(
    source: PublicKey,
    delegate: PublicKey,
    amount: bigint,
    tokenProgram: PublicKey,
): TransactionInstruction {
    const data = Buffer.alloc(9)
    data[0] = 4
    data.writeBigUInt64LE(amount, 1)
    return new TransactionInstruction({
        programId: tokenProgram,
        keys: [meta(source, true), meta(delegate), meta(me, false, true)],
        data,
    })
}

async function tokenProgramOf(mint: PublicKey): Promise<PublicKey> {
    const info = await conn.getAccountInfo(mint)
    if (info === null) throw new Error(`mint ${mint.toBase58()} not found`)
    return info.owner
}

function finalityOpt(): 'finalized' | 'safe' | number {
    const f = opts.get('finality') ?? 'finalized'
    return f === 'finalized' || f === 'safe' ? f : Number(f)
}

/** The message as the SDK sees it, for quoting and for the direct send. */
async function sdkMessage(recipient: string): Promise<Parameters<SolanaChain['getFee']>[0]['message']> {
    const token = opts.get('token')
    const feeToken = opts.get('fee-token') ?? 'native'
    const cfg = await readChainConfig()
    return {
        receiver: recipient,
        data: hexlify(Buffer.from(opts.get('data') ?? '', 'utf8')),
        tokenAmounts: token ? [{ token, amount: BigInt(opts.get('amount') ?? '0') }] : [],
        ...(feeToken === 'native' ? {} : { feeToken }),
        extraArgs: cfg.extraArgs,
    }
}

/** Decodes the stored `RemoteChainConfig` (recipient + extra args) for `dest`. */
async function readChainConfig(): Promise<{ recipient: Buffer; extraArgsHex: string; extraArgs: ExtraArgs }> {
    const info = await conn.getAccountInfo(TX.chainConfig(dest))
    if (info === null) throw new Error(`no chain config for ${dest}; run: sender.ts config ${dest} --recipient ...`)
    const d = info.data
    let o = 8 + 2
    const rn = d.readUInt32LE(o)
    o += 4
    const recipient = Buffer.from(d.subarray(o, o + rn))
    o += rn
    const en = d.readUInt32LE(o)
    o += 4
    const extraArgsHex = hexlify(d.subarray(o, o + en))
    const decoded = decodeExtraArgs(extraArgsHex, ChainFamily.Solana)
    if (decoded?._tag !== 'GenericExtraArgsV3') throw new Error(`chain config extra args are not GenericExtraArgsV3`)
    // Decoding renders the empty executor as the zero pubkey and the empty token receiver as 0x;
    // map both back to "not set" so the SDK quotes exactly the stored bytes.
    const extraArgs: ExtraArgs = {
        gasLimit: decoded.gasLimit,
        finality: decoded.finality,
        ccvs: decoded.ccvs,
        ccvArgs: decoded.ccvArgs,
        executor: decoded.executor === PublicKey.default.toBase58() ? '' : decoded.executor,
        executorArgs: decoded.executorArgs,
        tokenReceiver: decoded.tokenReceiver === '0x' ? '' : decoded.tokenReceiver,
        tokenArgs: decoded.tokenArgs,
    }
    return { recipient, extraArgsHex, extraArgs }
}

const keypairWallet = (
    kp: Keypair,
): { publicKey: PublicKey; signTransaction: <T extends Transaction | VersionedTransaction>(tx: T) => Promise<T> } => ({
    publicKey: kp.publicKey,
    signTransaction: <T extends Transaction | VersionedTransaction>(tx: T): Promise<T> => {
        if ('version' in tx) tx.sign([kp])
        else tx.partialSign(kp)
        return Promise.resolve(tx)
    },
})

switch (cmd) {
    case 'init': {
        await run('initialize', [
            new TransactionInstruction({
                programId: P,
                keys: [
                    meta(TX.state, true),
                    meta(TX.ccipSender),
                    meta(me, true, true),
                    meta(SystemProgram.programId),
                    meta(P),
                    meta(programData(P)),
                ],
                data: Buffer.concat([disc('initialize'), SOLANA_ROUTER.toBuffer()]),
            }),
        ])
        break
    }
    case 'ata': {
        // The PDA's ATA for a mint it will transfer or pay fees in.
        const mint = new PublicKey(destArg ?? '')
        const tp = await tokenProgramOf(mint)
        await run('init_ccip_sender_ata', [
            new TransactionInstruction({
                programId: P,
                keys: [
                    meta(TX.state),
                    meta(ata(TX.ccipSender, mint, tp), true),
                    meta(mint),
                    meta(TX.ccipSender),
                    meta(me, true, true),
                    meta(tp),
                    meta(ATA_PROGRAM),
                    meta(SystemProgram.programId),
                ],
                data: disc('init_ccip_sender_ata'),
            }),
        ])
        break
    }
    case 'config': {
        const recipient = evmAddressBytes(opts.get('recipient') ?? '')
        const ccvs = (opts.get('ccvs') ?? '').split(',').filter((x) => x !== '')
        const extraArgs = Buffer.from(
            encodeExtraArgs(
                {
                    gasLimit: BigInt(opts.get('gas') ?? '200000'),
                    finality: finalityOpt(),
                    ccvs,
                    ccvArgs: ccvs.map(() => '0x'),
                    executor: '',
                    executorArgs: '0x',
                    tokenReceiver: '',
                    tokenArgs: '0x',
                },
                ChainFamily.Solana,
            ).slice(2),
            'hex',
        )
        const exists = (await conn.getAccountInfo(TX.chainConfig(dest))) !== null
        const name = exists ? 'update_chain_config' : 'init_chain_config'
        await run(name, [
            new TransactionInstruction({
                programId: P,
                keys: [
                    meta(TX.state, true),
                    meta(TX.chainConfig(dest), true),
                    meta(me, true, true),
                    meta(SystemProgram.programId),
                ],
                data: Buffer.concat([disc(name), le64(dest), vecBytes(recipient), vecBytes(extraArgs)]),
            }),
        ])
        console.log({ extraArgs: hexlify(extraArgs) })
        break
    }
    case 'send':
    case 'direct': {
        const cfg = await readChainConfig()
        const recipient = `0x${cfg.recipient.subarray(12).toString('hex')}`
        const sol = await SolanaChain.fromUrl(solanaRpc(), { logger: quietLogger })
        const message = await sdkMessage(recipient)
        const quote = await sol.getFee({ router: SOLANA_ROUTER.toBase58(), destChainSelector: dest, message })
        const slippage = BigInt(opts.get('slippage-bps') ?? '2000')
        const maxFee = BigInt(opts.get('max-fee') ?? ((quote * (10_000n + slippage)) / 10_000n).toString())
        console.log({ quote: quote.toString(), maxFee: maxFee.toString(), extraArgs: cfg.extraArgsHex })

        if (cmd === 'direct') {
            const req = await sol.sendMessage({
                router: SOLANA_ROUTER.toBase58(),
                destChainSelector: dest,
                message: { ...message, fee: quote },
                wallet: keypairWallet(payer),
            })
            console.log({
                messageId: req.message.messageId,
                tx: explorerTx(req.tx.hash),
                explorer: ccipExplorer(req.message.messageId),
            })
            break
        }

        const tokenMint = opts.get('token')
        const token = tokenMint
            ? { mint: new PublicKey(tokenMint), amount: BigInt(opts.get('amount') ?? '0') }
            : undefined
        const feeTokenOpt = opts.get('fee-token') ?? 'native'
        const feeToken = feeTokenOpt === 'native' ? PublicKey.default : new PublicKey(feeTokenOpt)
        const feeMint = feeTokenOpt === 'native' ? WSOL : feeToken
        const feeProgram = await tokenProgramOf(feeMint)
        const ixData = (resolutionMetadata: Uint8Array): Buffer =>
            Buffer.concat([
                disc('ccip_send'),
                le64(dest),
                option(token && Buffer.concat([token.mint.toBuffer(), le64(token.amount)])),
                vecBytes(Buffer.from(opts.get('data') ?? '', 'utf8')),
                feeToken.toBuffer(),
                le64(maxFee),
                vecBytes(resolutionMetadata),
            ])
        const resolved = await resolveAccounts({
            program: P,
            caller: me,
            ixData: ixData(new Uint8Array()),
            startAccounts: [
                TX.state,
                TX.chainConfig(dest),
                feeMint,
                SOLANA_ROUTER,
                routerConfig(),
                ...(token ? [token.mint] : []),
            ],
            payer: me,
        })
        const ixs: TransactionInstruction[] = [
            ComputeBudgetProgram.setComputeUnitLimit({ units: 1_400_000 }),
            ComputeBudgetProgram.requestHeapFrame({ bytes: 256 * 1024 }),
        ]
        // The program pulls the user's tokens through a delegation to its `ccip_sender` PDA.
        const sameMint = token !== undefined && feeTokenOpt !== 'native' && token.mint.equals(feeToken)
        if (token) {
            const tp = await tokenProgramOf(token.mint)
            ixs.push(approve(ata(me, token.mint, tp), TX.ccipSender, token.amount + (sameMint ? maxFee : 0n), tp))
        }
        if (feeTokenOpt !== 'native' && !sameMint)
            ixs.push(approve(ata(me, feeMint, feeProgram), TX.ccipSender, maxFee, feeProgram))
        ixs.push(new TransactionInstruction({ programId: P, keys: resolved.accounts, data: ixData(resolved.metadata) }))
        const luts = await fetchLuts([...resolved.lookupTables, ROUTER_SEND_LUT])
        const sent = await sendV0(payer, ixs, luts)
        const reqs = await sol.getMessagesInTx(sent.signature)
        const req = reqs[0]
        console.log({
            messageId: req?.message.messageId,
            sender: TX.ccipSender.toBase58(),
            tx: explorerTx(sent.signature),
            computeUnits: sent.computeUnits,
            accounts: resolved.accounts.length,
            resolutionRounds: resolved.rounds,
            ...(req ? { explorer: ccipExplorer(req.message.messageId) } : {}),
        })
        break
    }
    default:
        console.log('usage: sender.ts init|config|send|direct (see header)')
}
