import { PublicKey } from '@solana/web3.js';
import { admin, base, info, housePda, analyticsPda, ledgerPda, spinPda, dpda, DELEGATION, VAULT, PROGRAM } from './common.mjs';
const DEV = new PublicKey(process.argv[2] ?? admin.publicKey.toBase58());
const house = housePda(), analytics = analyticsPda();
const show = async (name, key) => {
  const i = await info(key);
  let v = '';
  if (i?.owner.equals(DELEGATION)) {
    const rec = await info(dpda('delegation', key, DELEGATION));
    v = rec ? ' → validator ' + new PublicKey(rec.data.subarray(8, 40)).toBase58().slice(0, 8) : '';
  }
  console.log(name.padEnd(22), key.toBase58().slice(0, 8), i ? `owner ${i.owner.toBase58().slice(0, 8)} lamports ${i.lamports}${v}` : 'absent');
};
await show('house PDA', house);
await show('analytics PDA', analytics);
await show('house ledger', ledgerPda(house));
await show('admin ledger', ledgerPda(admin.publicKey));
await show('admin spin', spinPda(admin.publicKey));
await show('dev ledger', ledgerPda(DEV));
await show('dev spin', spinPda(DEV));
console.log('admin balance', (await base.getBalance(admin.publicKey)) / 1e9, 'SOL');
