// Receiver program accounts and decoders.
import { PublicKey } from '@solana/web3.js'

import { RECEIVER_PROGRAM, TOKEN_PROGRAM, ata, evmAddressBytes, le64, pda } from './common.ts'

const P = RECEIVER_PROGRAM

export const RX = {
    config: pda([Buffer.from('config')], P),
    state: pda([Buffer.from('receiver_state')], P),
    tokenAdmin: pda([Buffer.from('receiver_token_admin')], P),
    rentPayer: pda([Buffer.from('rent_payer')], P),
    ccvConfig: (sel: bigint): PublicKey => pda([Buffer.from('ccv_config'), le64(sel)], P),
    approvedSender: (sel: bigint, sender: Uint8Array): PublicKey =>
        pda([Buffer.from('approved_ccip_sender'), le64(sel), sender], P),
}

/** Sender bytes as the OffRamp delivers them: a 32-byte pubkey, or an EVM address left-padded to 32. */
export function senderBytes(s: string): Buffer {
    return s.startsWith('0x') ? evmAddressBytes(s) : new PublicKey(s).toBuffer()
}

/** `FWD1` followed by the beneficiary wallet. */
export function forwardPayload(beneficiary: PublicKey): Buffer {
    return Buffer.concat([Buffer.from('FWD1', 'latin1'), beneficiary.toBuffer()])
}

/**
 * The receiver's accounts for `SVMExecutorArgsV1.accounts`, after the OffRamp's own three, and the
 * writable bitmap over that list. Forward mode appends the nine accounts the payout needs.
 */
export function receiverAccounts(
    sourceSelector: bigint,
    sender: Uint8Array,
    forward?: { mint: PublicKey; beneficiary: PublicKey; tokenProgram?: PublicKey },
): { accounts: PublicKey[]; writableBitmap: bigint } {
    const base = [RX.approvedSender(sourceSelector, sender), RX.config, RX.state]
    if (!forward) return { accounts: base, writableBitmap: 1n << 2n }
    const tp = forward.tokenProgram ?? TOKEN_PROGRAM
    const accounts = [
        ...base,
        RX.tokenAdmin,
        ata(RX.tokenAdmin, forward.mint, tp),
        forward.mint,
        tp,
        forward.beneficiary,
        ata(forward.beneficiary, forward.mint, tp),
        new PublicKey('ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL'),
        new PublicKey('11111111111111111111111111111111'),
        RX.rentPayer,
    ]
    // writable: receiver_state (2), token admin ATA (4), beneficiary ATA (8), rent payer (11)
    return { accounts, writableBitmap: (1n << 2n) | (1n << 4n) | (1n << 8n) | (1n << 11n) }
}

/** Decodes `ReceiverState` (Borsh, after the 8-byte account discriminator). */
export function decodeReceiverState(d: Buffer): Record<string, string | number> {
    let o = 8 + 2
    const u64 = (): bigint => {
        const v = d.readBigUInt64LE(o)
        o += 8
        return v
    }
    const bytes = (n: number): Buffer => {
        const v = Buffer.from(d.subarray(o, o + n))
        o += n
        return v
    }
    const vec = (): Buffer => bytes(u32())
    const u32 = (): number => {
        const v = d.readUInt32LE(o)
        o += 4
        return v
    }
    const receivedCount = u64()
    const mode = ['None', 'Data', 'TokensAndData', 'Forward'][d[o] ?? 0] ?? 'unknown'
    o += 1
    const messageId = bytes(32)
    const source = u64()
    const sender = vec()
    const dataLen = u32()
    const data = vec()
    const token = new PublicKey(bytes(32))
    const amount = u64()
    const beneficiary = new PublicKey(bytes(32))
    const forwarded = u64()
    const kept = u64()
    return {
        receivedCount: receivedCount.toString(),
        lastMode: mode,
        lastMessageId: `0x${messageId.toString('hex')}`,
        lastSourceChainSelector: source.toString(),
        lastSender: `0x${sender.toString('hex')}`,
        lastDataLen: dataLen,
        lastData: `0x${data.toString('hex')}`,
        lastToken: token.toBase58(),
        lastAmount: amount.toString(),
        lastBeneficiary: beneficiary.toBase58(),
        lastForwarded: forwarded.toString(),
        lastKept: kept.toString(),
    }
}
