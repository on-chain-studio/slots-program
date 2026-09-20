// Publishes the machine shelf from scripts/machines.json — written by the solver
// (`cargo run --release --example solve` in engine/), never by hand.
//
//   node scripts/set-machines.mjs
//
// After publishing, node scripts/verify-machines.mjs must say the chain matches exactly.

import fs from 'fs';
import { TransactionInstruction } from '@solana/web3.js';
import {
  admin, base, send, configPda, PROGRAM, SYSTEM,
  sg, rw, ro, header, u16, u32, u64,
} from './common.mjs';

const MODE = { lines: 0, hold: 1, gamble: 2 };
const GAMBLE_FAIR = 2 ** 31;
const LINES = [[1, 1, 1], [0, 0, 0], [2, 2, 2], [0, 1, 2], [2, 1, 0]];

const MACHINES = JSON.parse(
  fs.readFileSync(new URL('machines.json', import.meta.url), 'utf8'));

const vec = (items, enc) => Buffer.concat([u32(items.length), ...items.map(enc)]);
const winOf = (m) => Math.floor(GAMBLE_FAIR * (m.win_pct / 50));

const setMachineIx = (index, m) => new TransactionInstruction({
  programId: PROGRAM,
  keys: [sg(admin.publicKey), rw(configPda()), ro(SYSTEM)],
  data: Buffer.concat([
    header(5),
    Buffer.from([index, MODE[m.mode]]),
    u64(m.stake),
    Buffer.from([3, m.rounds, m.rungs]),                    // row_count, rounds, gamble_rungs
    u32(winOf(m)),
    Buffer.alloc(32),                                        // mint: all-zero = SOL
    vec(m.strips, (s) => vec(s, (x) => Buffer.from([x]))),
    vec(m.mults, (mult) => Buffer.concat([u16(mult), Buffer.from([0])])),
    vec(LINES, (l) => vec(l, (x) => Buffer.from([x]))),
  ]),
});

for (const [i, m] of MACHINES.entries()) {
  const sig = await send(base, [setMachineIx(i, m)], [admin]);
  console.log(`  ✅ ${i} ${m.name.padEnd(13)} ${m.mode.padEnd(6)} stake ${(m.stake / 1e9).toFixed(3)} SOL  tx ${sig.slice(0, 16)}…`);
}
const cfg = await base.getAccountInfo(configPda());
console.log(`shelf: ${Number(cfg.data.readBigUInt64LE(48))} machines published`);
