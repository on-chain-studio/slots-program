# Audit — slots-program

Read against `main` at `0fa98e6` (the Solarium/Pinocchio build, live on devnet in slot 502548148),
2026-09-23, together with the `vault-program` receipt/settle path it depends on and the MagicBlock
SDK it re-implements. Not an independent audit: a hunt, by the people who wrote the game, for ways
money can leave the house or a player against the rules. One real bug was found and fixed; the rest
of the hunt is recorded so it isn't re-run.

> Covers the program's own code, its CPIs byte for byte, and the vault's receipt/settle path. Does
> not cover the vault beyond that, the VRF oracle, the delegation program's processor, or the
> enclave — trusted as documented.

---

## The one real finding

### 🔴 `receipt::close` took its CPI program id from a caller-supplied account

- [x] Fixed — pinned to `MAGIC_PROGRAM_ID`, 2026-09-23
- [x] Deployed: scratch-cards mainnet `GURq…` (slot 449550663, byte-verified, commit `4ce5f2f` on
  `on-chain-studio/scratchcards-solana`); slots devnet in this change.

`receipt::close` built its Magic-program CPI as `program_id: *magic_program.address()` — the program
id taken from the account passed in the `magic_program` slot, and used without a check. That account
is not this program's to choose: `settle_receipt` forwards the settle transaction's trailing
accounts straight into the callback, and the settle transaction is built by the receipt's
consenter — the player's own session key.

The collect callback marks a spin paid by **closing it** (the spin holds no "collected" flag; its
absence is the marker). The payout itself is the vault's, done inside `settle_receipt` before the
callback runs. So a player could build the settle with a do-nothing program in the `magic_program`
slot: the vault pays the win, the callback's close is skipped, and the still-`Rolled` spin is
collectable again — a repeatable drain of the house float.

**Why nothing else stops it** (checked, so it isn't re-litigated): the closed spin/card has **no
permission account** (`resolve_bet` creates it with a bare `create_ephemeral_account`), and the
private-rollup ACL judges the **top-level** program of each instruction, not CPI'd ones — here the
top-levels are the game and the vault, both permission members, so the filter passes and never sees
the substituted program. The receipt-atomicity rule (below) governs the receipt, which is properly
consumed; it does not touch the persistent spin.

**Fix — pin the CPI target to its constant**, the standard arbitrary-CPI hardening, correct whether
or not this instance is reachable:

```
if magic_program.address() != &MAGIC_PROGRAM_ID { return Err(IncorrectProgramId) }
invoke_signed(&Instruction { program_id: MAGIC_PROGRAM_ID, .. }, ..)
```

The same pattern is applied to the two other places that took a CPI target from a passed account:
`commit_and_undelegate` (Magic program) and `delegate_account` (delegation + system programs), both
admin-only. We chose **not** to build a live devnet exploit to confirm reachability — a pinned CPI
target is correct code either way, and the analysis above found nothing protecting it.

---

## Considered and withdrawn

### Stale-receipt binding (round/amount on collect, stake on bet) — impossible, not applied

Priced-then-settle could in theory be exploited by changing the spin's state between the receipt's
pricing and its settle (hold to a worse grid, climb to a lost flip, re-tune the stake). It cannot
happen. A vault receipt is a data account whose rent must be balanced by the end of its
transaction, so it is created and consumed **atomically or the whole transaction reverts** — it
cannot exist across transactions at all (the reap crank is only a backstop). `request_collect`/
`request_bet` and the `settleReceipt` that consumes the receipt are one atomic transaction, and
even unbundled the receipt can't survive to a later slot, while any state change needs an oracle
round-trip a slot later and flips the spin off `Rolled`. There is no window. An earlier draft added
round/amount and stake args to bind the receipt; it was reverted as guarding nothing.

### Mid-decision collect signer (finding 4) — non-issue, not changed

While a round decision is open, only the recorded consenter may collect, not the player's own
wallet. In practice the consenter (the session key) is always the one collecting, so this strands
nobody; left as-is.

### Admin guardrails (findings 5, 6) — correct but out of scope of the security fix

`WithdrawHouse` could validate both ledgers, and `SetMachine` could reject a non-dense shelf index
that would publish zeroed machines. Both are admin-only footguns with no attacker path; not applied
here to keep this change to the one security fix. Worth adding in a routine pass.

---

## Invariants (each with the line that holds it)

| Claim | Where |
|---|---|
| A stake settles or nothing happens (bet + spin creation are one atomic settle). | `request_bet.rs`, `resolve_bet.rs`; vault `settle_receipt.rs` |
| A spin pays `payout(spin, terms)` once and is gone (closed through the Magic program **and no other** now). | `request_collect.rs`, `resolve_collect.rs`, `receipt.rs` (`close`) |
| Only the player or recorded consenter decides a round; the spin PDA is re-derived from `spin.user`. | `hold.rs`, `gamble.rs` |
| No path yields round N+1's randomness while round N's decision is open. | `hold.rs`, `gamble.rs`, `request_reveal.rs`, `callback_reveal.rs` |
| The house cannot alter a placed bet — terms are stamped on the spin at settle and read from it after. | `resolve_bet.rs`, `spin.rs` |
| Every program this program CPIs is a **constant**, never a passed account (the fix above closed the last exception). | `vault.rs`, `vrf.rs`, `chain.rs`, `magicblock.rs`, `receipt.rs` |
| The settle callbacks answer only to the vault's authority; the oracle callback only to the VRF identity. | `receipt.rs` (`require_callback`), `callback_reveal.rs` |
| A session key can bet and decide, never withdraw (the vault's `withdraw` needs the ledger owner). | vault `withdraw.rs`, `settle_receipt.rs` |

---

## Not covered, deliberately

The vault beyond its receipt/settle path, the VRF oracle's unpredictability and its identity→callback
binding, the delegation program's processor, and the enclave itself — all trusted as documented.
