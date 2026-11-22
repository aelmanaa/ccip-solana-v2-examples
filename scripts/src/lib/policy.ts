// CCV and finality policy encoding shared by the receiver and ping-pong scripts.
import { PublicKey } from '@solana/web3.js'

/** `FinalityConfig { flags: u16, block_depth: u16 }`. Flag bit 0 accepts `safe`. */
export interface Finality {
    flags: number
    blockDepth: number
}

export interface CcvPolicy {
    requiredCcvs: PublicKey[]
    optionalCcvs: PublicKey[]
    optionalThreshold: number
    finality: Finality
}

/**
 * Parses an allowed-finality spec:
 * - `finalized`: finalized messages only (`{0, 0}`);
 * - `safe`: also messages sent with `safe` finality (`{1, 0}`);
 * - `depth:N`: also messages that requested N or more block confirmations (`{0, N}`);
 * - `safe+depth:N`: both.
 */
export function parseFinality(spec: string): Finality {
    if (spec === 'finalized') return { flags: 0, blockDepth: 0 }
    let flags = 0
    let blockDepth = 0
    for (const part of spec.split('+')) {
        if (part === 'safe') flags |= 1
        else if (part.startsWith('depth:')) blockDepth = Number(part.slice(6))
        else throw new Error(`bad finality ${spec}: use finalized | safe | depth:N | safe+depth:N`)
    }
    if (!Number.isInteger(blockDepth) || blockDepth < 0 || blockDepth > 65535) throw new Error(`bad depth in ${spec}`)
    return { flags, blockDepth }
}

export function formatFinality(f: Finality): string {
    const parts = [...(f.flags & 1 ? ['safe'] : []), ...(f.blockDepth > 0 ? [`depth:${f.blockDepth}`] : [])]
    return parts.length ? `finalized, ${parts.join(', ')}` : 'finalized only'
}

/** Decodes `GetReceiverCcvsOut` (Borsh). */
export function decodeCcvOut(b: Buffer): CcvPolicy {
    if (b.length === 0)
        return { requiredCcvs: [], optionalCcvs: [], optionalThreshold: 0, finality: { flags: 0, blockDepth: 0 } }
    let o = 0
    const keys = (): PublicKey[] => {
        const n = b.readUInt32LE(o)
        o += 4
        return Array.from({ length: n }, () => {
            const k = new PublicKey(b.subarray(o, o + 32))
            o += 32
            return k
        })
    }
    const requiredCcvs = keys()
    const optionalCcvs = keys()
    const optionalThreshold = b[o] ?? 0
    o += 1
    return {
        requiredCcvs,
        optionalCcvs,
        optionalThreshold,
        finality: { flags: b.readUInt16LE(o), blockDepth: b.readUInt16LE(o + 2) },
    }
}
