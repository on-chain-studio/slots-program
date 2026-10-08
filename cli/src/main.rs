//! `slots-ops`: Slot Machines, operated. Every instruction of the game is built by the client
//! Solarium generates from the program's `#[program]` block, and every account is read by casting
//! its bytes into the program's own state types — nothing here spells out a wire format. What
//! every game's tooling shares (the clusters, the admin key, the TEE login, the treasury
//! instructions, moving between rollups) is `casino-ops`.
//!
//! ```text
//! slots-ops setup                 initialize, open + float the house ledger, delegate
//! slots-ops publish               write scripts/machines.json onto the shelf
//! slots-ops verify                read every machine back: must match the sheet exactly
//! slots-ops play [machine]        one real spin end to end (--close reclaims the ledger)
//! slots-ops clear-ledger <wallet> empty and close a wallet's ledger, permission and session store
//! slots-ops status | shelf | analytics | spin [player]
//! ```
//!
//! Devnet unless `--mainnet`; the TEE unless `--public`; the admin key is `--keypair`,
//! `$CASINO_ADMIN_KEYPAIR` or the Solana CLI's.

mod machines;
mod play;

use std::path::PathBuf;

use anyhow::{Context, Result};
use casino_core::ids::PERMISSION_PROGRAM;
use casino_core::permission;
use casino_ops::magicblock::{EPHEMERAL_VAULT, MAGIC_PROGRAM};
use casino_ops::ops::Setup;
use casino_ops::vault::SYSTEM_PROGRAM;
use casino_ops::{
    keys, lamports, short, sol, AdminCommand, Chain, Game, Instruction, Keypair, Net, Ops, Player, Pubkey, Signer,
};
use clap::{Parser, Subcommand};
use slots::instructions::{close_spin::CloseSpin, initialize::Initialize};
use slots::state::analytics::{self as analytics_state, Analytics};
use slots::state::config::{MachineConfig, VERSION as CONFIG_VERSION};
use slots::state::spin::{self, Spin};

mod generated {
    solarium_client::generate_client!("slots");
}

pub use generated::Slots;

casino_ops::admin_instructions!(Slots, grow_config, add_caller);

impl Game for Slots {
    type Program = slots::Slots;
    const NAME: &'static str = "slots";
    /// The house pays inside the rollup; the analytics are written by the settle callbacks there.
    const ON_ROLLUP: &'static [&'static [u8]] = &[b"house", b"analytics"];
    const SHELF_ITEM: Option<usize> = Some(size_of::<MachineConfig>());
    const LOCAL_BUILD: Option<&'static str> = Some(concat!(env!("CARGO_MANIFEST_DIR"), "/../target/deploy/slots.so"));
}

fn pda(seeds: &[&[u8]]) -> Pubkey {
    Pubkey::find_program_address(seeds, &slots::ID).0
}
pub fn config() -> Pubkey {
    pda(&[b"config"])
}
pub fn house() -> Pubkey {
    pda(&[b"house"])
}
pub fn analytics() -> Pubkey {
    pda(&[b"analytics"])
}
pub fn identity() -> Pubkey {
    pda(&[b"identity"])
}
pub fn spin_of(user: &Pubkey) -> Pubkey {
    pda(&[b"spin", user.as_ref()])
}

#[derive(Parser)]
#[command(name = "slots-ops", about = "Operates Slot Machines through its generated client")]
struct Cli {
    #[command(flatten)]
    net: Net,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Stands the program up: initialize, open and float the house ledger, fund the house PDA,
    /// delegate the house, the analytics and the house ledger. Idempotent.
    Setup {
        /// SOL of payout float on the house ledger. It must cover the largest single win — GOLD
        /// RUSH at the 0.05 stake pays x80, 4 SOL — so mainnet floats 5 by default; devnet plays
        /// with a token 1.
        #[arg(long)]
        float: Option<f64>,
        /// SOL on the house PDA, its payer inside the rollup: spin rent and the VRF.
        #[arg(long, default_value_t = 0.1)]
        house_fund: f64,
        /// Mints the house ledger has room for.
        #[arg(long, default_value_t = 8)]
        slots: u16,
    },
    /// Publishes the shelf from the machine sheet, one machine per transaction. A machine already
    /// on chain byte for byte is left alone.
    Publish {
        #[arg(long)]
        sheet: Option<PathBuf>,
        /// Rewrite machines that already match.
        #[arg(long)]
        force: bool,
    },
    /// Reads every machine off the chain and compares it, byte for byte, with the sheet — the only
    /// proof a publish landed.
    Verify {
        #[arg(long)]
        sheet: Option<PathBuf>,
    },
    /// The shelf as published.
    Shelf,
    /// The lifetime counters, live off the rollup (the admins may read them there).
    Analytics,
    /// A player's spin on the rollup; the admin's by default.
    Spin { player: Option<String> },
    /// Admin: drops a stranded spin and returns its rent to the house. Rollup.
    CloseSpin { player: String },
    /// Plays one real spin end to end on the shelf's machine at this index; the machine's mode
    /// decides how — lines spin once, hold keeps a reel through its respins, gamble climbs a rung.
    Play {
        #[arg(default_value_t = 0)]
        machine: u64,
        /// Bring the ledger home and withdraw it to the wallet afterwards.
        #[arg(long)]
        close: bool,
        /// Play as this keypair rather than the admin.
        #[arg(long)]
        wallet: Option<PathBuf>,
    },
    /// Empties and closes a wallet's vault ledger: brings it home from whichever rollup holds it,
    /// withdraws everything to the wallet, then closes the ledger, its permission and its session
    /// store. For a test wallet that is done, or one that can no longer play; the wallet signs.
    ClearLedger { wallet: PathBuf },
    #[command(flatten)]
    Admin(AdminCommand),
}

fn key(text: &str) -> Result<Pubkey> {
    text.parse().map_err(|e| anyhow::anyhow!("{text} is not a key: {e:?}"))
}

fn built(result: solarium_client::result::Result<Instruction>) -> Result<Instruction> {
    result.map_err(|e| anyhow::anyhow!("{e}"))
}

/// Every machine the shelf publishes, read off basenet (the config is never delegated).
pub async fn machines_on_chain(chain: &Chain) -> Result<Vec<MachineConfig>> {
    let account = chain.account(&config()).await?.context("no config on chain — run `setup`")?;
    let (header, items) = casino_ops::shelf::<MachineConfig>(&account.data).context("the config is not a shelf")?;
    Ok(items.into_iter().take(header.count as usize).collect())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let chain = Chain::connect(&cli.net).await?;
    let admin = chain.admin.pubkey();
    match cli.command {
        Command::Setup { float, house_fund, slots } => {
            let float = float.unwrap_or(if cli.net.mainnet { 5.0 } else { 1.0 });
            let initialize = built(generated::Slots::initialize_instruction(
                admin, config(), house(), analytics(), permission::address(&analytics()), PERMISSION_PROGRAM,
                SYSTEM_PROGRAM,
                // A fresh shelf: machines come from `publish`.
                Initialize { machine_count: 0 },
            ))?;
            // Setup leaves an existing config alone, but a shelf at an older layout must be
            // re-initialised: that sets its version and grows it to the new stride. Its machines
            // then read as nonsense until `publish` rewrites them.
            if let Some(account) = chain.account(&config()).await? {
                let version = u64::from_le_bytes(account.data[8..16].try_into()?);
                if version != CONFIG_VERSION {
                    chain.base.send(&[initialize.clone()], &[&chain.admin]).await?;
                    println!("shelf moved from version {version} to {CONFIG_VERSION} — run `publish` next\n");
                }
            }
            let plan = Setup { initialize, slots: vec![slots], public: vec![], house_fund: lamports(house_fund), float: lamports(float) };
            Ops::<Slots>::new(&chain).setup(plan).await
        }
        Command::Publish { sheet, force } => {
            let sheet = machines::load(&sheet.unwrap_or_else(|| machines::default_path().into()))?;
            let account = chain.account(&config()).await?.context("no config on chain — run `setup`")?;
            let (_, slots) = casino_ops::shelf::<MachineConfig>(&account.data).context("the config is not a shelf")?;
            if sheet.len() > slots.len() {
                anyhow::bail!("the shelf has room for {} machines and the sheet has {}: `grow-shelf {}` first", slots.len(), sheet.len(), sheet.len() - slots.len());
            }
            for (index, machine) in sheet.iter().enumerate() {
                let want = machine.built(index as u8)?;
                if !force && bytemuck::bytes_of(&slots[index]) == bytemuck::bytes_of(&want) {
                    println!("  ⏭  {index} {:<13} already published", machine.name);
                    continue;
                }
                let ix = built(generated::Slots::set_machine_instruction(admin, config(), machine.set_machine(index as u8)?))?;
                let signature = chain.base.send(&[ix], &[&chain.admin]).await?;
                println!("  ✅ {index} {:<13} {:<6} stake {} SOL  {}", machine.name, machine.mode, sol(machine.stake), short(signature));
            }
            println!("shelf: {} machines published", machines_on_chain(&chain).await?.len());
            Ok(())
        }
        Command::Verify { sheet } => {
            let sheet = machines::load(&sheet.unwrap_or_else(|| machines::default_path().into()))?;
            let chain_machines = machines_on_chain(&chain).await?;
            let mut bad = 0;
            for index in 0..sheet.len().max(chain_machines.len()) {
                match (chain_machines.get(index), sheet.get(index)) {
                    (Some(_), None) => {
                        bad += 1;
                        println!("  ❌ machine {index} is on chain but not in the sheet");
                    }
                    (None, Some(machine)) => {
                        bad += 1;
                        println!("  ❌ {} is in the sheet but not published", machine.name);
                    }
                    (Some(on_chain), Some(machine)) => {
                        let differences = machines::differences(on_chain, &machine.built(index as u8)?);
                        println!("machine {index} — {}{}", machine.name, if differences.is_empty() { "" } else { "  DIFFERS" });
                        for line in &differences {
                            println!("  ❌ {line}");
                        }
                        bad += differences.len();
                    }
                    (None, None) => {}
                }
            }
            if bad > 0 {
                anyhow::bail!("{bad} mismatch(es) — republish with `slots-ops publish`");
            }
            println!("\n✅ the chain matches the sheet exactly");
            Ok(())
        }
        Command::Shelf => {
            for (index, m) in machines_on_chain(&chain).await?.iter().enumerate() {
                println!(
                    "{index}  {:<6}  stake {} SOL  {} reels × {} stops  {} symbols  {} lines  rounds {}  rungs {}  win {:.2}%",
                    machines::mode_name(m.mode), sol(m.stake_lamports), m.reel_count, m.strip_len, m.symbol_count, m.line_count,
                    m.rounds, m.gamble_rungs, m.gamble_win as f64 / (1u64 << 32) as f64 * 100.0,
                );
            }
            Ok(())
        }
        Command::Analytics => {
            let live = chain.live(&analytics(), &chain.admin).await?;
            let account = live.account.context("no analytics account — run `setup`")?;
            let a = casino_ops::decode::<Analytics>(&account.data)
                .filter(|a| a.discriminator == analytics_state::DISCRIMINATOR)
                .context("not an analytics account")?;
            println!("analytics {}  —  {}{}\n", analytics(), chain.describe(&live.place), if live.stale { " (basenet's copy)" } else { "" });
            println!("  taken in    {} SOL", sol(a.lamports_in));
            let per = |counts: &[u64]| {
                let lines: Vec<String> = counts.iter().enumerate().filter(|(_, n)| **n > 0).map(|(i, n)| format!("#{i}×{n}")).collect();
                if lines.is_empty() { "none".to_string() } else { lines.join("  ") }
            };
            println!("  bets        {}", per(&a.bets_placed));
            println!("  collected   {}", per(&a.bets_collected));
            for row in a.payouts.iter().filter(|row| row.amount > 0) {
                let mint = Pubkey::new_from_array(row.mint);
                let name = if row.mint == [0; 32] { "SOL".to_string() } else { short(mint) };
                println!("  paid out    {name} {}", if row.mint == [0; 32] { sol(row.amount) } else { row.amount.to_string() });
            }
            Ok(())
        }
        Command::Spin { player } => {
            let user = player.map(|p| key(&p)).transpose()?.unwrap_or(admin);
            let rollup = chain.rollup().await?;
            let Some(account) = rollup.account(&spin_of(&user)).await? else {
                println!("no spin for {user}");
                return Ok(());
            };
            let s = casino_ops::decode::<Spin>(&account.data).filter(|s| s.discriminator == spin::DISCRIMINATOR).context("not a spin")?;
            let status = ["bought", "requested", "rolled"].get(s.status as usize).copied().unwrap_or("?");
            println!("spin {}  of {}", spin_of(&user), Pubkey::new_from_array(s.user));
            println!("  machine {}  status {status}  round {}  hold {:#b}  pending {} SOL", s.machine_id, s.round, s.hold, sol(s.pending));
            println!("  stops {:?}  consenter {}", &s.stops[..5], Pubkey::new_from_array(s.consenter));
            Ok(())
        }
        Command::CloseSpin { player } => {
            let user = key(&player)?;
            let rollup = chain.rollup().await?;
            if rollup.account(&spin_of(&user)).await?.is_none() {
                println!("⏭  no spin for {user}");
                return Ok(());
            }
            let ix = built(generated::Slots::close_spin_instruction(admin, house(), spin_of(&user), EPHEMERAL_VAULT, MAGIC_PROGRAM, CloseSpin { user }))?;
            rollup.send(&[ix], &[&chain.admin]).await?;
            println!("✅ the spin of {user} is closed, its rent back with the house");
            Ok(())
        }
        Command::Play { machine, close, wallet } => {
            let wallet = match wallet {
                Some(path) => keys::read(&path)?,
                None => chain.admin.insecure_clone(),
            };
            play::play(&chain, &wallet, machine, close).await
        }
        Command::ClearLedger { wallet } => clear_ledger(&chain, &keys::read(&wallet)?).await,
        Command::Admin(command) => command.run::<Slots>(&chain).await,
    }
}

async fn clear_ledger(chain: &Chain, wallet: &Keypair) -> Result<()> {
    let player = Player::new(chain, wallet, slots::ID);
    let owner = player.key();
    let before = chain.balance(&owner).await?;
    println!("wallet {owner}  {} SOL\nledger {}", sol(before), player.ledger_key());
    if chain.account(&player.ledger_key()).await?.is_none() {
        println!("no ledger");
        return Ok(());
    }
    // Home from wherever it is in session, and every mint it holds back in the wallet.
    let withdrawn = player.withdraw_all().await?;
    println!("  ✅ home, withdrew {} SOL", sol(withdrawn));
    chain.base.send(&[casino_ops::vault::close_own_ledger(&owner)], &[wallet]).await?;
    let store = casino_ops::vault::session_store(&owner);
    let gone = chain.account(&player.ledger_key()).await?.is_none() && chain.account(&store).await?.is_none();
    let after = chain.balance(&owner).await?;
    println!(
        "  {} ledger, permission and session store {}\nwallet now {} SOL (+{})",
        if gone { "✅" } else { "❌" },
        if gone { "closed" } else { "STILL THERE" },
        sol(after),
        sol(after.saturating_sub(before)),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    //! The generated client, checked against the program's own numbers and against the bytes the
    //! old scripts sent for the same calls.

    use super::*;
    use casino_ops::admin as shared;
    use casino_ops::Treasury;

    fn number(ix: &Instruction) -> u64 {
        u64::from_le_bytes(ix.data[..8].try_into().unwrap())
    }

    #[test]
    fn the_shared_instructions_carry_slots_numbers() {
        let admin = Pubkey::new_from_array([1; 32]);
        let house = Treasury::house::<Slots>();
        let validator = casino_ops::TEE_VALIDATOR;
        assert_eq!(number(&shared::delegate::<Slots>(&admin, b"house", &validator).unwrap()), 2);
        assert_eq!(number(&shared::request_undelegation::<Slots>(&admin, &house.address).unwrap()), 4);
        assert_eq!(number(&shared::grow_config::<Slots>(&admin, 1).unwrap()), 6);
        assert_eq!(number(&shared::open_ledger::<Slots>(&admin, &house, 8).unwrap()), 8);
        assert_eq!(number(&shared::delegate_treasury::<Slots>(&admin, &house, &validator).unwrap()), 9);
        assert_eq!(number(&shared::undelegate_treasury::<Slots>(&admin, &house).unwrap()), 10);
        assert_eq!(number(&shared::close_ledger::<Slots>(&admin, &house, &[]).unwrap()), 11);
        assert_eq!(number(&shared::authorize_treasury::<Slots>(&admin, &house).unwrap()), 12);
        assert_eq!(number(&shared::set_privacy::<Slots>(&admin, &house, true).unwrap()), 13);
        assert_eq!(number(&shared::withdraw_house::<Slots>(&admin, &Pubkey::default(), 1).unwrap()), 14);
    }

    /// `setup-slots.mjs`'s `delegatePdaIx('house', house)`: header(2), one seed, the validator.
    #[test]
    fn a_pda_delegation_is_the_bytes_the_script_sent() {
        let admin = Pubkey::new_from_array([1; 32]);
        let validator = casino_ops::PUBLIC_ER_VALIDATOR;
        let ix = shared::delegate::<Slots>(&admin, b"house", &validator).unwrap();
        let mut want = 2u64.to_le_bytes().to_vec();
        want.extend_from_slice(&1u32.to_le_bytes());
        want.extend_from_slice(&5u32.to_le_bytes());
        want.extend_from_slice(b"house");
        want.extend_from_slice(validator.as_ref());
        assert_eq!(ix.data, want);
        let buffer = Pubkey::find_program_address(&[b"buffer", house().as_ref()], &slots::ID).0;
        let keys: Vec<Pubkey> = ix.accounts.iter().map(|m| m.pubkey).collect();
        assert_eq!(keys[..4], [admin, house(), slots::ID, buffer]);
        assert!(ix.accounts[0].is_signer && ix.accounts[1].is_writable && !ix.accounts[2].is_writable);
    }

    /// `set-machines.mjs`'s encoder, ported as it was, against the generated `set_machine`.
    #[test]
    fn a_published_machine_is_the_bytes_the_script_sent() {
        let sheet = machines::load(machines::default_path().as_ref()).unwrap();
        // The script only ever encoded complete-line machines.
        for (index, m) in sheet.iter().enumerate().filter(|(_, m)| m.run_pays.is_none()) {
            let mut want = 5u64.to_le_bytes().to_vec();
            want.extend_from_slice(&[index as u8, m.mode().unwrap()]);
            want.extend_from_slice(&m.stake.to_le_bytes());
            want.extend_from_slice(&[3, m.rounds, m.rungs]);
            want.extend_from_slice(&((2f64.powi(31) * (m.win_pct / 50.0)).floor() as u32).to_le_bytes());
            want.extend_from_slice(&[0; 32]);
            want.extend_from_slice(&(m.strips.len() as u32).to_le_bytes());
            for strip in &m.strips {
                want.extend_from_slice(&(strip.len() as u32).to_le_bytes());
                want.extend_from_slice(strip);
            }
            want.extend_from_slice(&(m.mults.len() as u32).to_le_bytes());
            for mult in &m.mults {
                want.extend_from_slice(&mult.to_le_bytes());
                want.push(0);
            }
            want.extend_from_slice(&(machines::LINES.len() as u32).to_le_bytes());
            for line in machines::LINES {
                want.extend_from_slice(&3u32.to_le_bytes());
                want.extend_from_slice(&line);
            }
            want.extend_from_slice(&m.shown_in().unwrap().to_le_bytes());
            // No run table: the arcade machines pay complete lines.
            want.push(0);
            let admin = Pubkey::new_from_array([1; 32]);
            let ix = generated::Slots::set_machine_instruction(admin, config(), m.set_machine(index as u8).unwrap()).unwrap();
            assert_eq!(ix.data, want, "machine {index}");
            assert_eq!(ix.accounts.len(), 2);
            // And the program would take it.
            m.built(index as u8).unwrap();
        }
    }

    /// The five-reel machine in the sheet is private-casino's `gold-rush-five`: SLOT-MACHINES.md
    /// balances it to 89.97%, so a slip converting its strips, lines or pays shows in the return.
    #[test]
    fn the_runs_machine_returns_what_it_was_balanced_to() {
        let sheet = machines::load(machines::default_path().as_ref()).unwrap();
        let (index, five) = sheet.iter().enumerate().find(|(_, m)| m.run_pays.is_some()).unwrap();
        let built = five.built(index as u8).unwrap();
        assert_eq!(built.shown_in, slots::state::config::SHOWN_IN_CASINO, "the arcade cannot play a runs machine");
        let engine = slots_engine::parse(bytemuck::bytes_of(&built)).unwrap();
        let rtp = slots_engine::analysis::report(&engine).unwrap().rtp;
        assert!((rtp - 0.8997).abs() < 0.0001, "rtp {rtp}");
    }

    /// `play-slots.mjs`'s `resolveBetAccounts`, after the receipt and the vault authority the vault
    /// puts first: the spin's permission and the ACL program follow the analytics, so the settle
    /// can make the spin private as it creates it.
    #[test]
    fn a_bet_settles_with_the_spin_permission() {
        let user = Pubkey::new_from_array([5; 32]);
        let resolve = generated::Slots::resolve_bet_instruction(
            Pubkey::new_from_array([6; 32]), casino_ops::vault::authority(), config(), house(), spin_of(&user),
            EPHEMERAL_VAULT, MAGIC_PROGRAM, analytics(), permission::address(&spin_of(&user)), PERMISSION_PROGRAM, vec![],
            slots::instructions::resolve_bet::ResolveBet { human: user, machine_id: 1, consenter: user },
        )
        .unwrap();
        let tail: Vec<(Pubkey, bool)> = resolve.accounts.iter().skip(2).map(|m| (m.pubkey, m.is_writable)).collect();
        assert_eq!(
            tail,
            [
                (config(), false),
                (house(), true),
                (spin_of(&user), true),
                (EPHEMERAL_VAULT, true),
                (MAGIC_PROGRAM, false),
                (analytics(), true),
                (permission::address(&spin_of(&user)), true),
                (PERMISSION_PROGRAM, false),
            ]
        );
        assert_eq!(number(&resolve), 17);
    }

    /// `clear-ledger.mjs`'s `close_ledger`: the owner twice, then the vault's accounts, the session
    /// store last.
    #[test]
    fn a_cleared_ledger_closes_its_session_store() {
        let owner = Pubkey::new_from_array([7; 32]);
        let ledger = casino_ops::vault::ledger(&owner);
        let ix = casino_ops::vault::close_own_ledger(&owner);
        let keys: Vec<Pubkey> = ix.accounts.iter().map(|m| m.pubkey).collect();
        assert_eq!(
            keys,
            [
                owner,
                owner,
                ledger,
                casino_ops::vault::reserve(),
                permission::address(&ledger),
                PERMISSION_PROGRAM,
                casino_core::ids::TOKEN_PROGRAM,
                SYSTEM_PROGRAM,
                casino_ops::vault::session_store(&owner),
            ]
        );
        assert!(ix.accounts.last().unwrap().is_writable);
        assert_eq!(ix.data, casino_ops::vault::discriminator("close_ledger"));
    }

    /// `play-slots.mjs`'s `requestBetIx`: header(16), the machine, ten accounts, the wallet signing.
    #[test]
    fn a_bet_is_what_the_script_sent() {
        let (session, user) = (Pubkey::new_from_array([4; 32]), Pubkey::new_from_array([5; 32]));
        let receipt = casino_core::receipt::address(&slots::ID, &session);
        let ix = generated::Slots::request_bet_instruction(
            session, user, config(), house(), receipt, EPHEMERAL_VAULT, MAGIC_PROGRAM, casino_core::ids::VAULT_PROGRAM,
            Treasury::house::<Slots>().ledger, casino_ops::magicblock::MAGIC_CONTEXT,
            slots::instructions::request_bet::RequestBet { machine_id: 2 },
        )
        .unwrap();
        let mut want = 16u64.to_le_bytes().to_vec();
        want.extend_from_slice(&2u64.to_le_bytes());
        assert_eq!(ix.data, want);
        assert_eq!(ix.accounts.len(), 10);
        assert!(ix.accounts[0].is_signer && !ix.accounts[0].is_writable);
        assert!(ix.accounts[3].is_writable && !ix.accounts[2].is_writable);
    }
}
