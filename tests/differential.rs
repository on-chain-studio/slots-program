//! Parity between two builds of this program, instruction by instruction.
//!
//! Every instruction is run on a baseline build and a candidate build from the same accounts, and
//! everything observable must match: the result, the log, the return data and every account
//! afterwards. Nothing here says what the right answer is — only that both builds give the same
//! one — so a rewrite is checked against the program it replaces rather than against a spec.
//!
//! The programs the game calls (vault, VRF, delegation, magic, permission, token) are all the
//! `cpi-recorder` fixture, which logs the exact bytes and account list of every call it gets. Two
//! builds that call out differently log differently. The system program is the real one.
//!
//! Each case runs as written and then mutated: each signer unsigned, the account list cut short,
//! each account swapped for a stranger, the arguments cut short, corrupted and padded.
//!
//!     BASELINE_SO=<original .so> CANDIDATE_SO=target/deploy/slots.so \
//!         cargo test --test differential -- --ignored --nocapture
//!
//! Instruction data is written here as bytes, not through either build's types: it is the wire
//! format both have to honour.

use std::collections::BTreeMap;
use std::rc::Rc;

use mollusk_svm::program::{
    create_program_account_loader_v2, keyed_account_for_system_program, loader_keys::LOADER_V2,
};
use mollusk_svm::Mollusk;
use solana_account::Account;
use solana_instruction::{error::InstructionError, AccountMeta, Instruction};
use solana_log_collector::LogCollector;
use solana_pubkey::Pubkey;

const RECORDER: &[u8] = include_bytes!("fixtures/cpi_recorder.so");

const PROGRAM: Pubkey = Pubkey::from_str_const("SLoTSdnmBH5KtNJjhEYw1MeWTKAfRnFfQTTcpgwRn2Q");
const VAULT: Pubkey = Pubkey::from_str_const("VAULTrDSUBZ8AXL2kGVYE8eKAn7tgWXRPAevNGUsyTV");
const VRF: Pubkey = Pubkey::from_str_const("Vrf1RNUjXmQGjmQrQLvJHs9SNkvDJEsRVFPkfSQUwGz");
const PERMISSION: Pubkey = Pubkey::from_str_const("ACLseoPoyC3cBqoUtkbjZ4aDrkurZW86v19pXz2XQnp1");
const TOKEN: Pubkey = Pubkey::from_str_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
const DELEGATION: Pubkey = Pubkey::from_str_const("DELeGGvXpWV2fqJUhqcF5ZSYMS4JTLjteaAMARRSaeSh");
const MAGIC: Pubkey = Pubkey::from_str_const("Magic11111111111111111111111111111111111111");
const MAGIC_CONTEXT: Pubkey = Pubkey::from_str_const("MagicContext1111111111111111111111111111111");
const EPHEMERAL_VAULT: Pubkey =
    Pubkey::from_str_const("MagicVau1t999999999999999999999999999999999");
const VAULT_AUTHORITY: Pubkey =
    Pubkey::from_str_const("341xevm3ejTyZCncco8UdEuiagcBbZQtJnEgsDDYBcgs");
const VRF_IDENTITY: Pubkey = Pubkey::from_str_const("9irBy75QS2BN81FUgXuHcjqceJJRuc9oDkAe8TKVvvAw");
const ADMIN: Pubkey = Pubkey::from_str_const("691aFvKMnHXrMSgqk6G8izoCbVZTmkrRcu8xCeMKfPh1");
const SYSTEM: Pubkey = Pubkey::from_str_const("11111111111111111111111111111111");
const SLOT_HASHES: Pubkey = Pubkey::from_str_const("SysvarS1otHashes111111111111111111111111111");

const RECORDED: [Pubkey; 6] = [VAULT, VRF, PERMISSION, TOKEN, DELEGATION, MAGIC];

const SOL: u64 = 1_000_000_000;

// ---------------------------------------------------------------------------------------------
// Wire encoding

fn ix(discriminator: u64) -> Vec<u8> {
    discriminator.to_le_bytes().to_vec()
}

trait Wire {
    fn u8(self, v: u8) -> Self;
    fn u16(self, v: u16) -> Self;
    fn u32(self, v: u32) -> Self;
    fn u64(self, v: u64) -> Self;
    fn key(self, k: &Pubkey) -> Self;
    fn raw(self, b: &[u8]) -> Self;
    fn bytes(self, b: &[u8]) -> Self;
    fn nested(self, v: &[Vec<u8>]) -> Self;
}

impl Wire for Vec<u8> {
    fn u8(mut self, v: u8) -> Self {
        self.push(v);
        self
    }
    fn u16(mut self, v: u16) -> Self {
        self.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn u32(mut self, v: u32) -> Self {
        self.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn u64(mut self, v: u64) -> Self {
        self.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn key(mut self, k: &Pubkey) -> Self {
        self.extend_from_slice(k.as_ref());
        self
    }
    fn raw(mut self, b: &[u8]) -> Self {
        self.extend_from_slice(b);
        self
    }
    fn bytes(self, b: &[u8]) -> Self {
        self.u32(b.len() as u32).raw(b)
    }
    fn nested(self, v: &[Vec<u8>]) -> Self {
        v.iter().fold(self.u32(v.len() as u32), |w, b| w.bytes(b))
    }
}

// ---------------------------------------------------------------------------------------------
// Running a build

#[derive(Debug, PartialEq)]
struct Outcome {
    result: Result<(), InstructionError>,
    logs: Vec<String>,
    return_data: Vec<u8>,
    /// Every account the instruction was given, afterwards. Programs are left out: they are the
    /// builds themselves.
    accounts: Vec<(Pubkey, Account)>,
}

struct Build {
    mollusk: Mollusk,
    compute_units: u64,
    /// What the last run cost.
    last: u64,
}

impl Build {
    fn load(variable: &str) -> Self {
        let path = std::env::var(variable)
            .unwrap_or_else(|_| panic!("{variable} must name the .so to test"));
        let elf = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let mut mollusk = Mollusk::default();
        mollusk.add_program_with_elf_and_loader(&PROGRAM, &elf, &LOADER_V2);
        for id in RECORDED {
            mollusk.add_program_with_elf_and_loader(&id, RECORDER, &LOADER_V2);
        }
        Self { mollusk, compute_units: 0, last: 0 }
    }

    fn run(&mut self, instruction: &Instruction, world: &World) -> Outcome {
        let logger = LogCollector::new_ref_with_limit(None);
        self.mollusk.logger = Some(Rc::clone(&logger));
        let mut keys: Vec<Pubkey> = Vec::new();
        for meta in &instruction.accounts {
            if !keys.contains(&meta.pubkey) {
                keys.push(meta.pubkey);
            }
        }
        let accounts: Vec<(Pubkey, Account)> =
            keys.iter().map(|k| (*k, world.get(k))).collect();
        let result = self.mollusk.process_instruction(instruction, &accounts);
        self.compute_units += result.compute_units_consumed;
        self.last = result.compute_units_consumed;
        let logs = logger
            .borrow()
            .get_recorded_content()
            .iter()
            // The one thing two correct builds may disagree on.
            .filter(|line| !line.contains(" consumed "))
            .cloned()
            .collect();
        Outcome {
            result: result.raw_result,
            logs,
            return_data: result.return_data,
            accounts: result
                .resulting_accounts
                .into_iter()
                .filter(|(_, a)| !a.executable)
                .collect(),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The world the instructions run against

#[derive(Clone, Default)]
struct World(BTreeMap<Pubkey, Account>);

impl World {
    fn get(&self, key: &Pubkey) -> Account {
        self.0.get(key).cloned().unwrap_or_default()
    }

    fn with(&self, key: Pubkey, account: Account) -> Self {
        let mut world = self.clone();
        world.0.insert(key, account);
        world
    }

    fn absorb(&mut self, accounts: &[(Pubkey, Account)]) {
        for (key, account) in accounts {
            self.0.insert(*key, account.clone());
        }
    }
}

fn pda(seeds: &[&[u8]], program: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(seeds, program).0
}

fn wallet(lamports: u64) -> Account {
    Account::new(lamports, 0, &SYSTEM)
}

fn owned(data: Vec<u8>, owner: &Pubkey) -> Account {
    Account { lamports: SOL, data, owner: *owner, executable: false, rent_epoch: 0 }
}

/// A vault ledger holding `lamports` of SOL: a 116-byte header, then slot 0 — the all-zero mint
/// and its amount.
fn ledger_holding(lamports: u64) -> Account {
    let mut data = vec![0u8; 116 + 40];
    data[148..156].copy_from_slice(&lamports.to_le_bytes());
    owned(data, &VAULT)
}

struct Keys {
    user: Pubkey,
    consenter: Pubkey,
    stranger: Pubkey,
    config: Pubkey,
    house: Pubkey,
    analytics: Pubkey,
    identity: Pubkey,
    spin: Pubkey,
    house_ledger: Pubkey,
    admin_ledger: Pubkey,
    receipt: Pubkey,
    analytics_permission: Pubkey,
    house_permission: Pubkey,
    oracle_queue: Pubkey,
    fees_vault: Pubkey,
    reserve: Pubkey,
}

impl Keys {
    fn new() -> Self {
        let user = Pubkey::new_from_array([0x11; 32]);
        let consenter = Pubkey::new_from_array([0x22; 32]);
        let house = pda(&[b"house"], &PROGRAM);
        let analytics = pda(&[b"analytics"], &PROGRAM);
        Self {
            user,
            consenter,
            stranger: Pubkey::new_from_array([0x33; 32]),
            config: pda(&[b"config"], &PROGRAM),
            house,
            analytics,
            identity: pda(&[b"identity"], &PROGRAM),
            spin: pda(&[b"spin", user.as_ref()], &PROGRAM),
            house_ledger: pda(&[b"ledger", house.as_ref()], &VAULT),
            admin_ledger: pda(&[b"ledger", ADMIN.as_ref()], &VAULT),
            receipt: pda(&[b"receipt", PROGRAM.as_ref(), consenter.as_ref()], &VAULT),
            analytics_permission: pda(&[b"permission:", analytics.as_ref()], &PERMISSION),
            house_permission: pda(&[b"permission:", house.as_ref()], &PERMISSION),
            oracle_queue: Pubkey::new_from_array([0x44; 32]),
            fees_vault: Pubkey::new_from_array([0x55; 32]),
            reserve: pda(&[b"vault"], &VAULT),
        }
    }
}

const LINES: u64 = 0;
const HOLD: u64 = 1;
const GAMBLE: u64 = 2;

/// Every strip the same run of symbols, so matching stops on every reel make a winning line.
fn strips(reels: usize) -> Vec<Vec<u8>> {
    vec![(0..20).map(|i| (i % 6) as u8).collect(); reels]
}

fn set_machine(index: u8, mode: u8, rounds: u8, rungs: u8) -> Vec<u8> {
    ix(5)
        .u8(index)
        .u8(mode)
        .u64(100_000_000)
        .u8(3)
        .u8(rounds)
        .u8(rungs)
        .u32(((1u64 << 31) / 100 * 96) as u32)
        .raw(&[0; 32])
        .nested(&strips(3))
        .u32(6)
        .raw(&[100u16, 40, 25, 15, 10, 6].iter().flat_map(|m| [&m.to_le_bytes()[..], &[0]].concat()).collect::<Vec<_>>())
        .nested(&[vec![1, 1, 1], vec![0, 0, 0], vec![2, 2, 2]])
}

fn initialize(k: &Keys, count: u8) -> Instruction {
    Instruction::new_with_bytes(
        PROGRAM,
        &ix(1).u8(count),
        vec![
            AccountMeta::new(ADMIN, true),
            AccountMeta::new(k.config, false),
            AccountMeta::new(k.house, false),
            AccountMeta::new(k.analytics, false),
            AccountMeta::new(k.analytics_permission, false),
            AccountMeta::new_readonly(PERMISSION, false),
            AccountMeta::new_readonly(SYSTEM, false),
        ],
    )
}

fn admin(data: Vec<u8>, accounts: Vec<AccountMeta>) -> Instruction {
    let mut metas = vec![AccountMeta::new(ADMIN, true)];
    metas.extend(accounts);
    Instruction::new_with_bytes(PROGRAM, &data, metas)
}

/// The programs, the admin and the players, and nothing of the game's yet.
fn bare_world() -> World {
    let mut world = World::default();
    for id in RECORDED.iter().chain([&PROGRAM]) {
        world.0.insert(*id, create_program_account_loader_v2(RECORDER));
    }
    let (system, system_account) = keyed_account_for_system_program();
    world.0.insert(system, system_account);
    let k = Keys::new();
    for key in [ADMIN, k.user, k.consenter, k.stranger, VAULT_AUTHORITY, VRF_IDENTITY] {
        world.0.insert(key, wallet(100 * SOL));
    }
    world
}

/// A published shelf — made by the baseline build's own `Initialize` and `SetMachine`, so the
/// bytes are the program's and not this file's idea of them — plus ledgers for the house and
/// the admin.
fn world(baseline: &mut Build) -> World {
    let k = Keys::new();
    let mut world = bare_world();
    let setup = [
        initialize(&k, 3),
        admin(set_machine(0, LINES as u8, 1, 0), vec![AccountMeta::new(k.config, false)]),
        admin(set_machine(1, HOLD as u8, 3, 0), vec![AccountMeta::new(k.config, false)]),
        admin(set_machine(2, GAMBLE as u8, 1, 4), vec![AccountMeta::new(k.config, false)]),
    ];
    for instruction in &setup {
        let outcome = baseline.run(instruction, &world);
        assert_eq!(outcome.result, Ok(()), "setup failed: {:?}", outcome.logs);
        world.absorb(&outcome.accounts);
    }
    world.0.insert(k.house_ledger, ledger_holding(50 * SOL));
    world.0.insert(k.admin_ledger, ledger_holding(0));
    world
}

/// A spin on `machine`, carrying that machine's terms as the shelf holds them.
fn spin(world: &World, machine: u64, status: u64, round: u64, hold: u64, pending: u64, stops: u8) -> Account {
    let k = Keys::new();
    let config = world.get(&k.config).data;
    let stride = (config.len() - 56) / 4;
    let terms = &config[56 + machine as usize * stride..56 + (machine as usize + 1) * stride];
    let data = Vec::new()
        .u64(3)
        .u64(1)
        .key(&k.user)
        .key(&k.consenter)
        .u64(machine)
        .u64(status)
        .u64(round)
        .u64(hold)
        .u64(pending)
        .raw(&[stops; 8])
        .raw(&[7; 32])
        .raw(terms);
    owned(data, &PROGRAM)
}

const BOUGHT: u64 = 0;
const REQUESTED: u64 = 1;
const ROLLED: u64 = 2;

// ---------------------------------------------------------------------------------------------
// The cases

struct Case {
    name: String,
    instruction: Instruction,
    world: World,
}

fn case(name: &str, instruction: Instruction, world: &World) -> Case {
    Case { name: name.into(), instruction, world: world.clone() }
}

fn cases(base: &World) -> Vec<Case> {
    let k = Keys::new();
    let fresh = bare_world();
    let mut all = Vec::new();

    all.push(case("initialize fresh", initialize(&k, 3), &fresh));
    all.push(case("initialize again", initialize(&k, 2), base));
    all.push(case("initialize past capacity", initialize(&k, 9), base));

    let delegate = |seeds: &[Vec<u8>], pda_key: Pubkey| {
        admin(
            ix(2).nested(seeds).key(&k.stranger),
            vec![
                AccountMeta::new(pda_key, false),
                AccountMeta::new_readonly(PROGRAM, false),
                AccountMeta::new(pda(&[b"buffer", pda_key.as_ref()], &PROGRAM), false),
                AccountMeta::new(pda(&[b"delegation", pda_key.as_ref()], &DELEGATION), false),
                AccountMeta::new(pda(&[b"delegation-metadata", pda_key.as_ref()], &DELEGATION), false),
                AccountMeta::new_readonly(DELEGATION, false),
                AccountMeta::new_readonly(SYSTEM, false),
            ],
        )
    };
    all.push(case("delegate house", delegate(&[b"house".to_vec()], k.house), base));
    all.push(case("delegate analytics", delegate(&[b"analytics".to_vec()], k.analytics), base));

    let undelegate = |tag: [u8; 8]| {
        Instruction::new_with_bytes(
            PROGRAM,
            &tag.to_vec().nested(&[b"house".to_vec()]),
            vec![
                AccountMeta::new(k.house, false),
                // The delegation program signs for the buffer when it calls back.
                AccountMeta::new(pda(&[b"buffer", k.house.as_ref()], &PROGRAM), true),
                AccountMeta::new(ADMIN, true),
                AccountMeta::new_readonly(SYSTEM, false),
            ],
        )
    };
    let buffered = base
        .with(pda(&[b"buffer", k.house.as_ref()], &PROGRAM), owned(vec![0; 0], &DELEGATION))
        .with(k.house, wallet(SOL));
    all.push(case("undelegate by number", undelegate(3u64.to_le_bytes()), &buffered));
    // Nothing left where the house was: recreated from scratch, rent exempt — the rent sysvar's
    // arithmetic is on the line here.
    all.push(case(
        "undelegate into an empty address",
        undelegate(3u64.to_le_bytes()),
        &buffered.with(k.house, Account::default()),
    ));
    all.push(case(
        "undelegate by the delegation program's tag",
        undelegate([196, 28, 41, 206, 48, 37, 51, 167]),
        &buffered,
    ));

    all.push(case(
        "request undelegation",
        admin(
            ix(4),
            vec![
                AccountMeta::new(k.house, false),
                AccountMeta::new(MAGIC_CONTEXT, false),
                AccountMeta::new_readonly(MAGIC, false),
                AccountMeta::new(k.fees_vault, false),
            ],
        ),
        base,
    ));

    all.push(case(
        "set machine",
        admin(set_machine(3, HOLD as u8, 2, 0), vec![AccountMeta::new(k.config, false)]),
        base,
    ));
    all.push(case(
        "set machine nonsense",
        admin(set_machine(0, 9, 1, 0), vec![AccountMeta::new(k.config, false)]),
        base,
    ));
    all.push(case(
        "set machine past capacity",
        admin(set_machine(7, LINES as u8, 1, 0), vec![AccountMeta::new(k.config, false)]),
        base,
    ));

    for add in [0u16, 1, 4] {
        all.push(case(
            &format!("grow config by {add}"),
            admin(
                ix(6).u16(add),
                vec![AccountMeta::new(k.config, false), AccountMeta::new_readonly(SYSTEM, false)],
            ),
            base,
        ));
    }

    all.push(case(
        "close spin",
        admin(
            ix(7).key(&k.user),
            vec![
                AccountMeta::new(k.house, false),
                AccountMeta::new(k.spin, false),
                AccountMeta::new(EPHEMERAL_VAULT, false),
                AccountMeta::new_readonly(MAGIC, false),
            ],
        ),
        &base.with(k.spin, spin(base, HOLD, ROLLED, 1, 0, 0, 3)),
    ));

    for which in [0u8, 1] {
        all.push(case(
            &format!("open ledger {which}"),
            admin(
                ix(8).u8(which).u16(4),
                vec![
                    AccountMeta::new(k.house, false),
                    AccountMeta::new(k.house_ledger, false),
                    AccountMeta::new(k.house_permission, false),
                    AccountMeta::new_readonly(PERMISSION, false),
                    AccountMeta::new_readonly(VAULT, false),
                    AccountMeta::new_readonly(SYSTEM, false),
                ],
            ),
            base,
        ));
        all.push(case(
            &format!("delegate treasury {which}"),
            admin(
                ix(9).u8(which).key(&k.stranger),
                vec![
                    AccountMeta::new(k.house, false),
                    AccountMeta::new(pda(&[b"buffer", k.house_ledger.as_ref()], &VAULT), false),
                    AccountMeta::new(pda(&[b"delegation", k.house_ledger.as_ref()], &DELEGATION), false),
                    AccountMeta::new(pda(&[b"delegation-metadata", k.house_ledger.as_ref()], &DELEGATION), false),
                    AccountMeta::new(k.house_ledger, false),
                    AccountMeta::new_readonly(VAULT, false),
                    AccountMeta::new_readonly(DELEGATION, false),
                    AccountMeta::new_readonly(SYSTEM, false),
                ],
            ),
            base,
        ));
        all.push(case(
            &format!("undelegate treasury {which}"),
            admin(
                ix(10).u8(which),
                vec![
                    AccountMeta::new(k.house, false),
                    AccountMeta::new(k.house_ledger, false),
                    AccountMeta::new_readonly(VAULT, false),
                    AccountMeta::new_readonly(MAGIC, false),
                    AccountMeta::new(MAGIC_CONTEXT, false),
                    AccountMeta::new(k.fees_vault, false),
                ],
            ),
            base,
        ));
        all.push(case(
            &format!("close ledger {which} with token pairs"),
            admin(
                ix(11).u8(which),
                vec![
                    AccountMeta::new(k.house, false),
                    AccountMeta::new(k.house_ledger, false),
                    AccountMeta::new(k.reserve, false),
                    AccountMeta::new(k.house_permission, false),
                    AccountMeta::new_readonly(PERMISSION, false),
                    AccountMeta::new_readonly(VAULT, false),
                    AccountMeta::new_readonly(TOKEN, false),
                    AccountMeta::new_readonly(SYSTEM, false),
                    AccountMeta::new(Pubkey::new_from_array([0x61; 32]), false),
                    AccountMeta::new(Pubkey::new_from_array([0x62; 32]), false),
                ],
            ),
            base,
        ));
        all.push(case(
            &format!("authorize treasury {which}"),
            admin(
                ix(12).u8(which),
                vec![
                    AccountMeta::new(k.house, false),
                    AccountMeta::new(k.house_ledger, false),
                    AccountMeta::new_readonly(VAULT, false),
                ],
            ),
            base,
        ));
        for public in [0u8, 1] {
            all.push(case(
                &format!("set privacy {which} public={public}"),
                admin(
                    ix(13).u8(which).u8(public),
                    vec![
                        AccountMeta::new(k.house, false),
                        AccountMeta::new(k.house_ledger, false),
                        AccountMeta::new(k.house_permission, false),
                        AccountMeta::new_readonly(PERMISSION, false),
                        AccountMeta::new_readonly(VAULT, false),
                        AccountMeta::new_readonly(SYSTEM, false),
                    ],
                ),
                base,
            ));
        }
    }

    all.push(case(
        "withdraw house",
        admin(
            ix(14).key(&Pubkey::default()).u64(SOL),
            vec![
                AccountMeta::new(k.house, false),
                AccountMeta::new(k.house_ledger, false),
                AccountMeta::new(k.admin_ledger, false),
                AccountMeta::new_readonly(VAULT, false),
            ],
        ),
        base,
    ));

    let request_bet = |machine: u64| {
        Instruction::new_with_bytes(
            PROGRAM,
            &ix(16).u64(machine),
            vec![
                AccountMeta::new_readonly(k.consenter, true),
                AccountMeta::new_readonly(k.user, false),
                AccountMeta::new_readonly(k.config, false),
                AccountMeta::new(k.house, false),
                AccountMeta::new(k.receipt, false),
                AccountMeta::new(EPHEMERAL_VAULT, false),
                AccountMeta::new_readonly(MAGIC, false),
                AccountMeta::new_readonly(VAULT, false),
                AccountMeta::new(k.house_ledger, false),
                AccountMeta::new(MAGIC_CONTEXT, false),
            ],
        )
    };
    for machine in [LINES, HOLD, GAMBLE, 3, 99] {
        all.push(case(&format!("request bet on {machine}"), request_bet(machine), base));
    }

    let resolve_bet = |machine: u64| {
        Instruction::new_with_bytes(
            PROGRAM,
            &ix(17).key(&k.user).u64(machine).key(&k.consenter),
            vec![
                AccountMeta::new_readonly(k.receipt, false),
                AccountMeta::new_readonly(VAULT_AUTHORITY, true),
                AccountMeta::new_readonly(k.config, false),
                AccountMeta::new(k.house, false),
                AccountMeta::new(k.spin, false),
                AccountMeta::new(EPHEMERAL_VAULT, false),
                AccountMeta::new_readonly(MAGIC, false),
                AccountMeta::new(k.analytics, false),
            ],
        )
    };
    // Where the spin will be: unclaimed, and the magic program's to create — see cpi-recorder.
    let unclaimed = base.with(k.spin, Account::new(SOL, 0, &MAGIC));
    for machine in [LINES, HOLD, GAMBLE] {
        all.push(case(&format!("resolve bet on {machine}"), resolve_bet(machine), &unclaimed));
    }
    all.push(case("resolve bet with no spin to create", resolve_bet(HOLD), base));
    all.push(case("resolve bet with a spin live", resolve_bet(HOLD), &base.with(k.spin, spin(base, HOLD, BOUGHT, 0, 0, 0, 0))));
    all.push(case("resolve bet on no machine", resolve_bet(99), &unclaimed));

    let request_reveal = || {
        Instruction::new_with_bytes(
            PROGRAM,
            &ix(18),
            vec![
                AccountMeta::new_readonly(k.user, false),
                AccountMeta::new(k.house, false),
                AccountMeta::new(k.spin, false),
                AccountMeta::new_readonly(k.identity, false),
                AccountMeta::new(k.oracle_queue, false),
                AccountMeta::new_readonly(SLOT_HASHES, false),
                AccountMeta::new_readonly(SYSTEM, false),
                AccountMeta::new_readonly(VRF, false),
            ],
        )
    };
    for (label, status, round) in [("bought", BOUGHT, 0), ("requested", REQUESTED, 1), ("rolled", ROLLED, 0)] {
        all.push(case(
            &format!("request reveal on a {label} spin"),
            request_reveal(),
            &base.with(k.spin, spin(base, HOLD, status, round, 0, 0, 0)),
        ));
    }

    let callback = |round: u64, extra: &[u8]| {
        Instruction::new_with_bytes(
            PROGRAM,
            &ix(19).raw(&[9; 32]).u64(round).raw(extra),
            vec![AccountMeta::new_readonly(VRF_IDENTITY, true), AccountMeta::new(k.spin, false)],
        )
    };
    let requested = base.with(k.spin, spin(base, HOLD, REQUESTED, 1, 0, 0, 0));
    all.push(case("oracle answers", callback(1, &[]), &requested));
    all.push(case("oracle answers with extra bytes", callback(1, &[0xAA; 8]), &requested));
    all.push(case("oracle answers late", callback(0, &[]), &requested));

    let decide = |discriminator: u64, signer: Pubkey, arguments: &[u8]| {
        Instruction::new_with_bytes(
            PROGRAM,
            &ix(discriminator).raw(arguments),
            vec![AccountMeta::new_readonly(signer, true), AccountMeta::new(k.spin, false)],
        )
    };
    for (label, machine, round) in [("hold machine", HOLD, 0), ("hold machine last grid", HOLD, 2), ("lines machine", LINES, 0)] {
        for mask in [0u8, 0b011, 0b1000] {
            all.push(case(
                &format!("hold {mask:#05b} on {label}"),
                decide(20, k.consenter, &[mask]),
                &base.with(k.spin, spin(base, machine, ROLLED, round, 0, 0, 2)),
            ));
        }
    }
    all.push(case("hold by the player", decide(20, k.user, &[1]), &base.with(k.spin, spin(base, HOLD, ROLLED, 0, 0, 0, 2))));
    all.push(case("hold by a stranger", decide(20, k.stranger, &[1]), &base.with(k.spin, spin(base, HOLD, ROLLED, 0, 0, 0, 2))));
    for (label, round, pending, stops) in [("winning base", 0, 0, 2), ("losing base", 0, 0, 7), ("on a rung", 1, 20_000_000, 2), ("at the top", 4, 20_000_000, 2)] {
        all.push(case(
            &format!("gamble {label}"),
            decide(21, k.consenter, &[]),
            &base.with(k.spin, spin(base, GAMBLE, ROLLED, round, 0, pending, stops)),
        ));
    }
    all.push(case("gamble on a hold machine", decide(21, k.consenter, &[]), &base.with(k.spin, spin(base, HOLD, ROLLED, 0, 0, 0, 2))));

    let request_collect = |signer: Pubkey| {
        Instruction::new_with_bytes(
            PROGRAM,
            &ix(22),
            vec![
                AccountMeta::new_readonly(k.user, false),
                AccountMeta::new(k.house, false),
                AccountMeta::new(k.spin, false),
                AccountMeta::new(pda(&[b"receipt", PROGRAM.as_ref(), signer.as_ref()], &VAULT), false),
                AccountMeta::new(EPHEMERAL_VAULT, false),
                AccountMeta::new_readonly(MAGIC, false),
                AccountMeta::new_readonly(VAULT, false),
                AccountMeta::new_readonly(signer, true),
                AccountMeta::new(k.house_ledger, false),
                AccountMeta::new(MAGIC_CONTEXT, false),
            ],
        )
    };
    for (label, machine, round, pending, stops) in [
        ("a winning line", LINES, 0, 0, 2),
        ("a losing line", LINES, 0, 0, 7),
        ("mid hold", HOLD, 1, 0, 2),
        ("the last hold grid", HOLD, 2, 0, 2),
        ("a gamble rung", GAMBLE, 2, 40_000_000, 2),
    ] {
        let world = base.with(k.spin, spin(base, machine, ROLLED, round, 0, pending, stops));
        all.push(case(&format!("collect {label}"), request_collect(k.consenter), &world));
        all.push(case(&format!("collect {label} by the player"), request_collect(k.user), &world));
    }
    all.push(case(
        "collect before the grid",
        request_collect(k.consenter),
        &base.with(k.spin, spin(base, LINES, REQUESTED, 0, 0, 0, 2)),
    ));

    let resolve_collect = Instruction::new_with_bytes(
        PROGRAM,
        &ix(23).key(&k.user),
        vec![
            AccountMeta::new_readonly(k.receipt, false),
            AccountMeta::new_readonly(VAULT_AUTHORITY, true),
            AccountMeta::new(k.house, false),
            AccountMeta::new(k.spin, false),
            AccountMeta::new(EPHEMERAL_VAULT, false),
            AccountMeta::new_readonly(MAGIC, false),
            AccountMeta::new(k.analytics, false),
        ],
    );
    for (label, machine, stops) in [("a win", LINES, 2), ("a loss", LINES, 7), ("a gamble", GAMBLE, 2)] {
        all.push(case(
            &format!("resolve collect of {label}"),
            resolve_collect.clone(),
            &base.with(k.spin, spin(base, machine, ROLLED, 0, 0, 0, stops)),
        ));
    }

    for unused in [0u64, 15, 24, 255, u64::MAX] {
        all.push(case(
            &format!("discriminator {unused}"),
            Instruction::new_with_bytes(PROGRAM, &ix(unused), vec![AccountMeta::new(ADMIN, true)]),
            base,
        ));
    }
    all.push(case("no instruction data", Instruction::new_with_bytes(PROGRAM, &[], vec![]), base));

    all
}

/// The case as written, then every way of getting it slightly wrong.
fn variants(case: &Case) -> Vec<(String, Instruction)> {
    let base = &case.instruction;
    let mut out = vec![("as written".to_string(), base.clone())];
    for (i, meta) in base.accounts.iter().enumerate() {
        if meta.is_signer {
            let mut v = base.clone();
            v.accounts[i].is_signer = false;
            out.push((format!("account {i} unsigned"), v));
        }
        let mut v = base.clone();
        v.accounts[i].pubkey = Pubkey::new_from_array([0xE0 | i as u8; 32]);
        out.push((format!("account {i} a stranger"), v));
    }
    for len in 0..base.accounts.len() {
        let mut v = base.clone();
        v.accounts.truncate(len);
        out.push((format!("only {len} accounts"), v));
    }
    if base.data.len() > 8 {
        let mut v = base.clone();
        v.data.truncate(8);
        out.push(("no arguments".into(), v));
        let mut v = base.clone();
        v.data.pop();
        out.push(("arguments cut short".into(), v));
        let mut v = base.clone();
        *v.data.last_mut().unwrap() ^= 0xFF;
        out.push(("last argument byte flipped".into(), v));
    }
    let mut v = base.clone();
    v.data.extend_from_slice(&[0xAB; 7]);
    out.push(("arguments padded".into(), v));
    out
}

#[test]
#[ignore = "needs BASELINE_SO and CANDIDATE_SO"]
fn both_builds_do_exactly_the_same() {
    let mut baseline = Build::load("BASELINE_SO");
    let mut candidate = Build::load("CANDIDATE_SO");
    let base = world(&mut baseline);
    baseline.compute_units = 0;
    let cases = cases(&base);
    if std::env::var("COVERAGE").is_ok() {
        for case in &cases {
            let outcome = baseline.run(&case.instruction, &case.world);
            let calls = outcome.logs.iter().filter(|l| l.starts_with("Program data:")).count();
            println!("{:<48} {:?} ({calls} calls)", case.name, outcome.result);
        }
        baseline.compute_units = 0;
    }

    let mut runs = 0;
    let mut succeeded = 0;
    let mut differences = Vec::new();
    let report = std::env::var("CU_REPORT").is_ok();
    for case in &cases {
        for (variant, instruction) in variants(case) {
            let expected = baseline.run(&instruction, &case.world);
            let actual = candidate.run(&instruction, &case.world);
            if report && variant == "as written" {
                println!("CU {:<48} {:>8} {:>8}", case.name, baseline.last, candidate.last);
            }
            runs += 1;
            succeeded += usize::from(expected.result.is_ok());
            if expected != actual {
                differences.push(format!(
                    "{} / {variant}\n  baseline:  {:?}\n  candidate: {:?}",
                    case.name, expected, actual
                ));
            }
        }
    }

    println!(
        "{} cases, {runs} runs ({succeeded} succeeded on the baseline), {} differences",
        cases.len(),
        differences.len()
    );
    println!(
        "compute units: baseline {}, candidate {}",
        baseline.compute_units, candidate.compute_units
    );
    for difference in differences.iter().take(5) {
        println!("\n{difference}");
    }
    assert!(differences.is_empty(), "{} runs differ", differences.len());
}
