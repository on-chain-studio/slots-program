//! One real spin on devnet, end to end, the way the browser client plays one: the player's ledger
//! funded and in session on the rollup, a session key signing every move, the stake settled into a
//! spin, the reels revealed round by round, a decision where the machine offers one, and the
//! collect that pays it.

use anyhow::{bail, Result};
use casino_core::ids::{PERMISSION_PROGRAM, VAULT_PROGRAM, VRF_PROGRAM};
use casino_core::permission;
use casino_ops::magicblock::{EPHEMERAL_VAULT, MAGIC_CONTEXT, MAGIC_PROGRAM, SLOT_HASHES, VRF_EPHEMERAL_QUEUE};
use casino_ops::vault::{self, SYSTEM_PROGRAM};
use casino_ops::{sol, Chain, Endpoint, Instruction, Keypair, Player, Pubkey, Signer, Treasury};
use slots::instructions::request_collect::payout;
use slots::instructions::{hold::Hold, request_bet::RequestBet, resolve_bet::ResolveBet, resolve_collect::ResolveCollect};
use slots::state::config::{MachineConfig, MODE_GAMBLE, MODE_HOLD};
use slots::state::spin::{self, Spin, SpinStatus};

use crate::{analytics, config, generated, house, identity, spin_of, Slots};

const BOUGHT: u64 = SpinStatus::Bought as u64;
const ROLLED: u64 = SpinStatus::Rolled as u64;
const COLLECTED: u64 = SpinStatus::Collected as u64;
/// `NothingToCollect`: a gamble on a base spin that won nothing.
const NOTHING_TO_COLLECT: &str = "Custom(9)";

fn built(result: solarium_client::result::Result<Instruction>) -> Result<Instruction> {
    result.map_err(|e| anyhow::anyhow!("{e}"))
}

struct Round<'p, 'c> {
    player: &'p Player<'c>,
    at: Endpoint,
}

impl Round<'_, '_> {
    fn user(&self) -> Pubkey {
        self.player.key()
    }

    async fn spin(&self) -> Result<Option<Spin>> {
        let account = self.at.account(&spin_of(&self.user())).await?;
        Ok(account.and_then(|a| casino_ops::decode::<Spin>(&a.data)).filter(|s| s.discriminator == spin::DISCRIMINATOR))
    }

    /// The spin with the terms it was bought under, printed after it on the account.
    async fn spin_with_terms(&self) -> Result<Option<(Spin, MachineConfig)>> {
        let Some(account) = self.at.account(&spin_of(&self.user())).await? else {
            return Ok(None);
        };
        let spin = casino_ops::decode::<Spin>(&account.data).filter(|s| s.discriminator == spin::DISCRIMINATOR);
        let terms = account.data.get(Spin::SIZE..).and_then(casino_ops::decode::<MachineConfig>);
        Ok(spin.zip(terms))
    }

    async fn wait(&self, status: u64, label: &str) -> Result<Spin> {
        self.player.until(label, async || Ok(self.spin().await?.filter(|s| s.status == status))).await
    }

    async fn bet(&self, machine: u64) -> Result<()> {
        let (user, session) = (self.user(), self.player.session.pubkey());
        let request = built(generated::Slots::request_bet_instruction(
            session, user, config(), house(), self.player.receipt(), EPHEMERAL_VAULT, MAGIC_PROGRAM, VAULT_PROGRAM,
            Treasury::house::<Slots>().ledger, MAGIC_CONTEXT, RequestBet { machine_id: machine },
        ))?;
        // The spin's permission rides along: the settle callback makes the new spin private to
        // the wallet the moment it exists, so its history is never readable by a stranger.
        let resolve = built(generated::Slots::resolve_bet_instruction(
            self.player.receipt(), vault::authority(), config(), house(), spin_of(&user), EPHEMERAL_VAULT, MAGIC_PROGRAM,
            analytics(), permission::address(&spin_of(&user)), PERMISSION_PROGRAM,
            // No casino floor station: this is a spin played standing up.
            vec![],
            ResolveBet { human: user, machine_id: machine, consenter: session },
        ))?;
        self.player.play(&self.at, &[request, self.player.settle::<Slots>(resolve)]).await?;
        Ok(())
    }

    /// Asks the oracle for this round's seed and waits for it to land. Permissionless and
    /// retryable, and its own transaction, so a VRF request that fails unwinds nothing paid for.
    async fn reveal(&self, label: &str) -> Result<Spin> {
        let request = built(generated::Slots::request_reveal_instruction(
            self.user(), house(), spin_of(&self.user()), identity(), VRF_EPHEMERAL_QUEUE, SLOT_HASHES, SYSTEM_PROGRAM,
            VRF_PROGRAM,
        ))?;
        self.player.play(&self.at, &[request]).await?;
        let spin = self.wait(ROLLED, label).await?;
        println!("  ✅ {label}: seed {}…  (round {})", hex(&spin.seed[..8]), spin.round);
        Ok(spin)
    }

    /// Finishes a spin an earlier run left behind; collecting it is the only way to bet again.
    /// While a decision is still open only the consenter recorded on the spin may collect — that
    /// run's session key, which this run does not have. The wallet may always decide, though, so
    /// it plays the spin out: it holds every reel, which is collecting the grid on show, or rides
    /// the ladder, round by round, until nothing is left to decide and anyone may collect.
    async fn finish_leftover(&self) -> Result<bool> {
        let wallet = self.player.wallet;
        let mut found = false;
        while let Some((spin, terms)) = self.spin_with_terms().await? {
            if spin.status == COLLECTED {
                break;
            }
            found = true;
            if spin.status != ROLLED {
                self.reveal("the leftover spin").await?;
                continue;
            }
            let amount = payout(&spin, &terms).map_err(|e| anyhow::anyhow!("pricing the leftover spin: {e:?}"))?;
            let deciding = match terms.mode {
                MODE_HOLD => spin.round + 1 < terms.rounds(),
                MODE_GAMBLE => amount > 0 && spin.round < terms.gamble_rungs as u64,
                _ => false,
            };
            if !deciding {
                self.collect().await?;
                break;
            }
            let decide = if terms.mode == MODE_HOLD {
                generated::Slots::hold_instruction(wallet.pubkey(), spin_of(&self.user()), Hold { mask: terms.reel_mask() as u8 })
            } else {
                generated::Slots::gamble_instruction(wallet.pubkey(), spin_of(&self.user()))
            };
            self.at.send(&[built(decide)?], &[wallet]).await?;
            self.wait(BOUGHT, "the leftover spin's decision").await?;
            println!("  ✅ played the leftover spin on as the wallet (round {})", spin.round + 1);
        }
        Ok(found)
    }

    async fn collect(&self) -> Result<()> {
        let (user, session) = (self.user(), self.player.session.pubkey());
        let request = built(generated::Slots::request_collect_instruction(
            user, house(), spin_of(&user), self.player.receipt(), EPHEMERAL_VAULT, MAGIC_PROGRAM, VAULT_PROGRAM, session,
            Treasury::house::<Slots>().ledger, MAGIC_CONTEXT,
        ))?;
        let resolve = built(generated::Slots::resolve_collect_instruction(
            self.player.receipt(), vault::authority(), house(), spin_of(&user), EPHEMERAL_VAULT, MAGIC_PROGRAM, analytics(),
            ResolveCollect { human: user },
        ))?;
        self.player.play(&self.at, &[request, self.player.settle::<Slots>(resolve)]).await?;
        // Older programs close the spin instead.
        self.player
            .until("the spin collected", async || {
                Ok(self.spin().await?.is_none_or(|s| s.status == COLLECTED).then_some(()))
            })
            .await
    }
}

pub async fn play(chain: &Chain, wallet: &Keypair, machine: u64, close: bool) -> Result<()> {
    let Some(terms) = crate::machines_on_chain(chain).await?.into_iter().nth(machine as usize) else {
        bail!("machine {machine} is not on the shelf");
    };
    let terms: MachineConfig = terms;
    let player = Player::new(chain, wallet, slots::ID);
    let before_wallet = chain.balance(&player.key()).await?;
    println!("machine {machine} ({})  player {}\n", crate::machines::mode_name(terms.mode), player.key());

    println!("1. player ledger");
    // Enough for a spin, its retries and a gamble; a deposit tops up only the shortfall.
    let live = player.ready(terms.stake_lamports * 4, |m| println!("  ✅ {m}")).await?;
    println!("  ✅ live on the rollup with {live} lamports");
    let round = Round { player: &player, at: player.rollup().await? };
    let before = player.ledger_at(&round.at).await?.map(|l| l.sol).unwrap_or(0);

    println!("2. bet");
    // A spin left by an interrupted run is finished first: collecting is the only way out.
    if round.finish_leftover().await? {
        println!("  ✅ collected a leftover spin from an earlier run");
    }
    round.bet(machine).await?;
    let spin = round.wait(BOUGHT, "the stake settling").await?;
    println!("  ✅ stake settled, spin created (machine {})", spin.machine_id);

    println!("3. play");
    round.reveal("spin").await?;
    if terms.mode == MODE_HOLD {
        // Keep reel 0 through two respins: each grid lands on chain, its stops visible after it.
        for respin in 1..terms.rounds {
            let hold = built(generated::Slots::hold_instruction(player.session.pubkey(), spin_of(&player.key()), Hold { mask: 0b001 }))?;
            player.play(&round.at, &[hold]).await?;
            let spin = round.wait(BOUGHT, "the hold").await?;
            println!("  ✅ held reel 0 → round {}, stops so far {:?}", spin.round, &spin.stops[..3]);
            round.reveal(&format!("respin {respin}")).await?;
        }
    } else if terms.mode == MODE_GAMBLE {
        // Climb one rung if the base spin won; a losing base is refused the climb.
        let gamble = built(generated::Slots::gamble_instruction(player.session.pubkey(), spin_of(&player.key())))?;
        match player.play(&round.at, &[gamble]).await {
            Ok(_) => {
                let spin = round.wait(BOUGHT, "the gamble").await?;
                println!("  ✅ base win {} SOL riding → rung {}", sol(spin.pending), spin.round);
                round.reveal("flip").await?;
            }
            Err(e) if e.to_string().contains(NOTHING_TO_COLLECT) => println!("  ✅ the base spin lost — nothing to gamble"),
            Err(e) => return Err(e),
        }
    }

    println!("4. collect");
    round.collect().await?;
    let after = player.ledger_at(&round.at).await?.map(|l| l.sol).unwrap_or(0);
    let delta = after as i128 - before as i128;
    println!("  ✅ spin closed. ledger {}{} SOL this spin", if delta >= 0 { "+" } else { "-" }, sol(delta.unsigned_abs() as u64));

    if close {
        println!("5. reclaim");
        let withdrawn = player.withdraw_all().await?;
        println!("  ✅ withdrew {} SOL back to the wallet", sol(withdrawn));
    }
    let after_wallet = chain.balance(&player.key()).await?;
    println!(
        "\nthis run cost {} SOL from the wallet (deposits included; the ledger persists)",
        sol(before_wallet.saturating_sub(after_wallet))
    );
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
