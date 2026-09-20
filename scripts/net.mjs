// Which chain a script run talks to: devnet, unless --mainnet is on the command line — real
// money never by default. The flag is consumed here so positional arguments keep their places.
import os from 'os';

const flag = process.argv.indexOf('--mainnet');
if (flag !== -1) process.argv.splice(flag, 1);

export const MAINNET = flag !== -1;
export const CLUSTER = MAINNET ? 'mainnet' : 'devnet';

export const BASENET = MAINNET ? 'https://api.mainnet-beta.solana.com'
                               : 'https://api.devnet.solana.com';
export const MAGIC_RPC = MAINNET ? 'https://rpc.magicblock.app/mainnet'
                                 : 'https://rpc.magicblock.app/devnet';
export const TEE = MAINNET ? 'https://mainnet-tee.magicblock.app'
                           : 'https://devnet-tee.magicblock.app';
export const PUBLIC_ER = MAINNET ? 'https://mainnet.magicblock.app'
                                 : 'https://devnet.magicblock.app';

/** casino_admin on both clusters: it deployed the program, holds its upgrade authority, and is
 *  funded on devnet too. (Scratch cards splits by cluster; slots deliberately does not.) */
export const ADMIN_PATH = `${os.homedir()}/casino_admin.json`;

if (MAINNET) console.log('■ MAINNET — real funds\n');
