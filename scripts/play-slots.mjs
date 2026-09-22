// Plays a real spin on devnet, end to end, in the TEE.
//
//   node scripts/play-slots.mjs [machine]     0 = lines (default), 1 = hold, 2 = gamble
//   node scripts/play-slots.mjs 2 --close     undelegate + withdraw the player ledger after
//
// Plays as the admin key against a ledger that persists between runs: deposit only the
// shortfall, delegate only if not already delegated. `--close` is the reclaim path.

import {
  admin, base, connect, send, sleep, info, ownerIs,
  housePda, spinPda, ledgerPda, decodeLedger, decodeSpin,
  requestBetIx, betLedgers, resolveBetAccounts, settleReceiptIx,
  requestRevealIx, holdIx, gambleIx, requestCollectIx, collectLedgers, resolveCollectAccounts,
  vaultDepositIx, delegateOwnLedgerIx, undelegateOwnLedgerIx,
  DELEGATION, VAULT, ER_VALIDATOR, dpda,
} from './common.mjs';
import { ROUTER } from './net.mjs';
import { teeEndpoint, endpointHolding } from './tee-auth.mjs';
import { TransactionInstruction, PublicKey } from '@solana/web3.js';

const MACHINE = Number(process.argv[2]) || 0;
const CLOSE = process.argv.includes('--close');
const STAKE = 5_000_000;
const BUDGET = STAKE * 4;   // enough for a spin with retries; deposits top up only the shortfall

const player = admin;
const er = connect(await teeEndpoint(player));
const user = player.publicKey;

const ok = (m) => console.log(`  ✅ ${m}`);
const readSpin = async () => {
  const i = await er.getAccountInfo(spinPda(user));
  return i && decodeSpin(i.data);
};
const readLedger = async (conn, owner) => {
  const i = await conn.getAccountInfo(ledgerPda(owner));
  return i && decodeLedger(i.data);
};
const awaitStatus = async (want, label) => {
  for (let i = 0; i < 60; i++) {
    const s = await readSpin();
    if (s?.status === want) return s;
    await sleep(750);
  }
  throw new Error(`${label}: spin never reached ${want} (now ${(await readSpin())?.status})`);
};

const feesBefore = await base.getBalance(user);
console.log('machine', MACHINE, ' player', user.toBase58(), '\n');

// 1 ── the player ledger: funded and on the rollup
console.log('1. player ledger');
{
  let onEr = await ownerIs(ledgerPda(user), DELEGATION);
  // Anyone's game may have delegated this ledger, to any validator. The router says which;
  // a ledger held elsewhere is brought home from there before it can be delegated to ours.
  if (onEr) {
    const record = await info(dpda('delegation', ledgerPda(user), DELEGATION));
    const holder = record ? new PublicKey(record.data.subarray(8, 40)) : null;
    if (holder && !holder.equals(ER_VALIDATOR)) {
      const there = await endpointHolding(ledgerPda(user), player, ROUTER);
      if (!there) throw new Error('ledger delegated, but the router cannot say where');
      await send(connect(there), [undelegateOwnLedgerIx(user)], [player]);
      for (let i = 0; i < 40 && onEr; i++) {
        if (await ownerIs(ledgerPda(user), VAULT)) onEr = false; else await sleep(1000);
      }
      if (onEr) throw new Error('reclaim from the other validator never landed');
      ok(`reclaimed the ledger from ${holder.toBase58().slice(0, 8)}…`);
    }
  }
  const led = await readLedger(onEr ? er : base, user);
  const have = Number(led?.sol ?? 0n);
  const short = Math.max(0, BUDGET - have);
  if (short && onEr) {
    await send(er, [undelegateOwnLedgerIx(user)], [player]);
    for (let i = 0; i < 40 && onEr; i++) {
      if (await ownerIs(ledgerPda(user), VAULT)) onEr = false; else await sleep(1000);
    }
    if (onEr) throw new Error('undelegation never landed');
    ok('undelegated to top up');
  }
  const ixs = [];
  if (short) ixs.push(vaultDepositIx(user, short));
  if (!onEr) ixs.push(delegateOwnLedgerIx(user, ER_VALIDATOR));
  if (ixs.length) await send(base, ixs, [player]);
  if (short) ok(`deposited ${(short / 1e9).toFixed(4)} SOL (had ${(have / 1e9).toFixed(4)})`);
  for (let i = 0; i < 40; i++) {
    if (await er.getAccountInfo(ledgerPda(user))) break;
    await sleep(1000);
    if (i === 39) throw new Error('ledger never appeared on the rollup');
  }
  const l = await readLedger(er, user);
  ok(`live on the rollup with ${(Number(l.sol) / 1e9).toFixed(4)} SOL`);
}

const before = await readLedger(er, user);

// 2 ── bet: record the stake, settle it, spin created by the callback
console.log('2. bet');
{
  // A leftover spin from an interrupted run: finish it first — collect is the only exit.
  const leftover = await readSpin();
  if (leftover) {
    if (leftover.status !== 'rolled') throw new Error(`a ${leftover.status} spin exists — reveal or CloseSpin first`);
    await send(er, [
      requestCollectIx(user),
      settleReceiptIx(user, true, collectLedgers(user), resolveCollectAccounts(user)),
    ], [player]);
    for (let i = 0; i < 40 && await readSpin(); i++) await sleep(750);
    ok('collected a leftover spin from a previous run');
  }
  await send(er, [
    requestBetIx(user, user, MACHINE),
    settleReceiptIx(user, true, betLedgers(user), resolveBetAccounts(user)),
  ], [player]);
  const s = await awaitStatus('bought', 'bet');
  ok(`stake settled, spin created (machine ${s.machineId})`);
}

// 3 ── reveal round by round, decide, collect
const reveal = async (label) => {
  await send(er, [requestRevealIx(user)], [player]);
  const s = await awaitStatus('rolled', label);
  ok(`${label}: seed ${Buffer.from(s.seed).toString('hex').slice(0, 16)}…  (round ${s.round})`);
  return s;
};

console.log('3. play');
await reveal('spin');

if (MACHINE === 1) {
  // hold: keep reel 0 through two respins — grids applied on chain, stops visible after each
  for (const round of [1, 2]) {
    await send(er, [holdIx(user, user, 0b001)], [player]);
    const s = await awaitStatus('bought', 'hold');
    ok(`held reel 0 → round ${s.round}, stops so far [${s.stops.slice(0, 3)}]`);
    await reveal(`respin ${round}`);
  }
} else if (MACHINE === 2) {
  // gamble: climb one rung if the base spin won anything; a losing base refuses the climb
  try {
    await send(er, [gambleIx(user, user)], [player]);
    const s = await awaitStatus('bought', 'gamble');
    ok(`base win ${(Number(s.pending) / 1e9).toFixed(4)} SOL riding → rung ${s.round}`);
    await reveal('flip');
  } catch (e) {
    if (!/Custom":9\b|0x9\b/.test(String(e))) throw e;   // NothingToCollect: base spin lost
    ok('base spin lost — nothing to gamble, collecting the loss');
  }
}

console.log('4. collect');
{
  await send(er, [
    requestCollectIx(user),
    settleReceiptIx(user, true, collectLedgers(user), resolveCollectAccounts(user)),
  ], [player]);
  for (let i = 0; i < 40; i++) {
    if (!(await readSpin())) break;
    await sleep(750);
    if (i === 39) throw new Error('spin still exists after collect');
  }
  const after = await readLedger(er, user);
  const delta = Number(after.sol - before.sol);
  ok(`spin closed. ledger ${delta >= 0 ? '+' : ''}${(delta / 1e9).toFixed(4)} SOL this spin`);
}

if (CLOSE) {
  console.log('5. reclaim');
  await send(er, [undelegateOwnLedgerIx(user)], [player]);
  for (let i = 0; i < 40; i++) {
    if (await ownerIs(ledgerPda(user), VAULT)) break;
    await sleep(1000);
  }
  const led = await readLedger(base, user);
  const { TransactionInstruction: TI } = await import('@solana/web3.js');
  const { anchorDisc, reservePda, permPda, sg, rw, ro, u64, SOL_MINT, PERMISSION, SYSTEM, TOKEN } =
    await import('./common.mjs').then((m) => ({ ...m, TOKEN: new PublicKey('TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA') }));
  await send(base, [new TI({
    programId: VAULT,
    keys: [sg(user), rw(ledgerPda(user)), rw(reservePda()),
           ro(SYSTEM), ro(SYSTEM), ro(TOKEN), ro(SYSTEM)],
    data: Buffer.concat([anchorDisc('withdraw'), SOL_MINT.toBuffer(), u64(led.sol)]),
  })], [player]);
  ok(`withdrew ${(Number(led.sol) / 1e9).toFixed(4)} SOL back to the wallet`);
}

const feesAfter = await base.getBalance(user);
console.log(`\nthis run cost ${((feesBefore - feesAfter) / 1e9).toFixed(6)} SOL from the wallet (deposits included; ledger persists)`);
