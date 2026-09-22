// Shared plumbing for every slots script: ids, PDAs, instruction builders, decoders, and a
// send() that tolerates rollup blockhash quirks. One copy of each wire shape — the account
// orders here mirror the handlers in src/instructions/ and are the client's half of them.

import fs from 'fs';
import crypto from 'crypto';
import {
  Connection, Keypair, PublicKey, SystemProgram, Transaction,
  TransactionInstruction, sendAndConfirmTransaction,
} from '@solana/web3.js';
import { ADMIN_PATH, BASENET } from './net.mjs';

export const PROGRAM = new PublicKey('SLoTSdnmBH5KtNJjhEYw1MeWTKAfRnFfQTTcpgwRn2Q');
export const VAULT = new PublicKey('VAULTrDSUBZ8AXL2kGVYE8eKAn7tgWXRPAevNGUsyTV');
export const PERMISSION = new PublicKey('ACLseoPoyC3cBqoUtkbjZ4aDrkurZW86v19pXz2XQnp1');
export const DELEGATION = new PublicKey('DELeGGvXpWV2fqJUhqcF5ZSYMS4JTLjteaAMARRSaeSh');
export const MAGIC_PROGRAM = new PublicKey('Magic11111111111111111111111111111111111111');
export const MAGIC_CONTEXT = new PublicKey('MagicContext1111111111111111111111111111111');
export const EPHEMERAL_VAULT = new PublicKey('MagicVau1t999999999999999999999999999999999');
export const VRF_PROGRAM = new PublicKey('Vrf1RNUjXmQGjmQrQLvJHs9SNkvDJEsRVFPkfSQUwGz');
export const VRF_EPHEMERAL_QUEUE = new PublicKey('5hBR571xnXppuCPveTrctfTU7tJLSN94nq7kv7FRK5Tc');
export const SLOT_HASHES = new PublicKey('SysvarS1otHashes111111111111111111111111111');
export const SOL_MINT = new PublicKey('11111111111111111111111111111111');
export const SYSTEM = SystemProgram.programId;

/** The TEE's validator identity — the private rollup every spin lives on; same on both clusters. */
export const ER_VALIDATOR = new PublicKey('MTEWGuqxUpYZGFJQcp8tLN7x5v9BSeoFHYWQQ3n3xzo');
/** The public ER's identity — only named so a ledger left there can be recognised and reclaimed. */
export const PUBLIC_ER_VALIDATOR = new PublicKey('MAS1Dt9qreoRMQ14YQuhg8UTZMMzDdKhmkZMECCzk57');

export const admin = Keypair.fromSecretKey(new Uint8Array(JSON.parse(fs.readFileSync(ADMIN_PATH))));

export const pda = (seeds, prog = PROGRAM) => PublicKey.findProgramAddressSync(seeds, prog)[0];
export const configPda = () => pda([Buffer.from('config')]);
export const housePda = () => pda([Buffer.from('house')]);
export const analyticsPda = () => pda([Buffer.from('analytics')]);
export const identityPda = () => pda([Buffer.from('identity')]);
export const spinPda = (user) => pda([Buffer.from('spin'), user.toBuffer()]);
export const ledgerPda = (o) => pda([Buffer.from('ledger'), o.toBuffer()], VAULT);
export const reservePda = () => pda([Buffer.from('vault')], VAULT);
export const permPda = (a) => pda([Buffer.from('permission:'), a.toBuffer()], PERMISSION);
export const receiptPda = (consenter) => pda(
  [Buffer.from('receipt'), PROGRAM.toBuffer(), consenter.toBuffer()], VAULT);
export const vaultAuthorityPda = () => PublicKey.findProgramAddressSync([], VAULT)[0];
export const dpda = (tag, acct, prog) => pda([Buffer.from(tag), acct.toBuffer()], prog);

export const ro = (k) => ({ pubkey: k, isSigner: false, isWritable: false });
export const rw = (k) => ({ pubkey: k, isSigner: false, isWritable: true });
export const sg = (k) => ({ pubkey: k, isSigner: true, isWritable: true });
export const sgro = (k) => ({ pubkey: k, isSigner: true, isWritable: false });

export const u16 = (n) => { const b = Buffer.alloc(2); b.writeUInt16LE(n); return b; };
export const u32 = (n) => { const b = Buffer.alloc(4); b.writeUInt32LE(n); return b; };
export const u64 = (n) => { const b = Buffer.alloc(8); b.writeBigUInt64LE(BigInt(n)); return b; };
export const header = (v) => { const b = Buffer.alloc(8); b[0] = v; return b; };
export const anchorDisc = (n) =>
  crypto.createHash('sha256').update('global:' + n).digest().subarray(0, 8);

/** The public devnet RPC rate-limits hard. A 429 is a wait, not a failure. */
const fetchRetrying = async (url, opts) => {
  for (let i = 0; i < 8; i++) {
    const r = await fetch(url, opts);
    if (r.status !== 429) return r;
    await new Promise((res) => setTimeout(res, 1000 * (i + 1)));
  }
  return fetch(url, opts);
};

export const base = new Connection(BASENET, { commitment: 'confirmed', fetch: fetchRetrying });
export const connect = (url) => new Connection(url, { commitment: 'confirmed', fetch: fetchRetrying });

/**
 * Never preflight against a rollup (its simulate substitutes blockhashes), and on blockheight
 * expiry ask the chain what actually happened rather than trusting the deadline — a rollup
 * advances its own height and the transaction may have landed perfectly well.
 */
export const send = (conn, ixs, signers) => (async () => {
  const tx = new Transaction().add(...ixs);
  try {
    return await sendAndConfirmTransaction(conn, tx, signers,
      { commitment: 'confirmed', skipPreflight: conn !== base });
  } catch (e) {
    const sig = e?.signature ?? String(e).match(/Signature ([1-9A-HJ-NP-Za-km-z]{60,}) has expired/)?.[1];
    if (!sig || !String(e).includes('block height exceeded')) throw e;
    for (let i = 0; i < 20; i++) {
      const { value } = await conn.getSignatureStatuses([sig]);
      const status = value?.[0];
      if (status?.err) throw Object.assign(
        new Error(`${sig} failed: ${JSON.stringify(status.err)}`), { err: status.err, signature: sig });
      if (status?.confirmationStatus) return sig;
      await new Promise((r) => setTimeout(r, 1000));
    }
    throw e;
  }
})();

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
export const info = (k) => base.getAccountInfo(k);
export const ownerIs = async (k, p) => (await info(k))?.owner.equals(p) ?? false;
export const awaitOwner = async (k, want, label) => {
  for (let i = 0; i < 40; i++) {
    if (await ownerIs(k, want)) return;
    await sleep(1500);
  }
  throw new Error(`${label}: owner never became ${want.toBase58()}`);
};

// ── game instructions (the numbers are src/instruction.rs positions, pinned by tests) ──────

export const requestBetIx = (wallet, user, machineId) => new TransactionInstruction({
  programId: PROGRAM,
  keys: [
    sgro(wallet), ro(user), ro(configPda()), rw(housePda()), rw(receiptPda(wallet)),
    rw(EPHEMERAL_VAULT), ro(MAGIC_PROGRAM), ro(VAULT), rw(ledgerPda(housePda())),
    rw(MAGIC_CONTEXT),
  ],
  data: Buffer.concat([header(16), u64(machineId)]),
});

/** Ledger order matches the owner list the receipt was created with: [user, house]. */
export const betLedgers = (user) => [ledgerPda(user), ledgerPda(housePda())];
export const collectLedgers = betLedgers;

/** What ResolveBet (17) needs after the receipt and vault authority the vault prepends. */
export const resolveBetAccounts = (user) => [
  ro(configPda()), rw(housePda()), rw(spinPda(user)),
  rw(EPHEMERAL_VAULT), ro(MAGIC_PROGRAM), rw(analyticsPda()),
];

export const requestRevealIx = (user) => new TransactionInstruction({
  programId: PROGRAM,
  keys: [
    ro(user), rw(housePda()), rw(spinPda(user)),
    ro(identityPda()), rw(VRF_EPHEMERAL_QUEUE), ro(SLOT_HASHES),
    ro(SYSTEM), ro(VRF_PROGRAM),
  ],
  data: header(18),
});

export const holdIx = (signer, user, mask) => new TransactionInstruction({
  programId: PROGRAM,
  keys: [sgro(signer), rw(spinPda(user))],
  data: Buffer.concat([header(20), Buffer.from([mask])]),
});

export const gambleIx = (signer, user) => new TransactionInstruction({
  programId: PROGRAM,
  keys: [sgro(signer), rw(spinPda(user))],
  data: header(21),
});

export const requestCollectIx = (user, wallet = user) => new TransactionInstruction({
  programId: PROGRAM,
  keys: [
    ro(user), rw(housePda()), rw(spinPda(user)), rw(receiptPda(wallet)),
    rw(EPHEMERAL_VAULT), ro(MAGIC_PROGRAM), ro(VAULT),
    sgro(wallet), rw(ledgerPda(housePda())), rw(MAGIC_CONTEXT),
  ],
  data: header(22),
});

/** What ResolveCollect (23) needs. A loss arrives on an empty receipt, exactly as a win does. */
export const resolveCollectAccounts = (user) => [
  rw(housePda()), rw(spinPda(user)), rw(EPHEMERAL_VAULT), ro(MAGIC_PROGRAM),
  rw(analyticsPda()),
];

/** settle_receipt — top-level vault: moves the balances, then calls back into the game. */
export const settleReceiptIx = (consenter, consenterSigns, ledgers, extra) =>
  new TransactionInstruction({
    programId: VAULT,
    keys: [
      rw(receiptPda(consenter)), ro(housePda()),
      { pubkey: consenter, isSigner: consenterSigns, isWritable: false },
      ro(PROGRAM), ro(vaultAuthorityPda()),
      rw(EPHEMERAL_VAULT), ro(MAGIC_PROGRAM), rw(MAGIC_CONTEXT),
      ...ledgers.map(rw),
      ...extra,
    ],
    data: anchorDisc('settle_receipt'),
  });

// ── vault instructions the player calls directly ───────────────────────────────────────────

export const vaultDepositIx = (owner, lamports) => new TransactionInstruction({
  programId: VAULT,
  keys: [
    sg(owner), rw(ledgerPda(owner)), rw(permPda(ledgerPda(owner))), ro(PERMISSION),
    rw(reservePda()), ro(SYSTEM), ro(SYSTEM), ro(new PublicKey('TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA')),
    ro(SYSTEM),
  ],
  data: Buffer.concat([anchorDisc('deposit'), SOL_MINT.toBuffer(), u64(lamports),
                       Buffer.from([0]), Buffer.from([0])]),
});

export const settleIx = (srcOwner, dstOwner, lamports) => new TransactionInstruction({
  programId: VAULT,
  keys: [rw(ledgerPda(srcOwner)), rw(ledgerPda(dstOwner)), sgro(srcOwner), ro(dstOwner)],
  data: Buffer.concat([anchorDisc('settle'), SOL_MINT.toBuffer(), u64(lamports)]),
});

export const delegateOwnLedgerIx = (owner, validator) => {
  const ledger = ledgerPda(owner);
  return new TransactionInstruction({
    programId: VAULT,
    keys: [
      sg(owner), sgro(owner),
      rw(dpda('buffer', ledger, VAULT)), rw(dpda('delegation', ledger, DELEGATION)),
      rw(dpda('delegation-metadata', ledger, DELEGATION)), rw(ledger),
      ro(VAULT), ro(DELEGATION), ro(SYSTEM),
    ],
    data: Buffer.concat([anchorDisc('delegate_ledger'), Buffer.from([1]), validator.toBuffer()]),
  });
};

export const undelegateOwnLedgerIx = (owner) => new TransactionInstruction({
  programId: VAULT,
  keys: [sg(owner), sgro(owner), rw(ledgerPda(owner)), ro(MAGIC_PROGRAM), rw(MAGIC_CONTEXT),
         rw(EPHEMERAL_VAULT)],
  data: anchorDisc('undelegate'),
});

// ── decoders ───────────────────────────────────────────────────────────────────────────────

const LEDGER_HEADER = 116;
const LEDGER_SOL_AT = LEDGER_HEADER + 32;

export function decodeLedger(data) {
  if (!data || data.length < LEDGER_HEADER + 40) return null;
  return {
    owner: new PublicKey(data.subarray(8, 40)),
    authorized: new PublicKey(data.subarray(80, 112)),
    sol: data.readBigUInt64LE(LEDGER_SOL_AT),
  };
}

export const SPIN_STATUS = ['bought', 'requested', 'rolled'];

/** `Spin` — offsets pinned by tests/layout.rs (160 bytes + terms). */
export function decodeSpin(data) {
  if (!data || data.length < 160) return null;
  return {
    user: new PublicKey(data.subarray(16, 48)),
    consenter: new PublicKey(data.subarray(48, 80)),
    machineId: Number(data.readBigUInt64LE(80)),
    status: SPIN_STATUS[Number(data.readBigUInt64LE(88))] ?? '?',
    round: Number(data.readBigUInt64LE(96)),
    hold: Number(data.readBigUInt64LE(104)),
    pending: data.readBigUInt64LE(112),
    stops: [...data.subarray(120, 125)],
    seed: data.subarray(128, 160),
  };
}
