// Empties and closes a wallet's vault ledger: brings it home from the TEE, withdraws every
// lamport to the wallet, closes the ledger, its permission and its session store. For a test
// wallet that is done, or one that can no longer play. The wallet's keypair signs throughout.
//
//   node scripts/clear-ledger.mjs <keypair.json>

import fs from 'fs';
import { Keypair, PublicKey, TransactionInstruction } from '@solana/web3.js';
import {
  base, connect, send, sleep, info, ownerIs, ledgerPda, reservePda, permPda, sessionPda,
  undelegateOwnLedgerIx, decodeLedger, anchorDisc, u64, sg, rw, ro,
  VAULT, DELEGATION, PERMISSION, SYSTEM, SOL_MINT,
} from './common.mjs';
import { teeEndpoint } from './tee-auth.mjs';

const TOKEN = new PublicKey('TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA');
const path = process.argv[2] ?? (() => { throw new Error('usage: clear-ledger.mjs <keypair.json>'); })();
const wallet = Keypair.fromSecretKey(new Uint8Array(JSON.parse(fs.readFileSync(path))));
const owner = wallet.publicKey;
const ledger = ledgerPda(owner);
const before = await base.getBalance(owner);
console.log(`wallet ${owner.toBase58()}  ${(before / 1e9).toFixed(6)} SOL\nledger ${ledger.toBase58()}`);

const at = await info(ledger);
if (!at) { console.log('no ledger'); process.exit(0); }
if (at.owner.equals(DELEGATION)) {
  const er = connect(await teeEndpoint(wallet));
  await send(er, [undelegateOwnLedgerIx(owner)], [wallet]);
  for (let i = 0; i < 60 && !(await ownerIs(ledger, VAULT)); i++) await sleep(2000);
  if (!(await ownerIs(ledger, VAULT))) throw new Error('ledger never came home');
  console.log('  ✅ ledger home on basenet');
}

const sol = decodeLedger((await info(ledger)).data).sol;
if (sol > 0n) {
  await send(base, [new TransactionInstruction({
    programId: VAULT,
    keys: [sg(owner), rw(ledger), rw(reservePda()), ro(SYSTEM), ro(SYSTEM), ro(TOKEN), ro(SYSTEM)],
    data: Buffer.concat([anchorDisc('withdraw'), SOL_MINT.toBuffer(), u64(sol)]),
  })], [wallet]);
  console.log(`  ✅ withdrew ${(Number(sol) / 1e9).toFixed(6)} SOL`);
}

await send(base, [new TransactionInstruction({
  programId: VAULT,
  keys: [sg(owner), sg(owner), rw(ledger), rw(reservePda()), rw(permPda(ledger)), ro(PERMISSION), ro(TOKEN), ro(SYSTEM), rw(sessionPda(owner))],
  data: anchorDisc('close_ledger'),
})], [wallet]);
const gone = !(await info(ledger)) && !(await info(sessionPda(owner)));
const after = await base.getBalance(owner);
console.log(`  ${gone ? '✅' : '❌'} ledger, permission and session store ${gone ? 'closed' : 'STILL THERE'}\nwallet now ${(after / 1e9).toFixed(6)} SOL (+${((after - before) / 1e9).toFixed(6)})`);
