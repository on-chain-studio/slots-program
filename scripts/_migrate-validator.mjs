// Moves the house PDA, the analytics PDA and the house ledger off whichever validator holds them
// and back home, so setup-slots.mjs can delegate them to the TEE. Idempotent: an account already
// home is skipped, one already on the TEE is left alone.
//
//   node scripts/_migrate-validator.mjs && node scripts/setup-slots.mjs
//
// The undelegations run *on the holding validator* — the router names it — because a commit +
// undelegate is a rollup-side instruction. Player ledgers are not touched: the app and the play
// script reclaim those the same way, one at a time, when their owner next plays.

import { PublicKey, TransactionInstruction } from '@solana/web3.js';
import {
  admin, base, send, info, ownerIs, awaitOwner, sleep, connect,
  housePda, analyticsPda, ledgerPda, dpda, ro, rw, sg, header,
  PROGRAM, VAULT, DELEGATION, MAGIC_PROGRAM, MAGIC_CONTEXT, EPHEMERAL_VAULT, ER_VALIDATOR,
} from './common.mjs';
import { ROUTER } from './net.mjs';
import { endpointHolding } from './tee-auth.mjs';

const house = housePda(), analytics = analyticsPda(), houseLedger = ledgerPda(house);

// RequestUndelegation (4): commit + undelegate one of the game's own PDAs.
const undelegatePdaIx = (pda) => new TransactionInstruction({
  programId: PROGRAM,
  keys: [sg(admin.publicKey), rw(pda), rw(MAGIC_CONTEXT), ro(MAGIC_PROGRAM), rw(EPHEMERAL_VAULT)],
  data: header(4),
});
// UndelegateTreasury (10): the game CPI-signs the house seeds as the ledger's owner.
const undelegateLedgerIx = () => new TransactionInstruction({
  programId: PROGRAM,
  keys: [sg(admin.publicKey), ro(house), rw(houseLedger), ro(VAULT), ro(MAGIC_PROGRAM),
         rw(MAGIC_CONTEXT), rw(EPHEMERAL_VAULT)],
  data: Buffer.concat([header(10), Buffer.from([0])]),
});

const holderOf = async (account) => {
  const record = await info(dpda('delegation', account, DELEGATION));
  return record ? new PublicKey(record.data.subarray(8, 40)) : null;
};

const feesBefore = await base.getBalance(admin.publicKey);
console.log('target validator', ER_VALIDATOR.toBase58(), '(TEE)\n');

const targets = [
  ['house PDA', house, PROGRAM, undelegatePdaIx(house)],
  ['analytics PDA', analytics, PROGRAM, undelegatePdaIx(analytics)],
  ['house ledger', houseLedger, VAULT, undelegateLedgerIx()],
];

for (const [name, account, homeOwner, ix] of targets) {
  if (!(await ownerIs(account, DELEGATION))) { console.log(`⏭  ${name}: already home`); continue; }
  const holder = await holderOf(account);
  if (holder?.equals(ER_VALIDATOR)) { console.log(`⏭  ${name}: already on the TEE`); continue; }
  const there = await endpointHolding(account, admin, ROUTER);
  if (!there) throw new Error(`${name}: delegated to ${holder?.toBase58()}, but the router cannot say where`);
  console.log(`• ${name}: held by ${holder?.toBase58().slice(0, 8)}… at ${there.split('?')[0]}`);
  await send(connect(there), [ix], [admin]);
  await awaitOwner(account, homeOwner, name);
  console.log(`   ✅ ${name} home`);
}

const feesAfter = await base.getBalance(admin.publicKey);
console.log(`\ndone. this run cost ${((feesBefore - feesAfter) / 1e9).toFixed(6)} SOL. now: node scripts/setup-slots.mjs`);
