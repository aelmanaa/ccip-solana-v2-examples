// Receiver program admin and reads.
//
//   node src/receiver.ts init [--forward-bps 250]          initialize, register with the router, fund rent payer
//   node src/receiver.ts approve <sourceSelector> <sender>   sender: 0x EVM address or base58 Solana address
//   node src/receiver.ts unapprove <sourceSelector> <sender>
//   node src/receiver.ts policy set <sourceSelector> [--sender <s>] [--required a,b] [--optional c,d --threshold n]
//                                     [--finality finalized|safe|depth:N|safe+depth:N]
//   node src/receiver.ts policy get <sourceSelector> [--sender <s>]   what get_ccvs_and_finality_config returns
//   node src/receiver.ts policy remove <sourceSelector> [--sender <s>]
//   node src/receiver.ts state                              last received message
//   node src/receiver.ts withdraw <mint> <amount> <destinationTokenAccount>
//
// `--required default` (or the zero address) is the "lane default CCVs" marker.
import { PublicKey, SystemProgram, TransactionInstruction } from '@solana/web3.js'

import {
    RECEIVER_PROGRAM,
    SOLANA_ROUTER,
    TOKEN_2022_PROGRAM,
    args,
    ata,
    conn,
    disc,
    explorerTx,
    le16,
    le64,
    loadKeypair,
    meta,
    option,
    pda,
    programData,
    receiverRegistry,
    routerConfig,
    routerEventAuthority,
    sendV0,
    simulateReturn,
    u8,
    vecBytes,
    vecPk,
} from './lib/common.ts'
import { type CcvPolicy, decodeCcvOut, formatFinality, parseFinality } from './lib/policy.ts'
import { RX, decodeReceiverState, senderBytes } from './lib/receiver.ts'

const P = RECEIVER_PROGRAM

const payer = loadKeypair()
const me = payer.publicKey
const { positional, opts } = args()
const [cmd, sub, third] = positional

async function run(name: string, ixs: TransactionInstruction[]): Promise<void> {
    const r = await sendV0(payer, ixs)
    console.log(`${name}: ${explorerTx(r.signature)}`)
}

const pkList = (v: string | undefined): PublicKey[] =>
    (v ?? '')
        .split(',')
        .filter((x) => x !== '')
        .map((x) => (x === 'default' ? PublicKey.default : new PublicKey(x)))

switch (cmd) {
    case 'init': {
        if ((await conn.getAccountInfo(RX.config)) === null) {
            await run('initialize', [
                new TransactionInstruction({
                    programId: P,
                    keys: [
                        meta(RX.config, true),
                        meta(RX.state, true),
                        meta(me, true, true),
                        meta(SystemProgram.programId),
                        meta(P),
                        meta(programData(P)),
                    ],
                    data: Buffer.concat([
                        disc('initialize'),
                        SOLANA_ROUTER.toBuffer(),
                        le16(Number(opts.get('forward-bps') ?? '250')),
                    ]),
                }),
            ])
        }
        const registry = receiverRegistry(P)
        if ((await conn.getAccountInfo(registry)) === null) {
            await run('register_with_router', [
                new TransactionInstruction({
                    programId: P,
                    keys: [
                        meta(RX.config),
                        meta(me, true, true),
                        meta(pda([Buffer.from('ccip_receiver_registration'), SOLANA_ROUTER.toBuffer()], P)),
                        meta(SOLANA_ROUTER),
                        meta(registry, true),
                        meta(routerConfig()),
                        meta(routerEventAuthority()),
                        meta(SystemProgram.programId),
                    ],
                    data: disc('register_with_router'),
                }),
            ])
        }
        const rent = (await conn.getAccountInfo(RX.rentPayer))?.lamports ?? 0
        if (rent < 20_000_000) {
            await run('fund rent_payer', [
                SystemProgram.transfer({ fromPubkey: me, toPubkey: RX.rentPayer, lamports: 50_000_000 }),
            ])
        }
        console.log({ tokenAdmin: RX.tokenAdmin.toBase58(), rentPayer: RX.rentPayer.toBase58() })
        break
    }
    case 'approve':
    case 'unapprove': {
        const sel = BigInt(sub ?? '')
        const sender = senderBytes(third ?? '')
        const keys =
            cmd === 'approve'
                ? [
                      meta(RX.config),
                      meta(RX.approvedSender(sel, sender), true),
                      meta(me, true, true),
                      meta(SystemProgram.programId),
                  ]
                : [meta(RX.config), meta(RX.approvedSender(sel, sender), true), meta(me, true, true)]
        await run(cmd, [
            new TransactionInstruction({
                programId: P,
                keys,
                data: Buffer.concat([disc(`${cmd}_sender`), le64(sel), vecBytes(sender)]),
            }),
        ])
        break
    }
    case 'policy': {
        const sel = BigInt(third ?? '')
        const senderOpt = opts.get('sender')
        const sender = senderOpt === undefined ? undefined : senderBytes(senderOpt)
        if (sub === 'set') {
            const hasCcvs = opts.has('required') || opts.has('optional')
            const ccvs = hasCcvs
                ? Buffer.concat([
                      vecPk(pkList(opts.get('required'))),
                      vecPk(pkList(opts.get('optional'))),
                      u8(Number(opts.get('threshold') ?? '0')),
                  ])
                : undefined
            const fin = opts.get('finality')
            const finality = fin === undefined ? undefined : parseFinality(fin)
            await run('set_ccv_policy', [
                new TransactionInstruction({
                    programId: P,
                    keys: [
                        meta(RX.config),
                        meta(RX.ccvConfig(sel), true),
                        meta(me, true, true),
                        meta(SystemProgram.programId),
                    ],
                    data: Buffer.concat([
                        disc('set_ccv_policy'),
                        le64(sel),
                        option(sender && vecBytes(sender)),
                        option(ccvs),
                        option(finality && Buffer.concat([le16(finality.flags), le16(finality.blockDepth)])),
                    ]),
                }),
            ])
        } else if (sub === 'remove') {
            await run(
                sender ? 'remove_sender_ccv_policy' : 'remove_ccv_config',
                sender
                    ? [
                          new TransactionInstruction({
                              programId: P,
                              keys: [meta(RX.config), meta(RX.ccvConfig(sel), true), meta(me, false, true)],
                              data: Buffer.concat([disc('remove_sender_ccv_policy'), le64(sel), vecBytes(sender)]),
                          }),
                      ]
                    : [
                          new TransactionInstruction({
                              programId: P,
                              keys: [meta(RX.config), meta(RX.ccvConfig(sel), true), meta(me, true, true)],
                              data: Buffer.concat([disc('remove_ccv_config'), le64(sel)]),
                          }),
                      ],
            )
        }
        // `get` (and after every change): ask the program exactly what the OffRamp would get back.
        const out = await simulateReturn(
            me,
            [
                new TransactionInstruction({
                    programId: P,
                    keys: [meta(RX.ccvConfig(sel))],
                    data: Buffer.concat([
                        disc('get_ccvs_and_finality_config'),
                        le64(sel),
                        vecBytes(sender ?? Buffer.alloc(32)),
                    ]),
                }),
            ],
            P,
        )
        const policy: CcvPolicy = decodeCcvOut(out)
        console.log({
            sourceChainSelector: sel.toString(),
            sender: senderOpt ?? '(any)',
            requiredCcvs: policy.requiredCcvs.map((k) => (k.equals(PublicKey.default) ? 'default' : k.toBase58())),
            optionalCcvs: policy.optionalCcvs.map((k) => k.toBase58()),
            optionalThreshold: policy.optionalThreshold,
            allowedFinality: formatFinality(policy.finality),
        })
        break
    }
    case 'state': {
        const info = await conn.getAccountInfo(RX.state)
        if (info === null) throw new Error('receiver not initialized')
        console.log(decodeReceiverState(info.data))
        break
    }
    case 'withdraw': {
        const mint = new PublicKey(sub ?? '')
        const mintInfo = await conn.getAccountInfo(mint)
        if (mintInfo === null) throw new Error('mint not found')
        const tokenProgram = mintInfo.owner
        const from = ata(RX.tokenAdmin, mint, tokenProgram)
        await run('withdraw_tokens', [
            new TransactionInstruction({
                programId: P,
                keys: [
                    meta(RX.config),
                    meta(from, true),
                    meta(new PublicKey(positional[3] ?? ''), true),
                    meta(mint),
                    meta(tokenProgram),
                    meta(RX.tokenAdmin),
                    meta(me, false, true),
                ],
                data: Buffer.concat([disc('withdraw_tokens'), le64(BigInt(third ?? '0'))]),
            }),
        ])
        console.log({ tokenProgram: tokenProgram.equals(TOKEN_2022_PROGRAM) ? 'Token-2022' : 'SPL Token' })
        break
    }
    default:
        console.log('usage: receiver.ts init|approve|unapprove|policy set|get|remove|state|withdraw (see header)')
}
