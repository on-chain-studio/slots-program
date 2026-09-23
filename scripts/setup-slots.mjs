// Stands the program up against the vault. Ordered + idempotent — safe to re-run.
//
//   node scripts/setup-slots.mjs
//
// Steps: Initialize (config + house + analytics) → open the house ledger → float it (deposit to
// the admin ledger, settle admin → house) → fund the house PDA (spin rent + VRF) → delegate the
// house PDA, analytics PDA and house ledger to the TEE — the private rollup every spin lives on.

import { LAMPORTS_PER_SOL, SystemProgram } from '@solana/web3.js';
import {
  admin, base, send, info, ownerIs, awaitOwner, sleep, connect,
  pda, configPda, housePda, analyticsPda, ledgerPda, permPda, dpda,
  ro, rw, sg, header, u16, u32, u64,
  PROGRAM, VAULT, PERMISSION, DELEGATION, SYSTEM, ER_VALIDATOR,
  vaultDepositIx, settleIx, delegateOwnLedgerIx, decodeLedger,
} from './common.mjs';
import { TransactionInstruction } from '@solana/web3.js';
import { MAINNET } from './net.mjs';

const MACHINE_COUNT = 0;                          // fresh shelf; machines come from set-machines.mjs
const HOUSE_PDA_FUND = 0.1 * LAMPORTS_PER_SOL;    // on ["house"]: spin ephemeral rent + VRF fees
// On the house *ledger*: the payout float. Must cover the largest single win — Gold Rush at the
// 0.05 stake is x80 = 4 SOL — with margin. Devnet plays with a token amount.
const HOUSE_FLOAT = (MAINNET ? 5 : 1) * LAMPORTS_PER_SOL;
const HOUSE_SLOTS = 8;

const config = configPda(), house = housePda(), analytics = analyticsPda();

const initializeIx = () => new TransactionInstruction({
  programId: PROGRAM,
  keys: [sg(admin.publicKey), rw(config), rw(house),
         rw(analytics), rw(permPda(analytics)), ro(PERMISSION), ro(SYSTEM)],
  data: Buffer.concat([header(1), Buffer.from([MACHINE_COUNT])]),
});
const openLedgerIx = () => new TransactionInstruction({
  programId: PROGRAM,
  keys: [sg(admin.publicKey), ro(house), rw(ledgerPda(house)), rw(permPda(ledgerPda(house))),
         ro(PERMISSION), ro(VAULT), ro(SYSTEM)],
  data: Buffer.concat([header(8), Buffer.from([0]), u16(HOUSE_SLOTS)]),
});
const delegateLedgerIx = () => new TransactionInstruction({
  programId: PROGRAM,
  keys: [sg(admin.publicKey), rw(house),
         rw(dpda('buffer', ledgerPda(house), VAULT)), rw(dpda('delegation', ledgerPda(house), DELEGATION)),
         rw(dpda('delegation-metadata', ledgerPda(house), DELEGATION)), rw(ledgerPda(house)),
         ro(VAULT), ro(DELEGATION), ro(SYSTEM)],
  data: Buffer.concat([header(9), Buffer.from([0]), ER_VALIDATOR.toBuffer()]),
});
const delegatePdaIx = (seedStr, account) => new TransactionInstruction({
  programId: PROGRAM,
  keys: [sg(admin.publicKey), rw(account), ro(PROGRAM),
         rw(dpda('buffer', account, PROGRAM)), rw(dpda('delegation', account, DELEGATION)),
         rw(dpda('delegation-metadata', account, DELEGATION)), ro(DELEGATION), ro(SYSTEM)],
  data: Buffer.concat([header(2), u32(1), u32(seedStr.length), Buffer.from(seedStr),
                       ER_VALIDATOR.toBuffer()]),
});

const feesBefore = await base.getBalance(admin.publicKey);
console.log('program  ', PROGRAM.toBase58());
console.log('admin    ', admin.publicKey.toBase58());
console.log('validator', ER_VALIDATOR.toBase58(), '(TEE)\n');

// 1 ── create + configure, one transaction
console.log('1. create + configure');
const setupIxs = [];
if (await info(config) && await info(analytics)) console.log('   ⏭  config + analytics exist');
else { setupIxs.push(initializeIx()); console.log('   • initialize'); }
if (await info(ledgerPda(house))) console.log('   ⏭  house ledger exists');
else { setupIxs.push(openLedgerIx()); console.log('   • open house ledger'); }

const hb = (await info(house))?.lamports ?? 0;
if (await ownerIs(house, DELEGATION)) console.log('   ⏭  house PDA delegated — fund on the ER if short');
else if (hb >= HOUSE_PDA_FUND) console.log(`   ⏭  house PDA holds ${(hb / 1e9).toFixed(3)} SOL`);
else {
  setupIxs.push(SystemProgram.transfer({
    fromPubkey: admin.publicKey, toPubkey: house, lamports: HOUSE_PDA_FUND - hb }));
  console.log('   • fund the house PDA');
}
if (setupIxs.length) { await send(base, setupIxs, [admin]); console.log(`   ✅ ${setupIxs.length} instruction(s)`); }

// 2 ── the payout float, while the house ledger is still at home
console.log('2. house-ledger float');
const houseLedgerInfo = await info(ledgerPda(house));
const houseSol = houseLedgerInfo?.owner.equals(VAULT)
  ? Number(decodeLedger(houseLedgerInfo.data)?.sol ?? 0n) : null;
if (houseSol === null) {
  console.log('   ⏭  house ledger not at home — float it on the ER via the play script if short');
} else if (houseSol >= HOUSE_FLOAT) {
  console.log(`   ⏭  holds ${(houseSol / 1e9).toFixed(3)} SOL`);
} else {
  const short = HOUSE_FLOAT - houseSol;
  // Deposit to the admin's ledger and settle admin → house — the canonical path in.
  await send(base, [vaultDepositIx(admin.publicKey, short),
                    settleIx(admin.publicKey, house, short)], [admin]);
  console.log(`   ✅ floated ${(short / 1e9).toFixed(3)} SOL`);
}

// 3 ── delegate
console.log('3. delegate');
const targets = [
  ['house PDA', delegatePdaIx('house', house), house],
  ['analytics PDA', delegatePdaIx('analytics', analytics), analytics],
  ['house ledger', delegateLedgerIx(), ledgerPda(house)],
];
const delIxs = [];
for (const [name, ix, key] of targets) {
  if (await ownerIs(key, DELEGATION)) { console.log(`   ⏭  ${name} already delegated`); continue; }
  delIxs.push([name, ix, key]);
  console.log(`   • ${name}`);
}
if (delIxs.length) {
  await send(base, delIxs.map(([, ix]) => ix), [admin]);
  for (const [name, , key] of delIxs) {
    await awaitOwner(key, DELEGATION, name);
    console.log(`   ✅ ${name}`);
  }
}

const feesAfter = await base.getBalance(admin.publicKey);
console.log(`\ndone. this run cost ${((feesBefore - feesAfter) / 1e9).toFixed(6)} SOL (float and PDA funds included; reclaimable)`);
