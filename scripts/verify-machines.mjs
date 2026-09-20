// Decodes every machine off the chain and compares it, byte-relevant field by field, to
// scripts/machines.json. The only thing that proves a publish landed — never skip it.
//
//   node scripts/verify-machines.mjs

import fs from 'fs';
import { base, configPda } from './common.mjs';

const MODE = { lines: 0, hold: 1, gamble: 2 };
const GAMBLE_FAIR = 2 ** 31;
const MACHINE_BYTES = 344;
const HEADER = 56;
const MAX_REELS = 5, MAX_STRIP = 32, MAX_SYMBOLS = 12, MAX_LINES = 16;
const LINES = [[1, 1, 1], [0, 0, 0], [2, 2, 2], [0, 1, 2], [2, 1, 0]];

const WANT = JSON.parse(fs.readFileSync(new URL('machines.json', import.meta.url), 'utf8'));

const info = await base.getAccountInfo(configPda());
if (!info) throw new Error('no config on chain');
const count = Number(info.data.readBigUInt64LE(48));

let bad = 0;
const check = (name, cond, got, want) => {
  if (!cond) { bad++; console.log(`  ❌ ${name}: chain ${got} vs sheet ${want}`); }
};

for (let i = 0; i < Math.max(count, WANT.length); i++) {
  const w = WANT[i];
  if (!w) { bad++; console.log(`  ❌ machine ${i} on chain but not in machines.json`); continue; }
  const d = info.data.subarray(HEADER + i * MACHINE_BYTES, HEADER + (i + 1) * MACHINE_BYTES);
  console.log(`machine ${i} — ${w.name}`);
  check('stake', d.readBigUInt64LE(0) === BigInt(w.stake), d.readBigUInt64LE(0), w.stake);
  check('mode', d[8] === MODE[w.mode], d[8], MODE[w.mode]);
  check('reels', d[9] === w.strips.length, d[9], w.strips.length);
  check('strip_len', d[10] === w.strips[0].length, d[10], w.strips[0].length);
  check('rows', d[11] === 3, d[11], 3);
  check('symbols', d[12] === w.mults.length, d[12], w.mults.length);
  check('lines', d[13] === LINES.length, d[13], LINES.length);
  check('rounds', d[14] === (w.mode === 'hold' ? w.rounds : 1), d[14], w.rounds);
  check('rungs', d[15] === (w.mode === 'gamble' ? w.rungs : 0), d[15], w.rungs);
  const win = w.mode === 'gamble' ? Math.floor(GAMBLE_FAIR * (w.win_pct / 50)) : GAMBLE_FAIR;
  check('gamble_win', d.readUInt32LE(16) === win, d.readUInt32LE(16), win);
  const AT_STRIPS = 56, AT_SYMBOLS = AT_STRIPS + MAX_REELS * MAX_STRIP;
  const AT_LINES = AT_SYMBOLS + MAX_SYMBOLS * 4;
  for (let r = 0; r < w.strips.length; r++) {
    const got = [...d.subarray(AT_STRIPS + r * MAX_STRIP, AT_STRIPS + r * MAX_STRIP + w.strips[r].length)];
    check(`strip ${r}`, got.join() === w.strips[r].join(), got.join(), w.strips[r].join());
  }
  for (let s2 = 0; s2 < w.mults.length; s2++) {
    check(`mult ${s2}`, d.readUInt16LE(AT_SYMBOLS + s2 * 4) === w.mults[s2],
          d.readUInt16LE(AT_SYMBOLS + s2 * 4), w.mults[s2]);
  }
  for (let l = 0; l < LINES.length; l++) {
    const got = [...d.subarray(AT_LINES + l * MAX_REELS, AT_LINES + l * MAX_REELS + 3)];
    check(`line ${l}`, got.join() === LINES[l].join(), got.join(), LINES[l].join());
  }
}
console.log(bad ? `\n❌ ${bad} mismatch(es)` : '\n✅ the chain matches machines.json exactly');
process.exit(bad ? 1 : 0);
