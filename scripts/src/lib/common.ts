// Shared helpers: environment, keys, Borsh encoding, PDA derivation, account resolution, sending.
import { createHash } from 'node:crypto'
import { existsSync, readFileSync } from 'node:fs'
import { homedir } from 'node:os'
import { resolve } from 'node:path'

import {
    type AccountMeta,
    type AddressLookupTableAccount,
    ComputeBudgetProgram,
    Connection,
    Keypair,
    PublicKey,
    TransactionInstruction,
    TransactionMessage,
    VersionedTransaction,
} from '@solana/web3.js'
import { JsonRpcProvider, Wallet } from 'ethers'

const envFile = resolve(import.meta.dirname, '../../.env')
if (existsSync(envFile)) process.loadEnvFile(envFile)

function env(name: string, fallback?: string): string {
    const v = process.env[name] ?? fallback
    if (v === undefined || v === '') throw new Error(`missing env ${name} (see scripts/.env.example)`)
    return v
}

// ---- networks (CCIP 2.0 testnet lane Solana Devnet <-> Ethereum Sepolia) ----
export const SOLANA_DEVNET_SELECTOR = 16423721717087811551n
export const SEPOLIA_SELECTOR = 16015286601757825753n
export const SOLANA_ROUTER = new PublicKey(env('SOLANA_ROUTER', 'Ccip842gzYHhvdDkSyi2YVCoAWPbYJoApMFzSxQroE9C'))
export const SEPOLIA_ROUTER = env('SEPOLIA_ROUTER', '0x0BF3dE8c5D3e8A2B34D2BEeB17ABfCeBaf363A59')
/** Lookup table of the Devnet router's `ccip_send_v2` accounts. */
export const ROUTER_SEND_LUT = new PublicKey(
    env('SOLANA_ROUTER_SEND_LUT', '43j7AWfB2STvzwKxik64fD12Swdz56rNTA5VQnCWwEXL'),
)

// ---- the example programs ----
export const SENDER_PROGRAM = new PublicKey(env('SENDER_PROGRAM', '5jYRxh4wyaupjbxJ4s5y1ypZCV1qhheZWaAY8MgYuNc5'))
export const RECEIVER_PROGRAM = new PublicKey(env('RECEIVER_PROGRAM', 'FgFQwXGVeKDNawwer8Toa1xSKih1DgQp9JMjp3ft7SkA'))
export const PING_PONG_PROGRAM = new PublicKey(env('PING_PONG_PROGRAM', '6A4neXDjavAyHnkGcVBckV2hQpBQhp7bXT2BS5JPR7Mh'))

export const BPF_UPGRADEABLE = new PublicKey('BPFLoaderUpgradeab1e11111111111111111111111')
export const TOKEN_PROGRAM = new PublicKey('TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA')
export const TOKEN_2022_PROGRAM = new PublicKey('TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb')
export const ATA_PROGRAM = new PublicKey('ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL')
export const SYSTEM_PROGRAM = new PublicKey('11111111111111111111111111111111')
export const WSOL = new PublicKey('So11111111111111111111111111111111111111112')

export const solanaRpc = (): string => env('SOLANA_RPC_URL', 'https://api.devnet.solana.com')
export const evmRpc = (): string => env('SEPOLIA_RPC_URL')
export const conn = new Connection(solanaRpc(), 'confirmed')

export function solanaKeypairPath(): string {
    return env('SOLANA_KEYPAIR', resolve(homedir(), '.config/solana/id.json'))
}

export function loadKeypair(path = solanaKeypairPath()): Keypair {
    const raw: unknown = JSON.parse(readFileSync(path, 'utf8'))
    if (!Array.isArray(raw) || !raw.every((x) => typeof x === 'number')) throw new Error(`bad keypair file ${path}`)
    return Keypair.fromSecretKey(Uint8Array.from(raw))
}

export function evmWallet(): Wallet {
    return new Wallet(env('EVM_PRIVATE_KEY'), new JsonRpcProvider(evmRpc()))
}

// ---- Borsh ----
export const u8 = (v: number): Buffer => Buffer.from([v])
export const le16 = (v: number): Buffer => {
    const b = Buffer.alloc(2)
    b.writeUInt16LE(v)
    return b
}
export const le32 = (v: number): Buffer => {
    const b = Buffer.alloc(4)
    b.writeUInt32LE(v)
    return b
}
export const le64 = (v: bigint): Buffer => {
    const b = Buffer.alloc(8)
    b.writeBigUInt64LE(v)
    return b
}
export const vecBytes = (b: Uint8Array): Buffer => Buffer.concat([le32(b.length), Buffer.from(b)])
export const vecPk = (ks: readonly PublicKey[]): Buffer =>
    Buffer.concat([le32(ks.length), ...ks.map((k) => k.toBuffer())])
export const option = (inner: Buffer | undefined): Buffer => (inner ? Buffer.concat([u8(1), inner]) : u8(0))
/** Anchor instruction discriminator. */
export const disc = (name: string): Buffer => createHash('sha256').update(`global:${name}`).digest().subarray(0, 8)
/** Anchor account discriminator. */
export const accountDisc = (name: string): Buffer =>
    createHash('sha256').update(`account:${name}`).digest().subarray(0, 8)

export const pda = (seeds: (Buffer | Uint8Array)[], program: PublicKey): PublicKey =>
    PublicKey.findProgramAddressSync(seeds, program)[0]
export const ata = (owner: PublicKey, mint: PublicKey, tokenProgram = TOKEN_PROGRAM): PublicKey =>
    pda([owner.toBuffer(), tokenProgram.toBuffer(), mint.toBuffer()], ATA_PROGRAM)
export const programData = (program: PublicKey): PublicKey => pda([program.toBuffer()], BPF_UPGRADEABLE)
export const meta = (pubkey: PublicKey, isWritable = false, isSigner = false): AccountMeta => ({
    pubkey,
    isSigner,
    isWritable,
})
/** An EVM address as the bytes CCIP uses on Solana: left-padded to 32 bytes. */
export const evmAddressBytes = (address: string): Buffer =>
    Buffer.concat([Buffer.alloc(12), Buffer.from(address.replace(/^0x/, ''), 'hex')])

export const routerConfig = (): PublicKey => pda([Buffer.from('config')], SOLANA_ROUTER)
export const routerEventAuthority = (): PublicKey => pda([Buffer.from('__event_authority')], SOLANA_ROUTER)
export const receiverRegistry = (receiver: PublicKey): PublicKey =>
    pda([Buffer.from('receiver_registry'), receiver.toBuffer()], SOLANA_ROUTER)

// ---- CCIP account resolution (ccip_common::resolution), simulated round by round ----
interface ResolveResponse {
    askAgainWith: PublicKey[]
    accountsToSave: AccountMeta[]
    lookupTables: PublicKey[]
    next: Buffer | null
    metadata: Buffer
}

function decodeResolveResponse(b: Buffer): ResolveResponse {
    let o = 0
    const u32 = (): number => {
        const v = b.readUInt32LE(o)
        o += 4
        return v
    }
    const pk = (): PublicKey => {
        const v = new PublicKey(b.subarray(o, o + 32))
        o += 32
        return v
    }
    const askAgainWith = Array.from({ length: u32() }, pk)
    const accountsToSave = Array.from({ length: u32() }, () => {
        const pubkey = pk()
        const isSigner = b[o] === 1
        const isWritable = b[o + 1] === 1
        o += 2
        return { pubkey, isSigner, isWritable }
    })
    const lookupTables = Array.from({ length: u32() }, pk)
    let next: Buffer | null = null
    if (b[o] === 1) {
        next = Buffer.from(b.subarray(o + 1, o + 9))
        o += 9
    } else o += 1
    const n = u32()
    return { askAgainWith, accountsToSave, lookupTables, next, metadata: Buffer.from(b.subarray(o, o + n)) }
}

export interface Resolved {
    accounts: AccountMeta[]
    lookupTables: PublicKey[]
    metadata: Buffer
    rounds: number
}

/**
 * Runs `program`'s `resolve_accounts_*` stages by simulation until it reports completion. Works for
 * the router itself (start account: its config PDA) and for any program that implements the same
 * interface, such as the sender example (start accounts: see its `resolve_accounts` module docs).
 */
export async function resolveAccounts(opts: {
    program: PublicKey
    caller: PublicKey
    ixData: Buffer
    startAccounts: PublicKey[]
    payer: PublicKey
}): Promise<Resolved> {
    const accounts: AccountMeta[] = []
    const lookupTables: PublicKey[] = []
    let discriminator = disc('resolve_accounts_start')
    let ask = opts.startAccounts
    let metadata: Buffer = Buffer.alloc(0)
    const { blockhash } = await conn.getLatestBlockhash()
    for (let round = 1; round <= 64; round++) {
        const data = Buffer.concat([discriminator, opts.caller.toBuffer(), vecBytes(opts.ixData), vecBytes(metadata)])
        const ix = new TransactionInstruction({ programId: opts.program, keys: ask.map((k) => meta(k)), data })
        const msg = new TransactionMessage({
            payerKey: opts.payer,
            recentBlockhash: blockhash,
            instructions: [ComputeBudgetProgram.requestHeapFrame({ bytes: 256 * 1024 }), ix],
        }).compileToV0Message()
        const sim = await conn.simulateTransaction(new VersionedTransaction(msg), { sigVerify: false })
        if (sim.value.err !== null) {
            const logs = (sim.value.logs ?? []).slice(-6).join(' | ')
            throw new Error(`resolution round ${round} failed: ${JSON.stringify(sim.value.err)} ${logs}`)
        }
        const rd = sim.value.returnData
        if (rd?.programId !== opts.program.toBase58()) throw new Error(`resolution round ${round}: no return data`)
        const r = decodeResolveResponse(Buffer.from(rd.data[0], 'base64'))
        accounts.push(...r.accountsToSave)
        lookupTables.push(...r.lookupTables)
        metadata = r.metadata
        if (r.next === null) return { accounts, lookupTables, metadata, rounds: round }
        discriminator = r.next
        ask = r.askAgainWith
    }
    throw new Error('resolution did not finish in 64 rounds')
}

/** Router `ccip_send_v2` instruction data (no tokens) for `CcipSendV2Params`. */
export function routerSendIxData(p: {
    destChainSelector: bigint
    receiver: Uint8Array
    data: Uint8Array
    feeToken?: PublicKey
    extraArgs: Uint8Array
    resolutionMetadata?: Uint8Array
}): Buffer {
    return Buffer.concat([
        disc('ccip_send_v2'),
        le64(p.destChainSelector),
        vecBytes(p.receiver),
        vecBytes(p.data),
        le32(0),
        (p.feeToken ?? PublicKey.default).toBuffer(),
        vecBytes(p.extraArgs),
        vecBytes(p.resolutionMetadata ?? new Uint8Array()),
    ])
}

export async function fetchLuts(keys: PublicKey[]): Promise<AddressLookupTableAccount[]> {
    const unique = [...new Map(keys.map((k) => [k.toBase58(), k])).values()]
    const out: AddressLookupTableAccount[] = []
    for (const k of unique) {
        const r = await conn.getAddressLookupTable(k)
        if (r.value === null) throw new Error(`lookup table ${k.toBase58()} not found`)
        out.push(r.value)
    }
    return out
}

export interface Sent {
    signature: string
    computeUnits: number | null
}

/** Signs and sends a v0 transaction, waits for confirmation, returns the signature and CU used. */
export async function sendV0(
    payer: Keypair,
    instructions: TransactionInstruction[],
    luts: AddressLookupTableAccount[] = [],
    signers: Keypair[] = [],
): Promise<Sent> {
    const { blockhash, lastValidBlockHeight } = await conn.getLatestBlockhash()
    const msg = new TransactionMessage({ payerKey: payer.publicKey, recentBlockhash: blockhash, instructions })
    const tx = new VersionedTransaction(msg.compileToV0Message(luts))
    tx.sign([payer, ...signers])
    const signature = await conn.sendTransaction(tx)
    const res = await conn.confirmTransaction({ signature, blockhash, lastValidBlockHeight }, 'confirmed')
    if (res.value.err !== null) throw new Error(`tx ${signature} failed: ${JSON.stringify(res.value.err)}`)
    const t = await conn.getTransaction(signature, { maxSupportedTransactionVersion: 0, commitment: 'confirmed' })
    return { signature, computeUnits: t?.meta?.computeUnitsConsumed ?? null }
}

/** Simulates instructions and returns the return data of `program`, if any. */
export async function simulateReturn(
    payer: PublicKey,
    instructions: TransactionInstruction[],
    program: PublicKey,
): Promise<Buffer> {
    const { blockhash } = await conn.getLatestBlockhash()
    const msg = new TransactionMessage({ payerKey: payer, recentBlockhash: blockhash, instructions })
    const sim = await conn.simulateTransaction(new VersionedTransaction(msg.compileToV0Message()), {
        sigVerify: false,
    })
    if (sim.value.err !== null) throw new Error(`simulation failed: ${JSON.stringify(sim.value.err)}`)
    const rd = sim.value.returnData
    if (rd?.programId !== program.toBase58()) return Buffer.alloc(0)
    return Buffer.from(rd.data[0], 'base64')
}

/** Reads `--name value` and `--flag` style arguments. */
export function args(argv = process.argv.slice(2)): { positional: string[]; opts: Map<string, string> } {
    const positional: string[] = []
    const opts = new Map<string, string>()
    for (let i = 0; i < argv.length; i++) {
        const a = argv[i] ?? ''
        if (a.startsWith('--')) {
            const next = argv[i + 1]
            if (next !== undefined && !next.startsWith('--')) {
                opts.set(a.slice(2), next)
                i++
            } else opts.set(a.slice(2), 'true')
        } else positional.push(a)
    }
    return { positional, opts }
}

/** Logger that keeps SDK debug and info output off the console. */
export const quietLogger = {
    debug: (): void => undefined,
    info: (): void => undefined,
    warn: console.warn,
    error: console.error,
}

export const explorerTx = (sig: string): string => `https://explorer.solana.com/tx/${sig}?cluster=devnet`
export const ccipExplorer = (messageId: string): string => `https://ccip.chain.link/msg/${messageId}`
