# slots-program

On-chain program for **Slot Machines**, sibling to `../scratch-cards-program` and in its exact
shape: a native (non-Anchor) program on Pinocchio, through Solarium's `#[program]` dispatch (`src/lib.rs` is the
whole wire interface, numbered as it always was), and bytemuck state. Everything below the game is
`../casino-core`, shared with the other cabinets rather than copied here: the chain as the program uses it, the
vault and its receipts, MagicBlock's delegation, ephemeral accounts, TEE permissions and VRF (through
`ephemeral-rollups-pinocchio`, MagicBlock's own crate), the config shelf, and the admin instructions 2–4, 6 and
8–14. Permissions are created once and never updated.

Program id: `SLoTSdnmBH5KtNJjhEYw1MeWTKAfRnFfQTTcpgwRn2Q` (`~/keys/slots_program.json`) —
live on devnet, upgrade authority casino_admin. `slots-ops` (below) picks the cluster with
`--mainnet`; the admin key is casino_admin on both, and it is the only admin: the dev key reads the
analytics and nothing more.

## Money

Same vault as scratch cards (`VAULTrDSU…`), which is the point: a ledger is `["ledger", owner]`
keyed on the wallet, so a player funded there can spin here without funding again. Every payment
is a vault receipt — request names the movements, settle moves them ledger-to-ledger and calls
back into the game in the same instruction. A dropped settle pays nothing and creates nothing.

**No jackpot.** The whole stake goes to the house and the whole return target lives in the
machines: **90% RTP** (scratch runs 80% + a 10% pot; there is no pot here). The gamble ladder's
rungs are published **shaded at 48%** — part of the edge is taken on the flip, by the player's
own choice; a rung above fair, or below the 45% typo floor, is unpublishable.

## The shelf

Two machines, each published at two stakes — RTP is a ratio, so the strips are shared and the
stake only scales the payout:

| id | machine | mode | stake | loop |
| --- | --- | --- | --- | --- |
| 0 | NEON NIGHTS | `MODE_LINES` | 0.005 | spin, read 5 lines. No decisions. |
| 1 | GOLD RUSH | `MODE_LINES` | 0.05 | the same maths at the higher stake; x80 = 4 SOL is the shelf's top win. |
| 2 | GRAVITY WELL | `MODE_HOLD` | 0.005 | three grids; between them the player commits which reels ride. |
| 3 | BLACK HOLE | `MODE_HOLD` | 0.05 | the same maths at the higher stake. |

The volatility profiles follow the *mechanics*, not the themes: holding well multiplies a pay
table's return several-fold (enumerated, not estimated), so the hold machines are structurally the
frequent-hits ones. The gamble ladder (`MODE_GAMBLE`) is no longer on the shelf; the program and
the engine still play it.

## Accounts

| PDA | What |
| --- | --- |
| `["config"]` | The shelf: every machine's strips, lines and pay table — all public. Grows, never shrinks. |
| `["house"]` | Payer inside the rollup (spin rent + VRF); owns the house ledger (the payout float). Delegated. |
| `["analytics"]` | Lifetime counters, written only by settle callbacks — every number is settled money. Delegated; TEE reads restricted to the analytics readers (the ops key and the dev key). |
| `["spin", user]` | The player's bet, **ephemeral** and reused: created by the first stake's settle callback, marked collected by the payout's. A new bet needs a collected spin. Carries its own terms, copied at purchase, and a generation that rejects VRF answers meant for an earlier bet. |

## The flow

```
RequestBet ─ settle ─▶ spin exists (Bought)
RequestReveal ──▶ Requested ── VRF callback ──▶ Rolled        (permissionless, retryable)
   Hold(mask) / Gamble ──▶ Bought (next round)                (player/consenter-signed)
RequestCollect ─ settle ─▶ paid, spin Collected               (a loss is an empty receipt)
```

The ordering *is* the security: `Hold` and `Gamble` are the only paths that advance the round,
and `RequestReveal` the only path to a new seed, so no sequence of instructions can produce
round N+1's randomness while round N's decision is open. Each VRF request carries its round in
the callback args and the callback refuses a mismatch — a re-fired request's late duplicate can
never land a seen seed as a later round's randomness. (Verified live: the oracle echoes the args
after the randomness.)

Decisions accept the **consenter** recorded on the spin — the session key the vault proved may
act for the user when the stake settled — so no wallet prompt per round. Collecting mid-hold is
allowed because it is provably harmless: collecting grid N equals holding every reel and
respinning.

## The engine

`engine/` (`slots-engine`) is the shared basis: the program calls it, the client runs the same
crate as wasm, and `parse` reads a machine straight out of the account bytes — no second
serialisation to drift. A machine is its **strips**: an ordered run of symbols per reel,
independent per reel, three consecutive positions visible. Odds are emergent, so
`analysis::report` *enumerates* them — all 8000 stops, the hold machine under optimal play via a
DP over hold masks. Nothing is sampled.

`examples/solve.rs` is the balancing tool: hill-climbs counts and multipliers against the exact
enumerator and writes `scripts/machines.json`. Republishing is `slots-ops publish`;
`slots-ops verify` reads every machine back off the chain, byte for byte, and is the only proof a
publish landed.

## Build & test

```
cargo build-sbf                      # target/deploy/slots.so
cargo test                           # program: layout + wire pins, SetMachine validation
cargo test --test program -- --ignored
                                     # the built .so run in Mollusk: dispatch, refusals, hold, VRF
(cd engine && cargo test --release)  # engine: determinism, RTP enumeration, hold DP, parity
```

## Operating it

`cli/` is `slots-ops`, the operator's tool. Every instruction it sends is built by the client
Solarium generates from the `#[program]` block in `src/lib.rs`, and every account it reads is cast
into the program's own state types; what every game's tooling shares — the clusters, the admin
key, the TEE login, the treasury instructions, moving between rollups — is
[`casino-ops`](https://github.com/on-chain-studio/casino-ops). It is a workspace of its own, so
`cargo build-sbf` and `cargo test` here never build it.

```
cd cli
cargo run -- setup                   # initialize, open + float the house ledger (5 SOL on mainnet, 1 on devnet), delegate
cargo run -- publish                 # publish scripts/machines.json (machines already on chain are skipped)
cargo run -- verify                  # must say: the chain matches the sheet exactly
cargo run -- play [0|1|2|3]          # one real spin end to end, as the machine's mode plays (--close reclaims the ledger)
cargo run -- clear-ledger <wallet>   # empty and close a wallet's ledger, permission and session store
cargo run -- status [player]         # where every account is, what the ledgers hold
cargo run -- shelf | analytics | spin [player]
cargo run -- grow-shelf [n] | move <tee|er> | float <sol> | where | help
```

Devnet unless `--mainnet`; the TEE unless `--public` (the public ER shows every failed
transaction's logs). The admin key is `--keypair`, `$CASINO_ADMIN_KEYPAIR` (the shared keys folder's
`~/keys/casino_admin.json`, say), or the Solana CLI's own. `play` plays as that key unless
`--wallet` names another keypair, with a session key the vault authorizes for the run, the way the
browser client plays; a spin an interrupted run left behind is played out and collected first.
`move tee` is how the house, the analytics and the house ledger come home from another validator
and go to the TEE.
