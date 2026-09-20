# slots-program

On-chain program for **Slot Machines**, sibling to `../scratch-cards-program` and in its exact
shape: a native (non-Anchor) program with manual dispatch, `ephemeral-rollups-sdk` delegation,
bytemuck state, and hand-rolled CPIs for the vault and the MagicBlock VRF.

Program id: `SLoTSdnmBH5KtNJjhEYw1MeWTKAfRnFfQTTcpgwRn2Q` (`keys/program-keypair.json`) —
live on devnet, upgrade authority `~/casino_admin.json`. Scripts pick the cluster with
`--mainnet`; the admin key is casino_admin on both.

## Money

Same vault as scratch cards (`VAULTrDSU…`), which is the point: a ledger is `["ledger", owner]`
keyed on the wallet, so a player funded there can spin here without funding again. Every payment
is a vault receipt — request names the movements, settle moves them ledger-to-ledger and calls
back into the game in the same instruction. A dropped settle pays nothing and creates nothing.

**No jackpot.** The whole stake goes to the house and the whole return target lives in the
machines: **90% RTP** (scratch runs 80% + a 10% pot; there is no pot here). The gamble ladder's
rungs are published **shaded at 48%** — part of the edge is taken on the flip, by the player's
own choice; a rung above fair, or below the 45% typo floor, is unpublishable.

## The three machines

| | mode | loop |
| --- | --- | --- |
| GOLD RUSH | `MODE_LINES` | spin, read 5 lines. No decisions. |
| GRAVITY WELL | `MODE_HOLD` | three grids; between them the player commits which reels ride. |
| LUCKY SPINS | `MODE_GAMBLE` | spin, then any win may be doubled up the ladder, rung by rung. |

The volatility profiles follow the *mechanics*, not the themes: holding well multiplies a pay
table's return several-fold (enumerated, not estimated), so GRAVITY WELL is structurally the
frequent-hits machine and LUCKY SPINS the volatile one. The two can never share a pay table.

## Accounts

| PDA | What |
| --- | --- |
| `["config"]` | The shelf: every machine's strips, lines and pay table — all public. Grows, never shrinks. |
| `["house"]` | Payer inside the rollup (spin rent + VRF); owns the house ledger (the payout float). Delegated. |
| `["analytics"]` | Lifetime counters, written only by settle callbacks — every number is settled money. Delegated; TEE reads restricted to the admins. |
| `["spin", user]` | One bet in flight, **ephemeral** — created by the stake's settle callback, closed by the payout's. Its existence is the one-bet-at-a-time mutex and the unpaid flag. Carries its own terms, copied at purchase. |

## The flow

```
RequestBet ─ settle ─▶ spin exists (Bought)
RequestReveal ──▶ Requested ── VRF callback ──▶ Rolled        (permissionless, retryable)
   Hold(mask) / Gamble ──▶ Bought (next round)                (player/consenter-signed)
RequestCollect ─ settle ─▶ paid, spin closed                  (a loss is an empty receipt)
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
enumerator and writes `scripts/machines.json`. Republishing is `set-machines.mjs`;
`verify-machines.mjs` reads every field back off the chain and is the only proof a publish landed.

## Build & test

```
cargo build-sbf                      # target/deploy/slots.so
cargo test                           # program: layout pins + SetMachine validation
(cd engine && cargo test --release)  # engine: determinism, RTP enumeration, hold DP, parity
```

## Scripts

```
node scripts/setup-slots.mjs         # initialize, open + float the house ledger, delegate
node scripts/set-machines.mjs        # publish machines.json
node scripts/verify-machines.mjs     # must say: the chain matches machines.json exactly
node scripts/play-slots.mjs [0|1|2]  # one real spin end to end (--close reclaims the ledger)
```
