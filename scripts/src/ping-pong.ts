// Ping-pong program admin, sends and reads.
//
//   node src/ping-pong.ts init [--mode deferred|immediate]     initialize, register with the router, fund ccip_signer
//   node src/ping-pong.ts peer <selector> --address <0x peer> [--gas 400000] [--finality finalized|safe|<depth>]
//                     [--reply-metadata 0x..]   router resolution metadata for immediate pongs (from evm-args --immediate)
//   node src/ping-pong.ts mode deferred|immediate
//   node src/ping-pong.ts ping <selector>                       owner only; fee paid by ccip_signer
//   node src/ping-pong.ts reply <selector>                      send the oldest queued pong (anyone)
//   node src/ping-pong.ts state
//   node src/ping-pong.ts evm-args [--gas 200000] [--finality <f>] [--immediate]
//       what the EVM peer needs: this program's receiver bytes, its accepted sender, and the extra args
//       for messages to it (with --immediate, the router accounts for the pong are appended)
//   node src/ping-pong.ts policy set|get|remove <sourceSelector> [--required ..] [--optional .. --threshold n] [--finality ..]
import { ComputeBudgetProgram, PublicKey, SystemProgram, TransactionInstruction } from '@solana/web3.js'
import { AbiCoder, hexlify } from 'ethers'

import { ChainFamily, SolanaChain, encodeExtraArgs } from '@chainlink/ccip-sdk'

import {
    PING_PONG_PROGRAM,
    ROUTER_SEND_LUT,
    SOLANA_ROUTER,
    args,
    ccipExplorer,
    conn,
    disc,
    evmAddressBytes,
    explorerTx,
    fetchLuts,
    le16,
    le64,
    loadKeypair,
    meta,
    option,
    pda,
    programData,
    receiverRegistry,
    resolveAccounts,
    routerConfig,
    routerEventAuthority,
    routerSendIxData,
    sendV0,
    simulateReturn,
    solanaRpc,
    u8,
    vecBytes,
    vecPk,
    quietLogger,
} from './lib/common.ts'
import { decodeCcvOut, formatFinality, parseFinality } from './lib/policy.ts'

const P = PING_PONG_PROGRAM
const PP = {
    config: pda([Buffer.from('config')], P),
    state: pda([Buffer.from('ping_pong_state')], P),
    signer: pda([Buffer.from('ccip_signer')], P),
    peer: (sel: bigint): PublicKey => pda([Buffer.from('peer'), le64(sel)], P),
    ccvConfig: (sel: bigint): PublicKey => pda([Buffer.from('ccv_config'), le64(sel)], P),
}

const payer = loadKeypair()
const me = payer.publicKey
const { positional, opts } = args()
const [cmd, arg1, arg2] = positional

const payload = (kind: 1 | 2, seq: bigint): Buffer =>
    Buffer.from(AbiCoder.defaultAbiCoder().encode(['uint8', 'uint64'], [kind, seq]).slice(2), 'hex')
const modeByte = (m: string): Buffer => u8(m === 'immediate' ? 1 : 0)

async function run(name: string, ixs: TransactionInstruction[]): Promise<void> {
    const r = await sendV0(payer, ixs)
    console.log(`${name}: ${explorerTx(r.signature)} (${r.computeUnits ?? '?'} CU)`)
}

interface PeerAccount {
    chainSelector: bigint
    address: Buffer
    acceptedSender: Buffer
    extraArgs: Buffer
    pending: bigint[]
}

async function readPeer(sel: bigint): Promise<PeerAccount> {
    const info = await conn.getAccountInfo(PP.peer(sel))
    if (info === null) throw new Error(`no peer for ${sel}`)
    const d = info.data
    let o = 9
    const chainSelector = d.readBigUInt64LE(o)
    o += 8
    const address = Buffer.from(d.subarray(o, o + 32))
    const acceptedSender = Buffer.from(d.subarray(o + 32, o + 64))
    o += 64
    const n = d.readUInt32LE(o)
    o += 4
    const extraArgs = Buffer.from(d.subarray(o, o + n))
    o += n
    const m = d.readUInt32LE(o)
    o += 4
    const pending = Array.from({ length: m }, (_, i) => d.readBigUInt64LE(o + 8 * i))
    return { chainSelector, address, acceptedSender, extraArgs, pending }
}

async function readState(): Promise<Record<string, string>> {
    const info = await conn.getAccountInfo(PP.state)
    if (info === null) throw new Error('not initialized')
    const d = info.data
    let o = 9
    const n = (): string => {
        const v = d.readBigUInt64LE(o)
        o += 8
        return v.toString()
    }
    const pingsSent = n()
    const pongsReceived = n()
    const pingsReceived = n()
    const pongsSent = n()
    const lastMessageId = `0x${d.subarray(o, o + 32).toString('hex')}`
    o += 32
    const lastKind = String(d[o])
    o += 1
    return {
        pingsSent,
        pongsReceived,
        pingsReceived,
        pongsSent,
        lastMessageId,
        lastKind,
        lastSequence: n(),
        lastSource: n(),
    }
}

/** Resolves the router's `ccip_send_v2` accounts for a message signed by the `ccip_signer` PDA. */
async function resolveSend(peer: PeerAccount, data: Buffer): Promise<Awaited<ReturnType<typeof resolveAccounts>>> {
    return resolveAccounts({
        program: SOLANA_ROUTER,
        caller: PP.signer,
        ixData: routerSendIxData({
            destChainSelector: peer.chainSelector,
            receiver: peer.address,
            data,
            extraArgs: peer.extraArgs,
        }),
        startAccounts: [routerConfig()],
        payer: me,
    })
}

function finality(): 'finalized' | 'safe' | number {
    const f = opts.get('finality') ?? 'finalized'
    return f === 'finalized' || f === 'safe' ? f : Number(f)
}

switch (cmd) {
    case 'init': {
        if ((await conn.getAccountInfo(PP.config)) === null) {
            await run('initialize', [
                new TransactionInstruction({
                    programId: P,
                    keys: [
                        meta(PP.config, true),
                        meta(PP.state, true),
                        meta(PP.signer),
                        meta(me, true, true),
                        meta(SystemProgram.programId),
                        meta(P),
                        meta(programData(P)),
                    ],
                    data: Buffer.concat([
                        disc('initialize'),
                        SOLANA_ROUTER.toBuffer(),
                        modeByte(opts.get('mode') ?? 'deferred'),
                    ]),
                }),
            ])
        }
        if ((await conn.getAccountInfo(receiverRegistry(P))) === null) {
            await run('register_with_router', [
                new TransactionInstruction({
                    programId: P,
                    keys: [
                        meta(PP.config),
                        meta(me, true, true),
                        meta(pda([Buffer.from('ccip_receiver_registration'), SOLANA_ROUTER.toBuffer()], P)),
                        meta(SOLANA_ROUTER),
                        meta(receiverRegistry(P), true),
                        meta(routerConfig()),
                        meta(routerEventAuthority()),
                        meta(SystemProgram.programId),
                    ],
                    data: disc('register_with_router'),
                }),
            ])
        }
        const balance = (await conn.getAccountInfo(PP.signer))?.lamports ?? 0
        if (balance < 100_000_000) {
            await run('fund ccip_signer', [
                SystemProgram.transfer({ fromPubkey: me, toPubkey: PP.signer, lamports: 200_000_000 }),
            ])
        }
        console.log({ ccipSigner: PP.signer.toBase58() })
        break
    }
    case 'peer': {
        const sel = BigInt(arg1 ?? '')
        const address = evmAddressBytes(opts.get('address') ?? '')
        const extraArgs = Buffer.from(
            encodeExtraArgs(
                {
                    gasLimit: BigInt(opts.get('gas') ?? '400000'),
                    finality: finality(),
                    ccvs: [],
                    ccvArgs: [],
                    executor: '',
                    executorArgs: '0x',
                    tokenReceiver: '',
                    tokenArgs: '0x',
                },
                ChainFamily.Solana,
            ).slice(2),
            'hex',
        )
        await run('set_peer', [
            new TransactionInstruction({
                programId: P,
                keys: [meta(PP.config), meta(PP.peer(sel), true), meta(me, true, true), meta(SystemProgram.programId)],
                data: Buffer.concat([
                    disc('set_peer'),
                    le64(sel),
                    address,
                    address,
                    vecBytes(extraArgs),
                    vecBytes(Buffer.from((opts.get('reply-metadata') ?? '0x').slice(2), 'hex')),
                ]),
            }),
        ])
        break
    }
    case 'mode': {
        await run('set_reply_mode', [
            new TransactionInstruction({
                programId: P,
                keys: [meta(PP.config, true), meta(me, false, true)],
                data: Buffer.concat([disc('set_reply_mode'), modeByte(arg1 ?? 'deferred')]),
            }),
        ])
        break
    }
    case 'ping':
    case 'reply': {
        const sel = BigInt(arg1 ?? '')
        const peer = await readPeer(sel)
        const state = await readState()
        const seq = cmd === 'ping' ? BigInt(state.pingsSent ?? '0') + 1n : peer.pending[0]
        if (seq === undefined) throw new Error('no pending reply')
        const resolved = await resolveSend(peer, payload(cmd === 'ping' ? 1 : 2, seq))
        const named =
            cmd === 'ping'
                ? [
                      meta(PP.config),
                      meta(PP.state, true),
                      meta(PP.peer(sel)),
                      meta(PP.signer, true),
                      meta(SOLANA_ROUTER),
                      meta(me, false, true),
                  ]
                : [
                      meta(PP.config),
                      meta(PP.state, true),
                      meta(PP.peer(sel), true),
                      meta(PP.signer, true),
                      meta(SOLANA_ROUTER),
                  ]
        const name = cmd === 'ping' ? 'ping' : 'send_pending_reply'
        const ix = new TransactionInstruction({
            programId: P,
            keys: [...named, ...resolved.accounts.map((a) => ({ ...a, isSigner: false }))],
            data: Buffer.concat([disc(name), le64(sel), vecBytes(resolved.metadata)]),
        })
        const luts = await fetchLuts([...resolved.lookupTables, ROUTER_SEND_LUT])
        const sent = await sendV0(
            payer,
            [
                ComputeBudgetProgram.setComputeUnitLimit({ units: 1_400_000 }),
                ComputeBudgetProgram.requestHeapFrame({ bytes: 256 * 1024 }),
                ix,
            ],
            luts,
        )
        const sol = await SolanaChain.fromUrl(solanaRpc(), { logger: quietLogger })
        const req = (await sol.getMessagesInTx(sent.signature))[0]
        console.log({
            sequence: seq.toString(),
            messageId: req?.message.messageId,
            tx: explorerTx(sent.signature),
            computeUnits: sent.computeUnits,
            routerAccounts: resolved.accounts.length,
            ...(req ? { explorer: ccipExplorer(req.message.messageId) } : {}),
        })
        break
    }
    case 'state': {
        console.log(await readState())
        for (const sel of positional.slice(1)) {
            const p = await readPeer(BigInt(sel))
            console.log({ peer: hexlify(p.address), pendingReplies: p.pending.map(String) })
        }
        break
    }
    case 'evm-args': {
        const accounts = [PP.config, PP.peer(BigInt(opts.get('source') ?? '16015286601757825753')), PP.state, PP.signer]
        let bitmap = (1n << 1n) | (1n << 2n) | (1n << 3n)
        if (opts.has('immediate')) {
            const peer = await readPeer(BigInt(opts.get('source') ?? '16015286601757825753'))
            const resolved = await resolveSend(peer, payload(2, 1n))
            accounts.push(SOLANA_ROUTER)
            for (const a of resolved.accounts) {
                accounts.push(a.pubkey)
                if (a.isWritable) bitmap |= 1n << BigInt(accounts.length - 1)
            }
            console.log({ immediateRouterAccounts: resolved.accounts.length, metadata: hexlify(resolved.metadata) })
        }
        const built = SolanaChain.buildMessageForDest({
            receiver: P.toBase58(),
            data: '0x',
            extraArgs: {
                gasLimit: BigInt(opts.get('gas') ?? '200000'),
                finality: finality(),
                accounts: accounts.map((a) => a.toBase58()),
                accountIsWritableBitmap: bitmap,
            },
        })
        console.log({
            receiver: hexlify(P.toBuffer()),
            acceptedSender: hexlify(PP.signer.toBuffer()),
            accounts: accounts.length,
            extraArgs: encodeExtraArgs(built.extraArgs, ChainFamily.EVM),
        })
        break
    }
    case 'policy': {
        const sel = BigInt(arg2 ?? '')
        if (arg1 === 'set') {
            const list = (v: string | undefined): PublicKey[] =>
                (v ?? '')
                    .split(',')
                    .filter((x) => x !== '')
                    .map((x) => (x === 'default' ? PublicKey.default : new PublicKey(x)))
            const ccvs =
                opts.has('required') || opts.has('optional')
                    ? Buffer.concat([
                          vecPk(list(opts.get('required'))),
                          vecPk(list(opts.get('optional'))),
                          u8(Number(opts.get('threshold') ?? '0')),
                      ])
                    : undefined
            const f = opts.get('finality')
            const fin = f === undefined ? undefined : parseFinality(f)
            await run('set_ccv_policy', [
                new TransactionInstruction({
                    programId: P,
                    keys: [
                        meta(PP.config),
                        meta(PP.ccvConfig(sel), true),
                        meta(me, true, true),
                        meta(SystemProgram.programId),
                    ],
                    data: Buffer.concat([
                        disc('set_ccv_policy'),
                        le64(sel),
                        option(ccvs),
                        option(fin && Buffer.concat([le16(fin.flags), le16(fin.blockDepth)])),
                    ]),
                }),
            ])
        } else if (arg1 === 'remove') {
            await run('remove_ccv_config', [
                new TransactionInstruction({
                    programId: P,
                    keys: [meta(PP.config), meta(PP.ccvConfig(sel), true), meta(me, true, true)],
                    data: Buffer.concat([disc('remove_ccv_config'), le64(sel)]),
                }),
            ])
        }
        const out = decodeCcvOut(
            await simulateReturn(
                me,
                [
                    new TransactionInstruction({
                        programId: P,
                        keys: [meta(PP.ccvConfig(sel))],
                        data: Buffer.concat([
                            disc('get_ccvs_and_finality_config'),
                            le64(sel),
                            vecBytes(Buffer.alloc(32)),
                        ]),
                    }),
                ],
                P,
            ),
        )
        console.log({
            requiredCcvs: out.requiredCcvs.map((k) => k.toBase58()),
            optionalCcvs: out.optionalCcvs.map((k) => k.toBase58()),
            optionalThreshold: out.optionalThreshold,
            allowedFinality: formatFinality(out.finality),
        })
        break
    }
    default:
        console.log('usage: ping-pong.ts init|peer|mode|ping|reply|state|evm-args|policy (see header)')
}
