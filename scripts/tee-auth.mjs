// Signed-challenge auth for the TEE (private rollup). Every RPC there needs ?token=; without
// it reads come back empty and writes fail with 401. Node's own crypto signs — a raw ed25519
// seed becomes a key object through the fixed PKCS#8 prefix, so no extra dependency.
import crypto from 'crypto';
import bs58 from 'bs58';
import { TEE } from './net.mjs';

const PKCS8_ED25519_PREFIX = Buffer.from('302e020100300506032b657004220420', 'hex');

const sign = (message, keypair) => {
  const key = crypto.createPrivateKey({
    key: Buffer.concat([PKCS8_ED25519_PREFIX, Buffer.from(keypair.secretKey.subarray(0, 32))]),
    format: 'der', type: 'pkcs8',
  });
  return crypto.sign(null, Buffer.from(message), key);
};

export async function teeToken(keypair, host = TEE) {
  const pubkey = keypair.publicKey.toBase58();
  const c = await (await fetch(`${host}/auth/challenge?pubkey=${pubkey}`)).json();
  if (!c.challenge) throw new Error(`no challenge from ${host}: ${JSON.stringify(c)}`);
  const signature = bs58.encode(sign(c.challenge, keypair));
  const r = await fetch(`${host}/auth/login`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ pubkey, challenge: c.challenge, signature }),
  });
  const j = await r.json();
  if (!j.token) throw new Error(`TEE login failed: ${JSON.stringify(j)}`);
  return j.token;
}

export const teeEndpoint = async (keypair, host = TEE) => `${host}?token=${await teeToken(keypair, host)}`;

/**
 * An endpoint for whichever validator holds a delegated account, ready to send to: a public one
 * as the router names it, a private one with a token for it. Null when not delegated.
 */
export async function endpointHolding(account, keypair, router) {
  const r = await fetch(router, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'getDelegationStatus', params: [account.toBase58()] }),
  });
  const fqdn = (await r.json())?.result?.fqdn?.replace(/\/$/, '');
  if (!fqdn) return null;
  // A private validator refuses the unauthenticated probe; that refusal is how it says so.
  const probe = await fetch(fqdn, {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'getHealth' }),
  });
  return probe.status === 401 || probe.status === 403 ? teeEndpoint(keypair, fqdn) : fqdn;
}
