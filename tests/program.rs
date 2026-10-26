//! The built program, run. `layout.rs` pins what the bytes mean; these check that the deployed
//! artifact dispatches and refuses exactly as it always has — the same accounts in the same order,
//! the same errors for the same mistakes. They load `target/deploy/slots.so`, so they need
//! `cargo build-sbf` first, and are ignored otherwise:
//!
//!     cargo build-sbf && cargo test --test program -- --ignored
//!
//! Point `SBF_OUT_DIR` at another build to run the same checks against it.
//!
//! Compute is printed with `--nocapture`; the README keeps the numbers.

use std::cell::RefCell;

use bytemuck::Zeroable;
use casino_core::observe::{self, Observable};
use casino_core::referral::Referral;
use mollusk_svm::program::{create_keyed_account_for_builtin_program, keyed_account_for_system_program, Builtin};
use mollusk_svm::Mollusk;
use pinocchio::error::ProgramError;
use solana_account::{Account, ReadableAccount, WritableAccount};
use solana_instruction::{error::InstructionError, AccountMeta, Instruction};
use solana_program_runtime::declare_process_instruction;
use solana_program_runtime::invoke_context::BuiltinFunctionRegisterer;
use solana_program_runtime::solana_sbpf::program::BuiltinFunctionDefinition;
use solana_pubkey::Pubkey;

use slots::error::GameError;
use slots::instructions::set_machine::{InitSymbol, SetMachine};
use slots::state::analytics::{self, Analytics};
use slots::state::config::{self, Config, MachineConfig, GAMBLE_FAIR, MODE_GAMBLE, MODE_HOLD, MODE_LINES};
use slots::state::spin::{self, Spin, SpinStatus};

fn key(bytes: [u8; 32]) -> Pubkey {
    Pubkey::new_from_array(bytes)
}

fn program() -> Pubkey {
    key(slots::ID.to_bytes())
}

fn mollusk() -> Mollusk {
    Mollusk::new(&program(), "slots")
}

/// A three-reel hold machine with three grids.
fn terms() -> MachineConfig {
    machine(MODE_HOLD)
}

/// The same strips in `mode`: three grids to hold between, or three rungs to gamble up.
fn machine(mode: u8) -> MachineConfig {
    let strip: Vec<u8> = (0..20).map(|i| (i % 6) as u8).collect();
    SetMachine {
        index: 0,
        mode,
        stake_lamports: 100_000_000,
        row_count: 3,
        rounds: 3,
        gamble_rungs: 3,
        gamble_win: GAMBLE_FAIR,
        mint: [0; 32],
        strips: vec![strip.clone(), strip.clone(), strip],
        symbols: [100u16, 40, 25, 15, 10, 6]
            .iter()
            .map(|&mult| InitSymbol { mult, flags: 0 })
            .collect(),
        lines: vec![vec![1, 1, 1]],
        shown_in: 0,
        runs: None,
    }
    .build_for_test()
}

struct Table {
    user: Pubkey,
    consenter: Pubkey,
    spin: Pubkey,
}

impl Table {
    fn new() -> Self {
        let user = Pubkey::new_unique();
        let (spin, _) =
            Pubkey::find_program_address(&[b"spin", user.as_ref()], &program());
        Self {
            user,
            consenter: Pubkey::new_unique(),
            spin,
        }
    }

    /// The same player every run, for the tests that print compute: the PDA searches the program
    /// makes cost about 1,500 CU per bump they try, so a fresh key would move the numbers.
    fn fixed() -> Self {
        let user = key([1; 32]);
        let (spin, _) = Pubkey::find_program_address(&[b"spin", user.as_ref()], &program());
        Self { user, consenter: key([2; 32]), spin }
    }

    fn spin_account(&self, status: SpinStatus, round: u64) -> Account {
        let mut s = Spin::zeroed();
        s.discriminator = spin::DISCRIMINATOR;
        s.version = spin::VERSION;
        s.user = self.user.to_bytes();
        s.consenter = self.consenter.to_bytes();
        s.status = status as u64;
        s.round = round;
        s.seed = [7; 32];
        let mut data = bytemuck::bytes_of(&s).to_vec();
        data.extend_from_slice(bytemuck::bytes_of(&terms()));
        Account {
            lamports: 1_000_000_000,
            data,
            owner: program(),
            executable: false,
            rent_epoch: 0,
        }
    }

    fn spin_after(&self, accounts: &[(Pubkey, Account)]) -> Spin {
        let (_, account) = accounts.iter().find(|(k, _)| *k == self.spin).unwrap();
        *bytemuck::from_bytes::<Spin>(&account.data[..Spin::SIZE])
    }
}

fn wallet() -> Account {
    Account::new(1_000_000_000, 0, &Pubkey::default())
}

fn hold(signer: Pubkey, signed: bool, spin: Pubkey, mask: u8) -> Instruction {
    let mut data = 20u64.to_le_bytes().to_vec();
    data.push(mask);
    Instruction::new_with_bytes(
        program(),
        &data,
        vec![
            AccountMeta { pubkey: signer, is_signer: signed, is_writable: false },
            AccountMeta::new(spin, false),
        ],
    )
}

fn custom(error: GameError) -> Result<(), InstructionError> {
    Err(InstructionError::Custom(error as u32))
}

fn core(error: casino_core::CoreError) -> Result<(), InstructionError> {
    Err(InstructionError::Custom(error as u32))
}

fn failure(error: InstructionError) -> Result<(), InstructionError> {
    Err(error)
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn a_hold_applies_the_seen_grid_and_commits_the_mask() {
    let table = Table::new();
    let result = mollusk().process_instruction(
        &hold(table.user, true, table.spin, 0b011),
        &[
            (table.user, wallet()),
            (table.spin, table.spin_account(SpinStatus::Rolled, 0)),
        ],
    );
    assert_eq!(result.raw_result, Ok(()));
    let after = table.spin_after(&result.resulting_accounts);
    assert_eq!(after.round, 1);
    assert_eq!(after.hold, 0b011);
    assert_eq!(after.status, SpinStatus::Bought as u64);
    assert_eq!(after.seed, [0; 32], "the seen seed must be spent");
    println!("hold: {} CU", result.compute_units_consumed);
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn the_session_key_may_hold_for_the_player() {
    let table = Table::new();
    let result = mollusk().process_instruction(
        &hold(table.consenter, true, table.spin, 0b001),
        &[
            (table.consenter, wallet()),
            (table.spin, table.spin_account(SpinStatus::Rolled, 0)),
        ],
    );
    assert_eq!(result.raw_result, Ok(()));
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn a_stranger_cannot_hold_someone_elses_spin() {
    let table = Table::new();
    let stranger = Pubkey::new_unique();
    let result = mollusk().process_instruction(
        &hold(stranger, true, table.spin, 0b001),
        &[
            (stranger, wallet()),
            (table.spin, table.spin_account(SpinStatus::Rolled, 0)),
        ],
    );
    assert_eq!(result.raw_result, core(casino_core::CoreError::Unauthorized));
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn an_unsigned_hold_is_refused_for_its_signature() {
    let table = Table::new();
    let result = mollusk().process_instruction(
        &hold(table.user, false, table.spin, 0b001),
        &[
            (table.user, wallet()),
            (table.spin, table.spin_account(SpinStatus::Rolled, 0)),
        ],
    );
    assert_eq!(result.raw_result, failure(InstructionError::MissingRequiredSignature));
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn a_hold_before_the_grid_is_seen_is_refused() {
    let table = Table::new();
    let result = mollusk().process_instruction(
        &hold(table.user, true, table.spin, 0b001),
        &[
            (table.user, wallet()),
            (table.spin, table.spin_account(SpinStatus::Requested, 0)),
        ],
    );
    assert_eq!(result.raw_result, custom(GameError::NotRolled));
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn a_hold_missing_its_spin_is_refused_for_the_account() {
    let table = Table::new();
    let mut instruction = hold(table.user, true, table.spin, 0b001);
    instruction.accounts.truncate(1);
    let result = mollusk().process_instruction(&instruction, &[(table.user, wallet())]);
    // The runtime still reports this code as the deprecated `NotEnoughAccountKeys`, and a
    // program has no way to return `MissingAccount`, so it is checked as the program raised it.
    let error = result.raw_result.unwrap_err();
    assert_eq!(ProgramError::try_from(error), Ok(ProgramError::NotEnoughAccountKeys));
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn numbers_nothing_answers_to_are_invalid_data() {
    let table = Table::new();
    for unused in [0u64, 15, 25] {
        let mut instruction = hold(table.user, true, table.spin, 0b001);
        instruction.data[..8].copy_from_slice(&unused.to_le_bytes());
        let result = mollusk().process_instruction(
            &instruction,
            &[
                (table.user, wallet()),
                (table.spin, table.spin_account(SpinStatus::Rolled, 0)),
            ],
        );
        assert_eq!(
            result.raw_result,
            failure(InstructionError::InvalidInstructionData),
            "{unused} was answered"
        );
    }
}

fn reveal(table: &Table, randomness: [u8; 32], round: u64, extra: &[u8]) -> Instruction {
    let data = [&19u64.to_le_bytes()[..], &randomness, &round.to_le_bytes(), &0u64.to_le_bytes(), extra].concat();
    Instruction::new_with_bytes(
        program(),
        &data,
        vec![
            AccountMeta::new_readonly(key(casino_core::vrf::scoped_identity(&slots::ID).to_bytes()), true),
            AccountMeta::new(table.spin, false),
        ],
    )
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn the_oracle_lands_its_seed_on_the_round_it_was_asked_for() {
    let table = Table::new();
    let identity = key(casino_core::vrf::scoped_identity(&slots::ID).to_bytes());
    let result = mollusk().process_instruction(
        // Whatever the oracle appends after the round is not the program's business.
        &reveal(&table, [9; 32], 1, &[0xAA; 8]),
        &[
            (identity, wallet()),
            (table.spin, table.spin_account(SpinStatus::Requested, 1)),
        ],
    );
    assert_eq!(result.raw_result, Ok(()));
    let after = table.spin_after(&result.resulting_accounts);
    assert_eq!(after.seed, [9; 32]);
    assert_eq!(after.status, SpinStatus::Rolled as u64);
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn a_late_answer_for_an_earlier_round_is_refused() {
    let table = Table::new();
    let identity = key(casino_core::vrf::scoped_identity(&slots::ID).to_bytes());
    let result = mollusk().process_instruction(
        &reveal(&table, [9; 32], 0, &[]),
        &[
            (identity, wallet()),
            (table.spin, table.spin_account(SpinStatus::Requested, 1)),
        ],
    );
    assert_eq!(result.raw_result, core(casino_core::CoreError::WrongStatus));
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn permission_upgrade_checks_the_permission_even_when_the_spin_is_absent() {
    let table = Table::new();
    let house = Pubkey::find_program_address(&[b"house"], &program()).0;
    let acl = key(casino_core::ids::PERMISSION_PROGRAM.to_bytes());
    let permission = Pubkey::new_unique();
    let vault = Pubkey::new_unique();
    let magic = Pubkey::new_unique();
    let instruction = Instruction::new_with_bytes(program(), &24u64.to_le_bytes(), vec![
        AccountMeta::new_readonly(table.user, false),
        AccountMeta::new_readonly(table.spin, false),
        AccountMeta::new(permission, false),
        AccountMeta::new(house, false),
        AccountMeta::new(vault, false),
        AccountMeta::new_readonly(magic, false),
        AccountMeta::new_readonly(acl, false),
    ]);
    let mut accounts = vec![
        (table.user, wallet()), (table.spin, Account::default()),
        (permission, Account::default()), (house, wallet()),
        (vault, wallet()), (magic, wallet()), (acl, wallet()),
    ];
    assert_eq!(mollusk().process_instruction(&instruction, &accounts).raw_result, Ok(()));
    accounts[2].1 = Account { data: vec![0; 134], owner: acl, ..wallet() };
    assert_eq!(mollusk().process_instruction(&instruction, &accounts).raw_result,
        failure(InstructionError::InvalidSeeds));
}

#[test]
fn new_spin_permissions_include_the_floor_and_vault() {
    let human = casino_core::chain::Pubkey::new_from_array([42; 32]);
    let members = slots::instructions::resolve_bet::spin_members(&human);
    assert!(members.contains(&human));
    assert!(members.contains(&slots::constants::PRIVATE_CASINO));
    assert!(members.contains(&casino_core::ids::VAULT_PROGRAM));
}

// ── the casino floor's trailer ─────────────────────────────────────────────────────────────
//
// What a spin shows the floor: the last `observe::SIZE` bytes, written by the bet, the reveal,
// the round decisions and the collect. The settle and reveal callbacks call into MagicBlock, which
// is not here, so stand-ins take the Magic program's and the VRF's addresses and record what they
// were asked. The Magic stand-in also does what the rollup would to the account — creates, resizes
// and closes it — which only the real Magic program may do to an account another program owns,
// so it reaches past the runtime's checks to do it.

const SOL: u64 = 1_000_000_000;

fn fixed(k: casino_core::chain::Pubkey) -> Pubkey {
    key(k.to_bytes())
}

fn pda(seeds: &[&[u8]]) -> Pubkey {
    Pubkey::find_program_address(seeds, &program()).0
}

fn magic() -> Pubkey {
    fixed(casino_core::ids::MAGIC_PROGRAM_ID)
}

fn vrf_program() -> Pubkey {
    fixed(casino_core::ids::VRF_PROGRAM)
}

/// What each stand-in charges: a builtin must consume something. The system program's figure; the
/// compute printed below is the program's own plus this per call.
const STAND_IN_CU: u64 = 150;

thread_local! {
    /// Every call a stand-in took: the program it stood in for, and the instruction data.
    static CALLS: RefCell<Vec<(Pubkey, Vec<u8>)>> = const { RefCell::new(Vec::new()) };
}

fn record(target: Pubkey, data: &[u8]) {
    CALLS.with(|c| c.borrow_mut().push((target, data.to_vec())));
}

/// The tag of every call the Magic stand-in took, in order: 12 create, 13 resize, 14 close,
/// 6 schedule a task.
fn magic_calls() -> Vec<u32> {
    CALLS.with(|c| c.borrow().iter().filter(|(p, _)| *p == magic()).map(|(_, d)| u32::from_le_bytes(d[..4].try_into().unwrap())).collect())
}

fn calls_to(target: Pubkey) -> Vec<Vec<u8>> {
    CALLS.with(|c| c.borrow().iter().filter(|(p, _)| *p == target).map(|(_, d)| d.clone()).collect())
}

// The Magic program's ephemeral-account calls are a u32 tag, then (create, resize) the size; the
// account is the second one. A closed account goes back to the Magic program, as the rollup
// leaves an address it may create again.
declare_process_instruction!(FakeMagic, STAND_IN_CU, |invoke_context| {
    let transaction = &invoke_context.transaction_context;
    let context = transaction.get_current_instruction_context()?;
    let data = context.get_instruction_data().to_vec();
    record(magic(), &data);
    let tag = u32::from_le_bytes(data[..4].try_into().unwrap());
    let size = || u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
    match tag {
        12 => {
            let mut ephemeral = context.try_borrow_instruction_account(1)?;
            ephemeral.set_data_length(size())?;
            ephemeral.set_owner(program().as_ref())?;
        }
        13 | 14 => {
            let index = context.get_index_of_instruction_account_in_transaction(1)?;
            let mut ephemeral = transaction.accounts().try_borrow_mut(index)?;
            if tag == 13 {
                let mut grown = ephemeral.data().to_vec();
                grown.resize(size(), 0);
                ephemeral.set_data_from_slice(&grown);
            } else {
                ephemeral.set_data_from_slice(&[]);
                ephemeral.set_owner(magic());
            }
        }
        _ => {}
    }
    Ok(())
});

// The VRF: any request is taken.
declare_process_instruction!(FakeVrf, STAND_IN_CU, |invoke_context| {
    let context = invoke_context.transaction_context.get_current_instruction_context()?;
    record(vrf_program(), context.get_instruction_data());
    Ok(())
});

fn vault() -> Pubkey {
    fixed(casino_core::ids::VAULT_PROGRAM)
}

thread_local! {
    /// The accounts of every call the vault stand-in took: key, signer, writable.
    static VAULT_ACCOUNTS: RefCell<Vec<Vec<(Pubkey, bool, bool)>>> = const { RefCell::new(Vec::new()) };
}

// The vault: a receipt is taken as asked, with who signed it kept.
declare_process_instruction!(FakeVault, STAND_IN_CU, |invoke_context| {
    let context = invoke_context.transaction_context.get_current_instruction_context()?;
    record(vault(), context.get_instruction_data());
    let mut accounts = Vec::new();
    for i in 0..context.get_number_of_instruction_accounts() {
        let key = Pubkey::new_from_array(context.get_key_of_instruction_account(i)?.to_bytes());
        accounts.push((key, context.is_instruction_account_signer(i)?, context.is_instruction_account_writable(i)?));
    }
    VAULT_ACCOUNTS.with(|v| v.borrow_mut().push(accounts));
    Ok(())
});

/// The program with the stand-ins at their addresses, and nothing recorded yet.
fn with_stand_ins() -> Mollusk {
    CALLS.with(|c| c.borrow_mut().clear());
    VAULT_ACCOUNTS.with(|v| v.borrow_mut().clear());
    let mut m = mollusk();
    let fakes: [(Pubkey, &'static str, BuiltinFunctionRegisterer); 3] = [
        (magic(), "fake_magic", FakeMagic::register),
        (vrf_program(), "fake_vrf", FakeVrf::register),
        (vault(), "fake_vault", FakeVault::register),
    ];
    for (program_id, name, register_fn) in fakes {
        m.program_cache.add_builtin(Builtin { program_id, name, register_fn });
    }
    m
}

fn builtin(id: Pubkey, name: &str) -> Account {
    create_keyed_account_for_builtin_program(&id, name).1
}

/// Mollusk carries no Magic program, so an instruction that reaches for the scheduler fails at
/// that CPI and nowhere earlier.
fn reached_the_scheduler(result: &mollusk_svm::result::InstructionResult) -> bool {
    result.program_result != mollusk_svm::result::ProgramResult::Success
        && result.raw_result.as_ref().is_err_and(|e| format!("{e:?}").contains("UnsupportedProgramId"))
}

fn owned(data: Vec<u8>) -> Account {
    Account { lamports: SOL, data, owner: program(), executable: false, rent_epoch: 0 }
}

/// A floor station: any account the floor program owns, with data.
fn station() -> Account {
    Account { lamports: SOL, data: vec![6; 112], owner: fixed(casino_core::ids::FLOOR), executable: false, rent_epoch: 0 }
}

/// A trailer as a bet leaves it, watched by `observer`.
fn trailer(status: u8, observer: Pubkey, result: [u8; 64]) -> Observable {
    Observable { status, generation: 5, config_id: 0, observer: observer.to_bytes(), result, ..Observable::new(observe::KIND_SLOTS) }
}

/// The machine's stake, which is a slot bet's whole stake.
const STAKE: u64 = 100_000_000;

/// The trailer's first layout as a spin bet on under it holds it: 136 bytes, under `OBSERVE1`,
/// with a seed in its result. Nothing reads it; the next bet writes the second layout over it.
fn first_layout(result: [u8; 32]) -> Vec<u8> {
    let mut old = b"OBSERVE1".to_vec();
    old.extend_from_slice(&[observe::KIND_SLOTS, observe::SETTLED, 0, 0, 0, 0, 0, 0]);
    old.extend_from_slice(&[0x5A; 56]);
    old.extend_from_slice(&result);
    old.extend_from_slice(&[0; 32]);
    assert_eq!(old.len(), 136);
    old
}

/// What a test's spin holds: its head, the terms as they were laid out at `size`, the
/// generation where the size has one, and the trailer, in the layout of its time, where it has
/// that.
impl Table {
    fn spin_at(&self, size: usize, status: SpinStatus, round: u64, generation: u64, observed: Option<Observable>) -> Account {
        let mut account = self.spin_account(status, round);
        let legacy = size < Spin::WITH_TERMS;
        if legacy {
            // Before run rules, terms were 344 bytes; the head is all a bet reads of them.
            account.data.truncate(Spin::SIZE + 344);
        }
        if [Spin::PERSISTENT_SIZE, Spin::FIRST_OBSERVED_SIZE, Spin::PRE_REFERRAL_SIZE, Spin::OBSERVED_SIZE, Spin::SIZE + 344 + 8].contains(&size) {
            account.data.extend_from_slice(&generation.to_le_bytes());
        }
        if size == Spin::FIRST_OBSERVED_SIZE {
            account.data.extend_from_slice(&first_layout([0xEE; 32]));
        }
        if size == Spin::OBSERVED_SIZE {
            account.data.extend_from_slice(bytemuck::bytes_of(&Referral::unlinked()));
        }
        if size == Spin::OBSERVED_SIZE || size == Spin::PRE_REFERRAL_SIZE {
            account.data.extend_from_slice(bytemuck::bytes_of(&observed.unwrap_or(Observable::new(observe::KIND_SLOTS))));
        }
        assert_eq!(account.data.len(), size);
        account
    }

    /// The same spin with `seed` as its unapplied seed.
    fn seeded(&self, mut account: Account, seed: [u8; 32]) -> Account {
        bytemuck::from_bytes_mut::<Spin>(&mut account.data[..Spin::SIZE]).seed = seed;
        account
    }

    fn data_after<'r>(&self, accounts: &'r [(Pubkey, Account)]) -> &'r [u8] {
        &accounts.iter().find(|(k, _)| *k == self.spin).unwrap().1.data
    }

    fn trailer_after(&self, accounts: &[(Pubkey, Account)]) -> Observable {
        let data = self.data_after(accounts);
        assert_eq!(data.len(), Spin::OBSERVED_SIZE);
        Observable::read(data).expect("a trailer")
    }

    fn generation_after(&self, accounts: &[(Pubkey, Account)]) -> u64 {
        let data = self.data_after(accounts);
        u64::from_le_bytes(data[Spin::WITH_TERMS..Spin::PERSISTENT_SIZE].try_into().unwrap())
    }

    fn permission(&self) -> Pubkey {
        fixed(casino_core::permission::address(&casino_core::chain::Pubkey::new_from_array(self.spin.to_bytes())))
    }

    /// The spin's permission as the ACL program leaves it once it names everyone a bet asks for,
    /// so the bet has nothing to add and calls nothing.
    fn full_permission(&self) -> Account {
        let read = casino_core::magicblock::MEMBER_READ;
        let mut data = vec![0u8, 255];
        data.extend_from_slice(self.spin.as_ref());
        data.push(1);
        for (flags, member) in [
            (0, program()),
            (read, fixed(casino_core::ids::VAULT_PROGRAM)),
            (read, self.user),
            (read, fixed(slots::constants::PRIVATE_CASINO)),
        ] {
            data.push(flags);
            data.extend_from_slice(member.as_ref());
        }
        Account { lamports: SOL, data, owner: fixed(casino_core::ids::PERMISSION_PROGRAM), executable: false, rent_epoch: 0 }
    }
}

/// The shelf, with the test machine at 0.
fn shelf() -> Account {
    let mut header = casino_core::shelf::Header::zeroed();
    header.discriminator = config::DISCRIMINATOR;
    header.version = config::VERSION;
    header.count = 1;
    let mut data = bytemuck::bytes_of(&header).to_vec();
    data.extend_from_slice(bytemuck::bytes_of(&terms()));
    assert_eq!(data.len(), Config::size_for(1));
    owned(data)
}

fn books() -> Account {
    let mut a = Analytics::zeroed();
    a.discriminator = analytics::DISCRIMINATOR;
    a.version = analytics::VERSION;
    owned(bytemuck::bytes_of(&a).to_vec())
}

/// What the test machine pays for a spin's head, as the collect prices it.
fn pays(spin: &Spin) -> u64 {
    slots::instructions::request_collect::payout(spin, &terms()).unwrap()
}

/// A seed whose grid wins (or loses) on the test machine from the head a test spin starts with.
fn seed_that(table: &Table, wins: bool) -> [u8; 32] {
    let head = *bytemuck::from_bytes::<Spin>(&table.spin_account(SpinStatus::Rolled, 1).data[..Spin::SIZE]);
    (0..=255u8)
        .map(|b| [b; 32])
        .find(|&seed| (pays(&Spin { seed, ..head }) > 0) == wins)
        .expect("no such seed")
}

fn winning_seed(table: &Table) -> [u8; 32] {
    seed_that(table, true)
}

fn vault_authority() -> Pubkey {
    fixed(casino_core::ids::VAULT_AUTHORITY)
}

/// The vault's settle callback for a stake on machine 0, with `rest` after the named accounts.
fn resolve_bet(table: &Table, rest: &[Pubkey]) -> Instruction {
    let mut data = 17u64.to_le_bytes().to_vec();
    data.extend_from_slice(table.user.as_ref());
    data.extend_from_slice(&0u64.to_le_bytes());
    data.extend_from_slice(table.consenter.as_ref());
    let mut accounts = vec![
        AccountMeta::new_readonly(Pubkey::new_unique(), false),
        AccountMeta::new_readonly(vault_authority(), true),
        AccountMeta::new_readonly(pda(&[b"config"]), false),
        AccountMeta::new(pda(&[b"house"]), false),
        AccountMeta::new(table.spin, false),
        AccountMeta::new(fixed(casino_core::ids::EPHEMERAL_VAULT_ID), false),
        AccountMeta::new_readonly(magic(), false),
        AccountMeta::new(pda(&[b"analytics"]), false),
        AccountMeta::new(table.permission(), false),
        AccountMeta::new_readonly(fixed(casino_core::ids::PERMISSION_PROGRAM), false),
    ];
    accounts.extend(rest.iter().map(|k| AccountMeta::new_readonly(*k, false)));
    Instruction::new_with_bytes(program(), &data, accounts)
}

fn bet_accounts(ix: &Instruction, table: &Table, spin: Account, rest: Vec<Account>) -> Vec<(Pubkey, Account)> {
    let k = |i: usize| ix.accounts[i].pubkey;
    let mut accounts = vec![
        (k(0), wallet()),
        (k(1), wallet()),
        (k(2), shelf()),
        (k(3), owned(vec![])),
        (k(4), spin),
        (k(5), wallet()),
        (k(6), builtin(magic(), "fake_magic")),
        (k(7), books()),
        (k(8), table.full_permission()),
        (k(9), wallet()),
    ];
    accounts.extend(ix.accounts[10..].iter().map(|m| m.pubkey).zip(rest));
    accounts
}

/// No spin yet: the address as the rollup leaves it for the Magic program to create.
fn no_spin() -> Account {
    Account { lamports: 0, data: vec![], owner: magic(), executable: false, rent_epoch: 0 }
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn a_first_bet_creates_the_spin_with_its_trailer() {
    let table = Table::fixed();
    let ix = resolve_bet(&table, &[]);
    let result = with_stand_ins().process_instruction(&ix, &bet_accounts(&ix, &table, no_spin(), vec![]));
    assert_eq!(result.raw_result, Ok(()));
    assert_eq!(magic_calls(), vec![12]);
    assert_eq!(calls_to(magic())[0][4..8], (Spin::OBSERVED_SIZE as u32).to_le_bytes(), "created at the observed size");
    assert_eq!(table.generation_after(&result.resulting_accounts), 1);
    assert_eq!(
        table.trailer_after(&result.resulting_accounts),
        Observable { status: observe::PENDING, generation: 1, stake: STAKE, ..Observable::new(observe::KIND_SLOTS) },
        "staked, unwatched, with nothing to show or pay yet"
    );
    println!("resolve_bet, creating the spin: {} CU", result.compute_units_consumed);
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn a_bet_brings_a_spin_of_any_earlier_size_to_the_observed_one() {
    // 504 and 512 are from before run rules (terms of another shape): closed, then created again.
    // 664, 672 and 808 (the trailer's first layout) only lack what came after: grown in place.
    // 1016 is already there.
    let table = Table::new();
    let shown = [3u8; 64];
    let last = Observable { stake: 9, paid: 4 * STAKE, ..trailer(observe::SETTLED, Pubkey::new_unique(), shown) };
    for (size, generation, calls) in [
        (Spin::SIZE + 344, 0, vec![14, 12]),
        (Spin::SIZE + 344 + 8, 6, vec![14, 12]),
        (Spin::WITH_TERMS, 0, vec![13]),
        (Spin::PERSISTENT_SIZE, 6, vec![13]),
        (Spin::FIRST_OBSERVED_SIZE, 6, vec![13]),
        (Spin::OBSERVED_SIZE, 6, vec![]),
    ] {
        let before = table.spin_at(size, SpinStatus::Collected, 2, generation, Some(last));
        let ix = resolve_bet(&table, &[]);
        let result = with_stand_ins().process_instruction(&ix, &bet_accounts(&ix, &table, before, vec![]));
        assert_eq!(result.raw_result, Ok(()), "{size}");
        assert_eq!(magic_calls(), calls, "{size}");
        let after = &result.resulting_accounts;
        assert_eq!(table.data_after(after).len(), Spin::OBSERVED_SIZE, "{size}");
        assert_eq!(table.generation_after(after), generation + 1, "{size}: a late answer for the last bet can no longer land");
        let s = table.spin_after(after);
        assert_eq!((s.status, s.round), (SpinStatus::Bought as u64, 0), "{size}");
        assert_eq!(s.seed, [7; 32], "{size}: the last seed stays until this bet's lands");
        assert_eq!(table.data_after(after)[Spin::SIZE..Spin::WITH_TERMS], *bytemuck::bytes_of(&terms()), "{size}: the terms are untouched");
        let t = table.trailer_after(after);
        assert_eq!((t.kind, t.status, t.generation, t.round, t.config_id), (observe::KIND_SLOTS, observe::PENDING, generation + 1, 0, 0), "{size}");
        assert_eq!((t.stake, t.paid), (STAKE, 0), "{size}: this bet's stake, and nothing paid on it yet");
        assert_eq!(t.observer, [0; 32], "{size}: a bet without a station clears the last one");
        assert_eq!(t.bet, [0; 192], "{size}: a slot's bet is its machine");
        if size == Spin::OBSERVED_SIZE {
            assert_eq!(t.result, shown, "the result the floor last saw is kept whole");
        } else {
            assert_eq!(t.result[..32], [7; 32], "{size}: a spin from before this layout shows its last seed");
            assert_eq!(t.result[32..], [0; 32]);
        }
    }

    // A size no spin was ever placed at is not guessed at.
    for size in [Spin::OBSERVED_SIZE - 8, Spin::OBSERVED_SIZE + 1] {
        let mut before = table.spin_at(Spin::OBSERVED_SIZE, SpinStatus::Collected, 2, 6, None);
        before.data.resize(size, 0);
        let ix = resolve_bet(&table, &[]);
        let result = with_stand_ins().process_instruction(&ix, &bet_accounts(&ix, &table, before, vec![]));
        assert_eq!(result.raw_result, failure(InstructionError::InvalidAccountData), "{size}");
        assert_eq!(magic_calls(), Vec::<u32>::new(), "{size}");
    }
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn the_observer_is_a_station_the_floor_owns_and_only_for_that_bet() {
    let table = Table::new();
    let at = Pubkey::new_unique();
    let not_the_floor = Account { owner: Pubkey::new_unique(), ..station() };
    let empty = Account { data: vec![], ..station() };
    for (named, expected) in [
        (station(), at.to_bytes()),
        (not_the_floor, [0; 32]),
        (empty, [0; 32]),
    ] {
        let before = table.spin_at(Spin::OBSERVED_SIZE, SpinStatus::Collected, 0, 1, Some(trailer(observe::SETTLED, Pubkey::new_unique(), [0; 64])));
        let ix = resolve_bet(&table, &[at]);
        let result = with_stand_ins().process_instruction(&ix, &bet_accounts(&ix, &table, before, vec![named]));
        assert_eq!(result.raw_result, Ok(()));
        assert_eq!(table.trailer_after(&result.resulting_accounts).observer, expected);
    }

    let table = Table::fixed();
    let before = table.spin_at(Spin::OBSERVED_SIZE, SpinStatus::Collected, 0, 1, None);
    let ix = resolve_bet(&table, &[]);
    let result = with_stand_ins().process_instruction(&ix, &bet_accounts(&ix, &table, before, vec![]));
    assert_eq!(result.raw_result, Ok(()));
    println!("resolve_bet, reusing the spin: {} CU", result.compute_units_consumed);
    let before = table.spin_at(Spin::OBSERVED_SIZE, SpinStatus::Collected, 0, 1, None);
    let ix = resolve_bet(&table, &[at]);
    let result = with_stand_ins().process_instruction(&ix, &bet_accounts(&ix, &table, before, vec![station()]));
    assert_eq!(result.raw_result, Ok(()));
    println!("resolve_bet, reusing the spin, at a station: {} CU", result.compute_units_consumed);

    // Only the first trailing account is a station.
    let before = table.spin_at(Spin::OBSERVED_SIZE, SpinStatus::Collected, 0, 1, None);
    let other = Pubkey::new_unique();
    let ix = resolve_bet(&table, &[other, at]);
    let result = with_stand_ins().process_instruction(&ix, &bet_accounts(&ix, &table, before, vec![wallet(), station()]));
    assert_eq!(result.raw_result, Ok(()));
    assert_eq!(table.trailer_after(&result.resulting_accounts).observer, [0; 32]);
}

/// Asks for this round's seed, with the stand-in VRF taking the request.
fn request_reveal(table: &Table) -> (Instruction, Vec<(Pubkey, Account)>) {
    let (system, system_account) = keyed_account_for_system_program();
    let ix = Instruction::new_with_bytes(program(), &18u64.to_le_bytes(), vec![
        AccountMeta::new_readonly(table.user, false),
        AccountMeta::new(pda(&[b"house"]), false),
        AccountMeta::new(table.spin, false),
        AccountMeta::new_readonly(pda(&[b"identity"]), false),
        AccountMeta::new(Pubkey::new_unique(), false),
        AccountMeta::new_readonly(Pubkey::new_unique(), false),
        AccountMeta::new_readonly(system, false),
        AccountMeta::new_readonly(vrf_program(), false),
    ]);
    let k = |i: usize| ix.accounts[i].pubkey;
    let accounts = vec![
        (k(0), wallet()),
        (k(1), owned(vec![])),
        (k(3), wallet()),
        (k(4), wallet()),
        (k(5), wallet()),
        (system, system_account),
        (vrf_program(), builtin(vrf_program(), "fake_vrf")),
    ];
    (ix, accounts)
}

/// The accounts a VRF request asks its callback to be called with, after the identity.
fn callback_metas(request: &[u8]) -> Vec<(Pubkey, bool, bool)> {
    // tag, caller seed, callback program, callback discriminator (length-prefixed), metas.
    let at = 8 + 32 + 32 + 4 + 8;
    let count = u32::from_le_bytes(request[at..at + 4].try_into().unwrap()) as usize;
    let (metas, _) = request[at + 4..at + 4 + count * 34].as_chunks::<34>();
    metas
        .iter()
        .map(|m| (key(m[..32].try_into().unwrap()), m[32] == 1, m[33] == 1))
        .collect()
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn the_callback_is_handed_the_publish_accounts_only_while_a_station_watches() {
    let table = Table::new();
    let house = pda(&[b"house"]);
    let publish = vec![
        (house, false, true),
        (fixed(casino_core::ids::MAGIC_CONTEXT_ID), false, true),
        (magic(), false, false),
    ];
    let watched = trailer(observe::PENDING, Pubkey::new_unique(), [0; 64]);
    for (spin, extra) in [
        (table.spin_at(Spin::OBSERVED_SIZE, SpinStatus::Bought, 0, 5, Some(watched)), publish),
        (table.spin_at(Spin::OBSERVED_SIZE, SpinStatus::Bought, 0, 5, Some(trailer(observe::PENDING, Pubkey::default(), [0; 64]))), vec![]),
        (table.spin_at(Spin::PERSISTENT_SIZE, SpinStatus::Bought, 0, 5, None), vec![]),
    ] {
        let (ix, mut accounts) = request_reveal(&table);
        accounts.push((table.spin, spin));
        let result = with_stand_ins().process_instruction(&ix, &accounts);
        assert_eq!(result.raw_result, Ok(()));
        let requests = calls_to(vrf_program());
        assert_eq!(requests.len(), 1);
        let mut expected = vec![(table.spin, false, true)];
        expected.extend(extra.iter().copied());
        assert_eq!(callback_metas(&requests[0]), expected);
    }
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn a_round_s_seed_is_asked_for_once() {
    let table = Table::new();
    let ask = |spin: Account| {
        let (ix, mut accounts) = request_reveal(&table);
        accounts.push((table.spin, spin));
        with_stand_ins().process_instruction(&ix, &accounts)
    };
    let first = ask(table.spin_at(Spin::PERSISTENT_SIZE, SpinStatus::Bought, 0, 5, None));
    assert_eq!(first.raw_result, Ok(()));
    assert_eq!(calls_to(vrf_program()).len(), 1);
    let asked = first.resulting_accounts.iter().find(|(k, _)| *k == table.spin).unwrap().1.clone();

    let again = ask(asked);
    assert_eq!(again.raw_result, core(casino_core::CoreError::WrongStatus), "a second ask is refused");
    assert!(calls_to(vrf_program()).is_empty());
    for status in [SpinStatus::Requested, SpinStatus::Rolled, SpinStatus::Collected] {
        let result = ask(table.spin_at(Spin::PERSISTENT_SIZE, status, 0, 5, None));
        assert_eq!(result.raw_result, core(casino_core::CoreError::WrongStatus));
    }
}

/// The oracle's answer for round `round` of bet 5, with `rest` after the spin.
fn callback(table: &Table, round: u64, rest: &[AccountMeta]) -> Instruction {
    callback_with(table, [9; 32], round, rest)
}

fn callback_with(table: &Table, seed: [u8; 32], round: u64, rest: &[AccountMeta]) -> Instruction {
    let mut ix = reveal(table, seed, round, &[]);
    ix.data[8 + 32 + 8..8 + 32 + 16].copy_from_slice(&5u64.to_le_bytes());
    ix.accounts.extend_from_slice(rest);
    ix
}

fn publish_metas() -> Vec<AccountMeta> {
    vec![
        AccountMeta::new(pda(&[b"house"]), false),
        AccountMeta::new(fixed(casino_core::ids::MAGIC_CONTEXT_ID), false),
        AccountMeta::new_readonly(magic(), false),
    ]
}

fn callback_accounts(ix: &Instruction, spin: Account, magic_account: Account) -> Vec<(Pubkey, Account)> {
    let mut accounts = vec![(ix.accounts[0].pubkey, wallet()), (ix.accounts[1].pubkey, spin)];
    for meta in &ix.accounts[2..] {
        let account = match meta.pubkey {
            k if k == magic() => magic_account.clone(),
            k if k == pda(&[b"house"]) => owned(vec![]),
            _ => wallet(),
        };
        accounts.push((meta.pubkey, account));
    }
    accounts
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn a_landed_seed_is_the_trailer_s_result() {
    let table = Table::fixed();
    let previous = Observable { stake: STAKE, paid: 77, ..trailer(observe::PENDING, Pubkey::default(), [3; 64]) };
    let win = winning_seed(&table);
    for seed in [win, seed_that(&table, false)] {
        let ix = callback_with(&table, seed, 1, &[]);
        let spin = table.spin_at(Spin::OBSERVED_SIZE, SpinStatus::Requested, 1, 5, Some(previous));
        let result = mollusk().process_instruction(&ix, &callback_accounts(&ix, spin, wallet()));
        assert_eq!(result.raw_result, Ok(()));
        let t = table.trailer_after(&result.resulting_accounts);
        assert_eq!((t.status, t.round, t.generation, t.stake), (observe::RESULT, 1, 5, STAKE));
        assert_eq!(t.result[..32], seed);
        assert_eq!(t.result[32..], [3; 32], "only the seed's bytes are the seed's");
        let landed = table.spin_after(&result.resulting_accounts);
        assert_eq!(landed.seed, seed);
        assert_eq!(t.paid, pays(&landed), "what collecting this grid would pay");
        assert_eq!(t.paid > 0, seed == win);
        println!("callback_reveal, unwatched, {}: {} CU", if seed == win { "a win" } else { "a loss" }, result.compute_units_consumed);
    }

    // A spin not yet at the observed size just lands its seed.
    let ix = callback(&table, 1, &[]);
    let spin = table.spin_at(Spin::PERSISTENT_SIZE, SpinStatus::Requested, 1, 5, None);
    let result = mollusk().process_instruction(&ix, &callback_accounts(&ix, spin, wallet()));
    assert_eq!(result.raw_result, Ok(()));
    assert_eq!(table.data_after(&result.resulting_accounts).len(), Spin::PERSISTENT_SIZE);
    println!("callback_reveal, a spin without the trailer: {} CU", result.compute_units_consumed);
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn a_watched_seed_is_published_only_with_the_house_s_accounts() {
    let table = Table::new();
    let station = Pubkey::new_unique();
    let watched = || table.spin_at(Spin::OBSERVED_SIZE, SpinStatus::Requested, 1, 5, Some(trailer(observe::PENDING, station, [0; 64])));

    // Requested before the station sat down, or before this program knew to ask: no publish.
    let ix = callback(&table, 1, &[]);
    let result = mollusk().process_instruction(&ix, &callback_accounts(&ix, watched(), wallet()));
    assert_eq!(result.raw_result, Ok(()));
    let t = table.trailer_after(&result.resulting_accounts);
    assert_eq!((t.status, t.observer), (observe::RESULT, station.to_bytes()));

    // With them, the house goes on to the scheduler.
    let ix = callback(&table, 1, &publish_metas());
    let result = mollusk().process_instruction(&ix, &callback_accounts(&ix, watched(), wallet()));
    assert!(reached_the_scheduler(&result), "{:?}", result.raw_result);

    // Anything that is not the house's own accounts is no publish.
    let mut wrong_house = publish_metas();
    wrong_house[0].pubkey = Pubkey::new_unique();
    let mut wrong_context = publish_metas();
    wrong_context[1].pubkey = Pubkey::new_unique();
    for rest in [wrong_house, wrong_context, publish_metas()[..2].to_vec()] {
        let ix = callback(&table, 1, &rest);
        let result = mollusk().process_instruction(&ix, &callback_accounts(&ix, watched(), wallet()));
        assert_eq!(result.raw_result, Ok(()));
    }

    // Nobody watching: nothing to publish, whatever the callback was handed.
    let ix = callback(&table, 1, &publish_metas());
    let unwatched = table.spin_at(Spin::OBSERVED_SIZE, SpinStatus::Requested, 1, 5, Some(trailer(observe::PENDING, Pubkey::default(), [0; 64])));
    let result = mollusk().process_instruction(&ix, &callback_accounts(&ix, unwatched, wallet()));
    assert_eq!(result.raw_result, Ok(()));
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn the_publish_is_one_task_the_house_schedules_for_the_floor() {
    let table = Table::fixed();
    let station = Pubkey::new_unique();
    let spin = table.spin_at(Spin::OBSERVED_SIZE, SpinStatus::Requested, 1, 5, Some(trailer(observe::PENDING, station, [0; 64])));
    let ix = callback(&table, 1, &publish_metas());
    let result = with_stand_ins().process_instruction(&ix, &callback_accounts(&ix, spin, builtin(magic(), "fake_magic")));
    assert_eq!(result.raw_result, Ok(()));
    assert_eq!(magic_calls(), vec![6], "one task scheduled");
    let task = &calls_to(magic())[0];
    let id = casino_core::observe::publish_id(&slots::ID, &casino_core::chain::Pubkey::new_from_array(table.spin.to_bytes()));
    assert_eq!(task[4..12], id.to_le_bytes(), "under the spin's publish id, so a newer result replaces it");
    let floor = fixed(casino_core::ids::FLOOR);
    let contains = |needle: &[u8]| task.windows(needle.len()).any(|w| w == needle);
    assert!(contains(floor.as_ref()) && contains(station.as_ref()) && contains(table.spin.as_ref()));
    assert!(contains(&observe::PUBLISH.to_le_bytes()));
    println!("callback_reveal, watched, scheduling the publish: {} CU (stand-in included)", result.compute_units_consumed);
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn a_round_decision_is_pending_again_for_the_same_station() {
    let table = Table::new();
    let station = Pubkey::new_unique();
    // Neither decision takes more stake; what the seen grid paid shows until the next one lands.
    // The decision is taken, so none is open.
    let mut landed = Observable { stake: STAKE, paid: 3 * STAKE, flags: observe::FLAG_DECIDING, ..trailer(observe::RESULT, station, [0; 64]) };
    landed.result[..32].copy_from_slice(&[7; 32]);
    let result = mollusk().process_instruction(
        &hold(table.user, true, table.spin, 0b011),
        &[(table.user, wallet()), (table.spin, table.spin_at(Spin::OBSERVED_SIZE, SpinStatus::Rolled, 0, 5, Some(landed)))],
    );
    assert_eq!(result.raw_result, Ok(()));
    assert_eq!(table.trailer_after(&result.resulting_accounts), Observable { status: observe::PENDING, flags: 0, ..landed });

    // A spin bet on before the trailer is held as it always was.
    let result = mollusk().process_instruction(
        &hold(table.user, true, table.spin, 0b011),
        &[(table.user, wallet()), (table.spin, table.spin_at(Spin::PERSISTENT_SIZE, SpinStatus::Rolled, 0, 5, None))],
    );
    assert_eq!(result.raw_result, Ok(()));
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn a_collected_spin_is_settled_for_the_floor() {
    let table = Table::new();
    let station = Pubkey::new_unique();
    let win = winning_seed(&table);
    let mut landed = Observable { stake: STAKE, paid: 1, flags: observe::FLAG_DECIDING, ..trailer(observe::RESULT, station, [0; 64]) };
    landed.result[..32].copy_from_slice(&win);
    let mut data = 23u64.to_le_bytes().to_vec();
    data.extend_from_slice(table.user.as_ref());
    let ix = Instruction::new_with_bytes(program(), &data, vec![
        AccountMeta::new_readonly(Pubkey::new_unique(), false),
        AccountMeta::new_readonly(vault_authority(), true),
        AccountMeta::new(pda(&[b"house"]), false),
        AccountMeta::new(table.spin, false),
        AccountMeta::new(fixed(casino_core::ids::EPHEMERAL_VAULT_ID), false),
        AccountMeta::new_readonly(magic(), false),
        AccountMeta::new(pda(&[b"analytics"]), false),
    ]);
    let k = |i: usize| ix.accounts[i].pubkey;
    // Round 2 of three: the last grid, which only a collect follows.
    let accounts = vec![
        (k(0), wallet()),
        (k(1), wallet()),
        (k(2), owned(vec![])),
        (k(3), table.seeded(table.spin_at(Spin::OBSERVED_SIZE, SpinStatus::Rolled, 2, 5, Some(landed)), win)),
        (k(4), wallet()),
        (k(5), wallet()),
        (k(6), books()),
    ];
    let result = mollusk().process_instruction(&ix, &accounts);
    assert_eq!(result.raw_result, Ok(()));
    let after = table.spin_after(&result.resulting_accounts);
    assert_eq!(after.status, SpinStatus::Collected as u64);
    let paid = pays(&after);
    assert!(paid > 0);
    assert_eq!(table.trailer_after(&result.resulting_accounts), Observable { status: observe::SETTLED, flags: 0, paid, ..landed }, "what the vault moved");
    let books = result.resulting_accounts.iter().find(|(key, _)| *key == k(6)).unwrap();
    assert_eq!(bytemuck::from_bytes::<Analytics>(&books.1.data).payouts[0].amount, paid, "and what the books recorded");
}

// ── the collect ────────────────────────────────────────────────────────────────────────────
//
// ScheduleCollect (27) names no ledger: whoever may ask — the player, their session key, or anyone
// once nothing is left to decide, the casino floor's house among them — and it schedules the house's
// own PayCollect, which the house's executor signs and which pays. A watched spin's publish carries
// what the floor needs to send the request, and nothing else. RequestCollect (22) stays beside it
// as every released client sends it: the signer consents to a receipt the vault settles.

/// The spin, bet on `mode`'s machine.
fn in_mode(mut account: Account, mode: u8) -> Account {
    account.data[Spin::SIZE..Spin::WITH_TERMS].copy_from_slice(bytemuck::bytes_of(&machine(mode)));
    account
}

/// The same spin with `pending` riding, as a gamble leaves it.
fn riding(mut account: Account, pending: u64) -> Account {
    bytemuck::from_bytes_mut::<Spin>(&mut account.data[..Spin::SIZE]).pending = pending;
    account
}

/// A seed on which `spin` pays (or does not) when collected, on the machine it was bet on.
fn seed_paying(spin: &Account, wins: bool) -> [u8; 32] {
    let head = *bytemuck::from_bytes::<Spin>(&spin.data[..Spin::SIZE]);
    let terms = *bytemuck::from_bytes::<MachineConfig>(&spin.data[Spin::SIZE..Spin::WITH_TERMS]);
    (0..=255u8)
        .map(|b| [b; 32])
        .find(|&seed| (slots::instructions::request_collect::payout(&Spin { seed, ..head }, &terms).unwrap() > 0) == wins)
        .expect("no such seed")
}

fn core_key(k: Pubkey) -> casino_core::chain::Pubkey {
    casino_core::chain::Pubkey::new_from_array(k.to_bytes())
}

/// Everything a publish for `table`'s spin at `station` names, as the scheduler fires it: the
/// publish's own seven accounts, then the request's three. Key, signer, writable.
fn the_floor_holds(table: &Table, station: Pubkey) -> Vec<(Pubkey, bool, bool)> {
    let house = core_key(pda(&[b"house"]));
    let publish = observe::publish_collecting_instruction(&slots::ID, &house, &core_key(station), &table.user.to_bytes(), &core_key(table.spin));
    publish.accounts.iter().map(|m| (fixed(m.pubkey), m.is_signer, m.is_writable)).collect()
}

/// The accounts of the one instruction a task fires: after the tag, the id, the interval, the
/// iterations, the instruction count and the instruction's program.
fn task_accounts(task: &[u8]) -> (Pubkey, Vec<(Pubkey, bool, bool)>) {
    let at = 4 + 8 + 8 + 8 + 8;
    let program = key(task[at..at + 32].try_into().unwrap());
    let count = u64::from_le_bytes(task[at + 32..at + 40].try_into().unwrap()) as usize;
    let (metas, _) = task[at + 40..at + 40 + count * 34].as_chunks::<34>();
    (program, metas.iter().map(|m| (key(m[..32].try_into().unwrap()), m[32] == 1, m[33] == 1)).collect())
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn a_watched_spin_s_publish_carries_the_floor_s_collect() {
    let table = Table::fixed();
    let station = Pubkey::new_unique();
    let spin = in_mode(table.spin_at(Spin::OBSERVED_SIZE, SpinStatus::Requested, 0, 5, Some(trailer(observe::PENDING, station, [0; 64]))), MODE_LINES);
    let ix = callback_with(&table, seed_paying(&spin, true), 0, &publish_metas());
    let result = with_stand_ins().process_instruction(&ix, &callback_accounts(&ix, spin, builtin(magic(), "fake_magic")));
    assert_eq!(result.raw_result, Ok(()));
    assert_eq!(magic_calls(), vec![6], "one task scheduled");
    let (program, accounts) = task_accounts(&calls_to(magic())[0]);
    assert_eq!(program, fixed(casino_core::ids::FLOOR));
    assert_eq!(accounts, the_floor_holds(&table, station));
    assert_eq!(accounts.len(), 7 + observe::REQUEST_ACCOUNTS);
    assert_eq!(accounts[3], (table.spin, false, true), "the play, writable for the request");
    let request = observe::request_metas(&slots::ID, &core_key(pda(&[b"house"])), &core_key(table.user));
    let request: Vec<_> = request.iter().map(|m| (fixed(m.pubkey), m.is_signer, m.is_writable)).collect();
    assert_eq!(accounts[7..], request[..], "the user, the game, the house");
    let t = table.trailer_after(&result.resulting_accounts);
    assert_eq!((t.status, t.flags), (observe::RESULT, 0), "a line spin leaves nothing to decide");
    assert!(t.paid > 0);
    println!("callback_reveal, watched, scheduling the collecting publish: {} CU (stand-in included)", result.compute_units_consumed);
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn the_floor_is_told_a_decision_is_open_exactly_while_one_is() {
    let table = Table::new();
    let station = Pubkey::new_unique();
    // Three grids to hold between, three rungs to gamble up.
    for (mode, round, wins, deciding) in [
        (MODE_LINES, 0, true, false),
        (MODE_LINES, 0, false, false),
        (MODE_HOLD, 0, false, true),
        (MODE_HOLD, 1, true, true),
        (MODE_HOLD, 2, true, false),
        (MODE_GAMBLE, 0, true, true),
        (MODE_GAMBLE, 0, false, false),
        (MODE_GAMBLE, 2, true, true),
        (MODE_GAMBLE, 2, false, false),
        (MODE_GAMBLE, 3, true, false),
    ] {
        let watched = Some(trailer(observe::PENDING, station, [0; 64]));
        let spin = riding(in_mode(table.spin_at(Spin::OBSERVED_SIZE, SpinStatus::Requested, round, 5, watched), mode), STAKE);
        let ix = callback_with(&table, seed_paying(&spin, wins), round, &[]);
        let result = mollusk().process_instruction(&ix, &callback_accounts(&ix, spin, wallet()));
        assert_eq!(result.raw_result, Ok(()), "{mode} {round}");
        let t = table.trailer_after(&result.resulting_accounts);
        assert_eq!(t.paid > 0, wins, "{mode} {round}");
        assert_eq!(t.flags, if deciding { observe::FLAG_DECIDING } else { 0 }, "mode {mode}, round {round}, a win: {wins}");
        let landed = table.spin_after(&result.resulting_accounts);
        assert_eq!(slots::instructions::request_collect::deciding(&landed, &machine(mode), t.paid), deciding, "the collect's own rule");
    }
}

/// ScheduleCollect with `collector` signing: [collector (s), user, house (w), spin (w),
/// magic_context (w), magic_program].
fn schedule_collect(table: &Table, collector: Pubkey) -> Instruction {
    Instruction::new_with_bytes(program(), &27u64.to_le_bytes(), vec![
        AccountMeta::new_readonly(collector, true),
        AccountMeta::new_readonly(table.user, false),
        AccountMeta::new(pda(&[b"house"]), false),
        AccountMeta::new(table.spin, false),
        AccountMeta::new(fixed(casino_core::ids::MAGIC_CONTEXT_ID), false),
        AccountMeta::new_readonly(magic(), false),
    ])
}

/// ScheduleCollect as the floor sends it: every account from what its publish holds, as writable
/// as it is there, and the floor's house signing.
fn floor_collect(table: &Table, station: Pubkey) -> Instruction {
    let holds = the_floor_holds(table, station);
    let mut ix = schedule_collect(table, fixed(casino_core::ids::FLOOR_HOUSE));
    for meta in ix.accounts.iter_mut().skip(1) {
        let &(_, _, is_writable) = holds.iter().find(|(held, ..)| *held == meta.pubkey).expect("the publish names it");
        assert_eq!(meta.is_writable, is_writable, "{}", meta.pubkey);
    }
    ix
}

fn collect_accounts(ix: &Instruction, spin: Account) -> Vec<(Pubkey, Account)> {
    let k = |i: usize| ix.accounts[i].pubkey;
    vec![
        (k(0), wallet()),
        (k(1), wallet()),
        (k(2), owned(vec![])),
        (k(3), spin),
        (k(4), wallet()),
        (k(5), builtin(magic(), "fake_magic")),
    ]
}

/// The PayCollect `ScheduleCollect` schedules for `table`'s spin at `generation` and `round`.
fn pay_collect(table: &Table, generation: u64, round: u64) -> Instruction {
    let ix = slots::instructions::pay_collect::instruction(&core_key(pda(&[b"house"])), &core_key(table.spin), &core_key(table.user), generation, round);
    Instruction::new_with_bytes(
        program(),
        &ix.data,
        ix.accounts.iter().map(|m| AccountMeta { pubkey: fixed(m.pubkey), is_signer: m.is_signer, is_writable: m.is_writable }).collect(),
    )
}

fn pay_accounts(ix: &Instruction, spin: Account) -> Vec<(Pubkey, Account)> {
    let k = |i: usize| ix.accounts[i].pubkey;
    vec![
        (k(0), wallet()),
        (k(1), owned(vec![])),
        (k(2), wallet()),
        (k(3), spin),
        (k(4), wallet()),
        (k(5), builtin(vault(), "fake_vault")),
        (k(6), books()),
    ]
}

/// RequestCollect (22) with `wallet` consenting to the receipt: [user, house (w), spin (w),
/// receipt (w), ephemeral_vault (w), magic_program, vault_program, wallet (s), house_ledger (w),
/// magic_context (w)] — what every released client sends.
fn receipt_collect(table: &Table, wallet: Pubkey) -> Instruction {
    let house = core_key(pda(&[b"house"]));
    Instruction::new_with_bytes(program(), &22u64.to_le_bytes(), vec![
        AccountMeta::new_readonly(table.user, false),
        AccountMeta::new(pda(&[b"house"]), false),
        AccountMeta::new(table.spin, false),
        AccountMeta::new(fixed(casino_core::receipt::address(&slots::ID, &core_key(wallet))), false),
        AccountMeta::new(fixed(casino_core::ids::EPHEMERAL_VAULT_ID), false),
        AccountMeta::new_readonly(magic(), false),
        AccountMeta::new_readonly(vault(), false),
        AccountMeta::new_readonly(wallet, true),
        AccountMeta::new(fixed(casino_core::vault::ledger(&house)), false),
        AccountMeta::new(fixed(casino_core::ids::MAGIC_CONTEXT_ID), false),
    ])
}

fn receipt_accounts(ix: &Instruction, spin: Account) -> Vec<(Pubkey, Account)> {
    let k = |i: usize| ix.accounts[i].pubkey;
    vec![
        (k(0), wallet()),
        (k(1), owned(vec![])),
        (k(2), spin),
        (k(3), wallet()),
        (k(4), wallet()),
        (k(5), builtin(magic(), "fake_magic")),
        (k(6), builtin(vault(), "fake_vault")),
        (k(7), wallet()),
        (k(8), wallet()),
        (k(9), wallet()),
    ]
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn a_session_key_collects_by_receipt_as_released_clients_do() {
    let table = Table::fixed();
    let ix = receipt_collect(&table, table.consenter);
    let spin = finished(&table, Pubkey::new_unique());
    let paid = pays_on(&spin);
    let result = with_stand_ins().process_instruction(&ix, &receipt_accounts(&ix, spin));
    assert_eq!(result.raw_result, Ok(()));
    println!("request_collect, the session key's receipt: {} CU (stand-in included)", result.compute_units_consumed);

    // The receipt: the session key consents, the house pays the player what the spin pays, and
    // the vault calls back ResolveCollect.
    let created = VAULT_ACCOUNTS.with(|v| v.borrow().clone());
    assert_eq!(created, vec![vec![
        (pda(&[b"house"]), true, true),
        (ix.accounts[8].pubkey, false, true),
        (table.consenter, true, false),
        (ix.accounts[3].pubkey, false, true),
        (ix.accounts[4].pubkey, false, true),
        (magic(), false, false),
        (ix.accounts[9].pubkey, false, true),
    ]]);
    let receipt = &calls_to(vault())[0];
    let contains = |needle: &[u8]| receipt.windows(needle.len()).any(|w| w == needle);
    let movement = [&[0; 32][..], &paid.to_le_bytes(), &[1, 0]].concat();
    assert!(contains(&movement), "house to player, what the spin pays");
    assert!(contains(&[table.user.as_ref(), pda(&[b"house"]).as_ref()].concat()), "the player's ledger, then the house's");
    assert!(contains(&23u64.to_le_bytes()), "called back as ResolveCollect");
    assert!(magic_calls().is_empty(), "no task: the receipt pays");
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn by_receipt_too_only_the_consenter_may_collect_while_deciding() {
    let table = Table::new();
    let ix = receipt_collect(&table, Pubkey::new_unique());
    let result = with_stand_ins().process_instruction(&ix, &receipt_accounts(&ix, deciding(&table)));
    assert_eq!(result.raw_result, core(casino_core::CoreError::Unauthorized));
    assert!(calls_to(vault()).is_empty(), "no receipt");

    let own = receipt_collect(&table, table.consenter);
    let result = with_stand_ins().process_instruction(&own, &receipt_accounts(&own, deciding(&table)));
    assert_eq!(result.raw_result, Ok(()));
    assert_eq!(calls_to(vault()).len(), 1);
}

#[test]
fn the_two_collects_are_the_numbers_the_clients_and_the_floor_send() {
    let table = Table::new();
    assert_eq!(receipt_collect(&table, table.consenter).data, 22u64.to_le_bytes());
    assert_eq!(schedule_collect(&table, table.consenter).data, 27u64.to_le_bytes());
    assert_eq!(receipt_collect(&table, table.consenter).accounts.len(), 10);
}

/// A finished line spin of bet 5 that pays, watched by `station`.
fn finished(table: &Table, station: Pubkey) -> Account {
    let landed = Some(trailer(observe::RESULT, station, [0; 64]));
    let spin = in_mode(table.spin_at(Spin::OBSERVED_SIZE, SpinStatus::Rolled, 0, 5, landed), MODE_LINES);
    table.seeded(spin.clone(), seed_paying(&spin, true))
}

/// A hold spin on its first grid, with two respins left.
fn deciding(table: &Table) -> Account {
    table.spin_at(Spin::OBSERVED_SIZE, SpinStatus::Rolled, 0, 5, Some(trailer(observe::RESULT, Pubkey::new_unique(), [0; 64])))
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn the_floor_s_house_requests_a_collect_with_nothing_left_to_decide() {
    let table = Table::fixed();
    let station = Pubkey::new_unique();
    let ix = floor_collect(&table, station);
    let result = with_stand_ins().process_instruction(&ix, &collect_accounts(&ix, finished(&table, station)));
    assert_eq!(result.raw_result, Ok(()));
    println!("request_collect, the floor's house: {} CU (stand-in included)", result.compute_units_consumed);
    assert!(calls_to(vault()).is_empty(), "the request moves nothing");
    assert_eq!(magic_calls(), vec![6], "one task scheduled");

    // The house's PayCollect, under the spin's collect id, for the grid on show.
    let task = &calls_to(magic())[0];
    let id = slots::instructions::pay_collect::id(&core_key(table.spin));
    assert_eq!(id, casino_core::task::id(&[slots::ID.as_ref(), b"collect", table.spin.as_ref()]));
    assert_eq!(task[4..12], id.to_le_bytes());
    assert_eq!(task[12..20], 1u64.to_le_bytes(), "at once");
    assert_eq!(task[20..28], 1u64.to_le_bytes(), "one fire, never retried");
    let (target, accounts) = task_accounts(task);
    assert_eq!(target, program());
    let house = pda(&[b"house"]);
    let human = core_key(table.user);
    assert_eq!(accounts, vec![
        (fixed(observe::executor(&core_key(house))), true, false),
        (house, false, true),
        (fixed(casino_core::vault::ledger(&core_key(house))), false, true),
        (table.spin, false, true),
        (fixed(casino_core::vault::ledger(&human)), false, true),
        (vault(), false, false),
        (pda(&[b"analytics"]), false, true),
    ]);
    let pay = [&26u64.to_le_bytes()[..], &5u64.to_le_bytes(), &0u64.to_le_bytes()].concat();
    assert!(task.windows(pay.len()).any(|w| w == pay), "PayCollect for generation 5, round 0");

    // The last grid of a hold machine is finished too.
    let last = table.seeded(table.spin_at(Spin::OBSERVED_SIZE, SpinStatus::Rolled, 2, 5, Some(trailer(observe::RESULT, station, [0; 64]))), [7; 32]);
    let result = with_stand_ins().process_instruction(&ix, &collect_accounts(&ix, last));
    assert_eq!(result.raw_result, Ok(()));
    assert_eq!(magic_calls(), vec![6]);
}

#[test]
fn a_collect_request_names_no_ledger() {
    let table = Table::new();
    let house = core_key(pda(&[b"house"]));
    let ledgers = [casino_core::vault::ledger(&house), casino_core::vault::ledger(&core_key(table.user))].map(fixed);
    for ix in [schedule_collect(&table, table.user), floor_collect(&table, Pubkey::new_unique())] {
        assert!(ix.accounts.iter().all(|m| !ledgers.contains(&m.pubkey)));
        assert_eq!(ix.accounts.len(), 6);
    }
    let holds = the_floor_holds(&table, Pubkey::new_unique());
    assert!(holds.iter().all(|(k, ..)| !ledgers.contains(k)), "nor does the publish that carries it");
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn the_player_and_their_session_key_may_collect_while_deciding() {
    let table = Table::new();
    for collector in [table.user, table.consenter] {
        let ix = schedule_collect(&table, collector);
        let result = with_stand_ins().process_instruction(&ix, &collect_accounts(&ix, deciding(&table)));
        assert_eq!(result.raw_result, Ok(()));
        assert_eq!(magic_calls(), vec![6]);
    }
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn nobody_else_may_collect_while_the_player_is_deciding() {
    let table = Table::new();
    for ix in [schedule_collect(&table, Pubkey::new_unique()), floor_collect(&table, Pubkey::new_unique())] {
        let result = with_stand_ins().process_instruction(&ix, &collect_accounts(&ix, deciding(&table)));
        assert_eq!(result.raw_result, core(casino_core::CoreError::Unauthorized));
        assert!(magic_calls().is_empty(), "nothing scheduled");
    }
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn a_stranger_may_collect_a_spin_with_nothing_left_to_decide() {
    let table = Table::new();
    let ix = schedule_collect(&table, Pubkey::new_unique());
    let result = with_stand_ins().process_instruction(&ix, &collect_accounts(&ix, finished(&table, Pubkey::new_unique())));
    assert_eq!(result.raw_result, Ok(()));
    assert_eq!(magic_calls(), vec![6]);

    // Unsigned, though, it is nobody asking.
    let mut unsigned = ix.clone();
    unsigned.accounts[0].is_signer = false;
    let result = with_stand_ins().process_instruction(&unsigned, &collect_accounts(&unsigned, finished(&table, Pubkey::new_unique())));
    assert_eq!(result.raw_result, failure(InstructionError::MissingRequiredSignature));
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn a_collected_spin_is_left_and_an_unfinished_one_refused() {
    let table = Table::new();
    let ix = schedule_collect(&table, table.user);
    let collected = table.spin_at(Spin::OBSERVED_SIZE, SpinStatus::Collected, 0, 5, None);
    let result = with_stand_ins().process_instruction(&ix, &collect_accounts(&ix, collected));
    assert_eq!(result.raw_result, Ok(()));
    assert!(magic_calls().is_empty());

    let requested = table.spin_at(Spin::OBSERVED_SIZE, SpinStatus::Requested, 0, 5, None);
    let result = with_stand_ins().process_instruction(&ix, &collect_accounts(&ix, requested));
    assert_eq!(result.raw_result, custom(GameError::NotRolled));

    // Someone else's spin is not this user's to collect.
    let other = Table::new();
    let mut theirs = ix.clone();
    theirs.accounts[1].pubkey = other.user;
    let result = with_stand_ins().process_instruction(&theirs, &collect_accounts(&theirs, finished(&table, Pubkey::new_unique())));
    assert_eq!(result.raw_result, core(casino_core::CoreError::Unauthorized));
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn the_house_pays_a_requested_spin_once() {
    let table = Table::fixed();
    let station = Pubkey::new_unique();
    let spin = finished(&table, station);
    let paid = pays_on(&spin);
    assert!(paid > 0);
    let ix = pay_collect(&table, 5, 0);
    let result = with_stand_ins().process_instruction(&ix, &pay_accounts(&ix, spin));
    assert_eq!(result.raw_result, Ok(()));
    println!("pay_collect, a win: {} CU (stand-in included)", result.compute_units_consumed);

    // One settle, house ledger to player ledger, the house signing for its side.
    let house = pda(&[b"house"]);
    let settles = VAULT_ACCOUNTS.with(|v| v.borrow().clone());
    assert_eq!(settles, vec![vec![
        (ix.accounts[2].pubkey, false, true),
        (ix.accounts[4].pubkey, false, true),
        (house, true, false),
        (house, true, false),
    ]]);
    let settle = &calls_to(vault())[0];
    assert_eq!(settle[8..40], [0; 32], "SOL");
    assert_eq!(settle[40..48], paid.to_le_bytes());

    let after = table.spin_after(&result.resulting_accounts);
    assert_eq!(after.status, SpinStatus::Collected as u64);
    let t = table.trailer_after(&result.resulting_accounts);
    assert_eq!((t.status, t.flags, t.paid, t.observer), (observe::SETTLED, 0, paid, station.to_bytes()));
    let books = result.resulting_accounts.iter().find(|(key, _)| *key == ix.accounts[6].pubkey).unwrap();
    let books = bytemuck::from_bytes::<Analytics>(&books.1.data);
    assert_eq!(books.payouts[0].amount, paid);
    assert_eq!(books.bets_collected[0], 1);

    // The task's later fires find it collected and do nothing.
    let collected = result.resulting_accounts.iter().find(|(k, _)| *k == table.spin).unwrap().1.clone();
    let again = with_stand_ins().process_instruction(&ix, &pay_accounts(&ix, collected.clone()));
    assert_eq!(again.raw_result, Ok(()));
    assert!(calls_to(vault()).is_empty());
    assert_eq!(table.data_after(&again.resulting_accounts), &collected.data[..]);
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn the_house_pays_only_the_grid_it_was_asked_for() {
    let table = Table::new();
    // The player's next bet, a decision taken since, a spin waiting on its seed, a closed spin.
    let next_bet = pay_collect(&table, 4, 0);
    let later_round = pay_collect(&table, 5, 1);
    for ix in [next_bet, later_round] {
        let result = with_stand_ins().process_instruction(&ix, &pay_accounts(&ix, finished(&table, Pubkey::new_unique())));
        assert_eq!(result.raw_result, Ok(()));
        assert!(calls_to(vault()).is_empty());
        assert_eq!(table.spin_after(&result.resulting_accounts).status, SpinStatus::Rolled as u64);
    }
    let ix = pay_collect(&table, 5, 0);
    let requested = table.spin_at(Spin::OBSERVED_SIZE, SpinStatus::Requested, 0, 5, None);
    for spin in [requested, no_spin()] {
        let result = with_stand_ins().process_instruction(&ix, &pay_accounts(&ix, spin));
        assert_eq!(result.raw_result, Ok(()));
        assert!(calls_to(vault()).is_empty());
    }

    // A loss moves nothing and is collected all the same.
    let landed = in_mode(table.spin_at(Spin::OBSERVED_SIZE, SpinStatus::Rolled, 0, 5, Some(trailer(observe::RESULT, Pubkey::new_unique(), [0; 64]))), MODE_LINES);
    let lost = table.seeded(landed.clone(), seed_paying(&landed, false));
    let result = with_stand_ins().process_instruction(&ix, &pay_accounts(&ix, lost));
    assert_eq!(result.raw_result, Ok(()));
    assert!(calls_to(vault()).is_empty());
    assert_eq!(table.spin_after(&result.resulting_accounts).status, SpinStatus::Collected as u64);
    assert_eq!(table.trailer_after(&result.resulting_accounts).status, observe::SETTLED);
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn only_the_house_s_executor_may_pay() {
    let table = Table::new();
    let ix = pay_collect(&table, 5, 0);
    let mut stranger = ix.clone();
    stranger.accounts[0].pubkey = Pubkey::new_unique();
    let result = with_stand_ins().process_instruction(&stranger, &pay_accounts(&stranger, finished(&table, Pubkey::new_unique())));
    assert_eq!(result.raw_result, core(casino_core::CoreError::Unauthorized));

    let mut unsigned = ix.clone();
    unsigned.accounts[0].is_signer = false;
    let result = with_stand_ins().process_instruction(&unsigned, &pay_accounts(&unsigned, finished(&table, Pubkey::new_unique())));
    assert_eq!(result.raw_result, failure(InstructionError::MissingRequiredSignature));

    // Nor to a ledger that is not the spin's player's.
    let mut elsewhere = ix.clone();
    elsewhere.accounts[4].pubkey = Pubkey::new_unique();
    let result = with_stand_ins().process_instruction(&elsewhere, &pay_accounts(&elsewhere, finished(&table, Pubkey::new_unique())));
    assert_eq!(result.raw_result, core(casino_core::CoreError::InvalidPDA));
    assert!(calls_to(vault()).is_empty());
}

#[test]
fn the_analytics_constant_is_its_derivation() {
    assert_eq!(fixed(slots::constants::ANALYTICS), pda(&[b"analytics"]));
}

/// What a test spin pays on the machine it was bet on.
fn pays_on(spin: &Account) -> u64 {
    let head = bytemuck::from_bytes::<Spin>(&spin.data[..Spin::SIZE]);
    slots::instructions::request_collect::payout(head, bytemuck::from_bytes(&spin.data[Spin::SIZE..Spin::WITH_TERMS])).unwrap()
}

// HasPlayed (28): MagicBlock Quests' lookup. The quest verifier key alone gets an answer — success
// or NotPlayed; everyone else gets the same refusal whether the spin exists or not.

fn verifier() -> Pubkey {
    key(slots::constants::QUEST_VERIFIER.to_bytes())
}

fn has_played(signer: Pubkey, signs: bool, spin: Pubkey, wallet: Pubkey) -> Instruction {
    Instruction::new_with_bytes(
        program(),
        &[&28u64.to_le_bytes()[..], &wallet.to_bytes()].concat(),
        vec![AccountMeta::new_readonly(signer, signs), AccountMeta::new_readonly(spin, false)],
    )
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn the_verifier_learns_whether_a_wallet_has_spun() {
    let table = Table::new();
    let ix = has_played(verifier(), true, table.spin, table.user);
    let played = mollusk().process_instruction(&ix, &[(verifier(), wallet()), (table.spin, table.spin_account(SpinStatus::Collected, 0))]);
    assert_eq!(played.raw_result, Ok(()));
    let none = mollusk().process_instruction(&ix, &[(verifier(), wallet()), (table.spin, Account::default())]);
    assert_eq!(none.raw_result, custom(GameError::NotPlayed));
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn anyone_else_is_refused_the_same_way_spin_or_not() {
    let table = Table::new();
    let stranger = Pubkey::new_unique();
    for spin in [table.spin_account(SpinStatus::Collected, 0), Account::default()] {
        let ix = has_played(stranger, true, table.spin, table.user);
        let result = mollusk().process_instruction(&ix, &[(stranger, wallet()), (table.spin, spin.clone())]);
        assert_eq!(result.raw_result, core(casino_core::CoreError::Unauthorized));
        let claimed = has_played(verifier(), false, table.spin, table.user);
        let result = mollusk().process_instruction(&claimed, &[(verifier(), wallet()), (table.spin, spin)]);
        assert_eq!(result.raw_result, failure(InstructionError::MissingRequiredSignature));
    }
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn the_spin_must_be_the_wallet_s_own() {
    let table = Table::new();
    let other = Table::new();
    let ix = has_played(verifier(), true, other.spin, table.user);
    let result = mollusk().process_instruction(&ix, &[(verifier(), wallet()), (other.spin, other.spin_account(SpinStatus::Collected, 0))]);
    assert_eq!(result.raw_result, core(casino_core::CoreError::InvalidPDA));
}

// ── Referrals ──────────────────────────────────────────────────────────────────────────────

use casino_core::referral::{self, Counters};

/// The books grown to hold the referral counters.
fn grown_books() -> Account {
    let mut account = books();
    account.data.resize(Analytics::REFERRALS_SIZE, 0);
    account
}

fn counters_after(accounts: &[(Pubkey, Account)]) -> Counters {
    let data = &accounts.iter().find(|(k, _)| *k == pda(&[b"analytics"])).unwrap().1.data;
    *bytemuck::from_bytes(&data[Analytics::SIZE..Analytics::REFERRALS_SIZE])
}

fn section_after(table: &Table, accounts: &[(Pubkey, Account)]) -> referral::Referral {
    referral::Referral::read(table.data_after(accounts), Spin::REFERRAL_AT).expect("a referral section")
}

/// ResolveBet naming `referrer` after its consenter, as the receipt carries it.
fn referred_bet(table: &Table, referrer: Pubkey, rest: &[Pubkey]) -> Instruction {
    let mut ix = resolve_bet(table, rest);
    ix.data.extend_from_slice(referrer.as_ref());
    ix
}

fn magic_context() -> Pubkey {
    fixed(casino_core::ids::MAGIC_CONTEXT_ID)
}

/// A spin at the current size whose player is owed `owed`.
fn owed_spin(table: &Table, owed: u64) -> Account {
    let mut account = table.spin_at(Spin::OBSERVED_SIZE, SpinStatus::Collected, 0, 3, None);
    let section = referral::Referral { owed, earned: owed, ..referral::Referral::unlinked() };
    section.write(&mut account.data, Spin::REFERRAL_AT).unwrap();
    account
}

/// Whether the Magic stand-in was asked to schedule a task carrying `bytes`.
fn scheduled(bytes: &[u8]) -> bool {
    calls_to(magic()).iter().any(|d| d[..4] == 6u32.to_le_bytes() && d.windows(bytes.len()).any(|w| w == bytes))
}

fn link_data(amount: u64, generation: u64) -> Vec<u8> {
    let mut data = slots::SlotsInstruction::LINK_REFERRAL.to_le_bytes().to_vec();
    data.extend_from_slice(&amount.to_le_bytes());
    data.extend_from_slice(&generation.to_le_bytes());
    data
}

fn credit_data(amount: u64) -> Vec<u8> {
    let mut data = slots::SlotsInstruction::CREDIT_REFERRAL.to_le_bytes().to_vec();
    data.extend_from_slice(&amount.to_le_bytes());
    data
}

fn linked(account: Pubkey, since: i64) -> referral::Referral {
    referral::Referral { referrer: account.to_bytes(), since, ..referral::Referral::unlinked() }
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn a_first_bet_naming_a_code_schedules_the_link_and_links_nothing_yet() {
    let table = Table::fixed();
    let code = Pubkey::new_unique();
    let ix = referred_bet(&table, code, &[magic_context()]);
    let mut accounts = bet_accounts(&ix, &table, no_spin(), vec![wallet()]);
    accounts[7].1 = grown_books();
    let result = with_stand_ins().process_instruction(&ix, &accounts);
    assert_eq!(result.raw_result, Ok(()));
    assert_eq!(magic_calls(), vec![12, 6], "created, then the link scheduled");
    assert_eq!(calls_to(magic())[0][4..8], (Spin::OBSERVED_SIZE as u32).to_le_bytes());
    assert!(scheduled(&link_data(STAKE / 100, 1)) && scheduled(code.as_ref()));
    assert_eq!(section_after(&table, &result.resulting_accounts).referrer(), None);
    assert_eq!(counters_after(&result.resulting_accounts), Counters { players_unreferred: 1, ..Counters::default() });
    assert_eq!(table.trailer_after(&result.resulting_accounts).stake, STAKE, "the trailer still last");
    println!("resolve_bet, creating a spin naming a code: {} CU", result.compute_units_consumed);
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn a_bet_naming_a_code_without_the_magic_context_is_refused() {
    let table = Table::new();
    let ix = referred_bet(&table, Pubkey::new_unique(), &[]);
    let result = with_stand_ins().process_instruction(&ix, &bet_accounts(&ix, &table, no_spin(), vec![]));
    assert_eq!(result.raw_result, failure(#[allow(deprecated)] InstructionError::NotEnoughAccountKeys));
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn a_code_naming_the_player_s_own_spin_is_not_worth_a_task() {
    let table = Table::new();
    let ix = referred_bet(&table, table.spin, &[magic_context()]);
    let mut accounts = bet_accounts(&ix, &table, no_spin(), vec![wallet()]);
    accounts[7].1 = grown_books();
    let result = with_stand_ins().process_instruction(&ix, &accounts);
    assert_eq!(result.raw_result, Ok(()));
    assert_eq!(magic_calls(), vec![12], "nothing to schedule");
    assert_eq!(counters_after(&result.resulting_accounts).players_unreferred, 1);
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn a_spin_from_before_referrals_grows_unlinked_and_may_link() {
    let table = Table::new();
    let ix = referred_bet(&table, Pubkey::new_unique(), &[magic_context()]);
    let before = table.spin_at(Spin::PRE_REFERRAL_SIZE, SpinStatus::Collected, 0, 4, None);
    let mut accounts = bet_accounts(&ix, &table, before, vec![wallet()]);
    accounts[7].1 = grown_books();
    let result = with_stand_ins().process_instruction(&ix, &accounts);
    assert_eq!(result.raw_result, Ok(()));
    assert_eq!(magic_calls(), vec![13, 6], "grown in place, then the link scheduled");
    assert!(scheduled(&link_data(STAKE / 100, 5)));
    assert_eq!(table.data_after(&result.resulting_accounts).len(), Spin::OBSERVED_SIZE);
    assert_eq!(section_after(&table, &result.resulting_accounts).referrer(), None);
    assert_eq!(table.generation_after(&result.resulting_accounts), 5);
    assert_eq!(counters_after(&result.resulting_accounts), Counters { players_unreferred: 1, ..Counters::default() });
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn a_linked_spin_s_next_bet_credits_its_link_whatever_code_it_names() {
    let table = Table::new();
    let referrer = Table::new();
    let mut before = table.spin_at(Spin::OBSERVED_SIZE, SpinStatus::Collected, 0, 2, None);
    linked(referrer.spin, 9).write(&mut before.data, Spin::REFERRAL_AT).unwrap();
    let ix = referred_bet(&table, Pubkey::new_unique(), &[magic_context()]);
    let mut accounts = bet_accounts(&ix, &table, before, vec![wallet()]);
    accounts[7].1 = grown_books();
    let result = with_stand_ins().process_instruction(&ix, &accounts);
    assert_eq!(result.raw_result, Ok(()));
    assert_eq!(magic_calls(), vec![6]);
    assert!(scheduled(&credit_data(STAKE / 100)) && scheduled(referrer.spin.as_ref()));
    assert!(!scheduled(&link_data(STAKE / 100, 3)));
    let section = section_after(&table, &result.resulting_accounts);
    assert_eq!((section.referrer, section.since), (referrer.spin.to_bytes(), 9));
    assert_eq!(counters_after(&result.resulting_accounts), Counters::default());
    println!("resolve_bet, a linked spin: {} CU", result.compute_units_consumed);
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn books_not_grown_yet_never_stop_a_bet() {
    let table = Table::new();
    let ix = referred_bet(&table, Pubkey::new_unique(), &[magic_context()]);
    let result = with_stand_ins().process_instruction(&ix, &bet_accounts(&ix, &table, no_spin(), vec![wallet()]));
    assert_eq!(result.raw_result, Ok(()));
    assert_eq!(magic_calls(), vec![12, 6]);
}

fn credit(referrer: &Table, amount: u64) -> Instruction {
    let ix = referral::credit_instruction(
        &core_key(program()), &core_key(pda(&[b"house"])), &core_key(referrer.spin), &slots::constants::ANALYTICS,
        slots::SlotsInstruction::CREDIT_REFERRAL, amount, None,
    );
    Instruction::new_with_bytes(
        program(),
        &ix.data,
        ix.accounts.iter().map(|m| AccountMeta { pubkey: fixed(m.pubkey), is_signer: m.is_signer, is_writable: m.is_writable }).collect(),
    )
}

fn credit_accounts(ix: &Instruction, spin: Account) -> Vec<(Pubkey, Account)> {
    let k = |i: usize| ix.accounts[i].pubkey;
    vec![
        (k(0), wallet()), (k(1), owned(vec![])), (k(2), spin), (k(3), grown_books()),
        (k(4), wallet()), (k(5), builtin(magic(), "fake_magic")),
    ]
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn a_referrer_s_spin_from_before_referrals_is_grown_by_its_first_credit() {
    let referrer = Table::new();
    let watched = trailer(observe::RESULT, Pubkey::new_unique(), [3; 64]);
    let before = table_spin(&referrer, Spin::PRE_REFERRAL_SIZE, Some(watched));
    let ix = credit(&referrer, 1_000);
    let result = with_stand_ins().process_instruction(&ix, &credit_accounts(&ix, before));
    assert_eq!(result.raw_result, Ok(()));
    assert_eq!(magic_calls(), vec![13], "grown in place");
    assert_eq!(referrer.trailer_after(&result.resulting_accounts), watched, "the floor's trailer moved with the end");
    assert_eq!(referrer.generation_after(&result.resulting_accounts), 4);
    let section = section_after(&referrer, &result.resulting_accounts);
    assert_eq!((section.referrer(), section.owed, section.earned), (None, 1_000, 1_000));
    assert_eq!(counters_after(&result.resulting_accounts), Counters { players_unreferred: 1, owed: 1_000, earned: 1_000, ..Counters::default() });
    println!("credit_referral, growing the spin: {} CU", result.compute_units_consumed);

    // One without the trailer yet grows too, with nothing for the floor to show.
    let before = table_spin(&referrer, Spin::PERSISTENT_SIZE, None);
    let result = with_stand_ins().process_instruction(&ix, &credit_accounts(&ix, before));
    assert_eq!(result.raw_result, Ok(()));
    assert_eq!(section_after(&referrer, &result.resulting_accounts).owed, 1_000);
    assert_eq!(&referrer.data_after(&result.resulting_accounts)[Spin::OBSERVED_SIZE - observe::SIZE..], &[0; observe::SIZE][..]);
}

fn table_spin(table: &Table, size: usize, observed: Option<Observable>) -> Account {
    table.spin_at(size, SpinStatus::Collected, 0, 4, observed)
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn the_house_s_credit_adds_to_what_the_referrer_is_owed() {
    let referrer = Table::new();
    let ix = credit(&referrer, referral::cut(STAKE));
    let accounts = credit_accounts(&ix, owed_spin(&referrer, 50));
    let result = with_stand_ins().process_instruction(&ix, &accounts);
    assert_eq!(result.raw_result, Ok(()));
    let section = section_after(&referrer, &result.resulting_accounts);
    assert_eq!((section.owed, section.earned), (50 + STAKE / 100, 50 + STAKE / 100));
    assert_eq!(counters_after(&result.resulting_accounts), Counters { owed: STAKE / 100, earned: STAKE / 100, ..Counters::default() });
    println!("credit_referral: {} CU", result.compute_units_consumed);

    let mut stranger = ix.clone();
    stranger.accounts[0].pubkey = Pubkey::new_unique();
    let result = with_stand_ins().process_instruction(&stranger, &credit_accounts(&stranger, owed_spin(&referrer, 50)));
    assert_eq!(result.raw_result, core(casino_core::CoreError::Unauthorized));
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn a_credit_to_a_spin_that_is_gone_is_the_house_s() {
    let referrer = Table::new();
    let ix = credit(&referrer, 1_000);
    let result = with_stand_ins().process_instruction(&ix, &credit_accounts(&ix, no_spin()));
    assert_eq!(result.raw_result, Ok(()));
    assert_eq!(counters_after(&result.resulting_accounts), Counters::default());
}

fn claim(table: &Table) -> Instruction {
    Instruction::new_with_bytes(program(), &30u64.to_le_bytes(), vec![
        AccountMeta::new_readonly(Pubkey::new_unique(), true),
        AccountMeta::new_readonly(table.user, false),
        AccountMeta::new(pda(&[b"house"]), false),
        AccountMeta::new_readonly(table.spin, false),
        AccountMeta::new(magic_context(), false),
        AccountMeta::new_readonly(magic(), false),
    ])
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn anyone_may_claim_and_the_house_schedules_the_payout() {
    let table = Table::new();
    let ix = claim(&table);
    let k = |i: usize| ix.accounts[i].pubkey;
    let accounts = |owed| vec![
        (k(0), wallet()), (k(1), wallet()), (k(2), owned(vec![])), (k(3), owed_spin(&table, owed)),
        (k(4), wallet()), (k(5), builtin(magic(), "fake_magic")),
    ];
    let result = with_stand_ins().process_instruction(&ix, &accounts(700));
    assert_eq!(result.raw_result, Ok(()));
    assert_eq!(magic_calls(), vec![6]);

    let result = with_stand_ins().process_instruction(&ix, &accounts(0));
    assert_eq!(result.raw_result, core(casino_core::CoreError::NothingToCollect));
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn the_house_pays_what_is_owed_once() {
    let table = Table::new();
    let ix = referral::pay_instruction(
        &core_key(program()), &core_key(pda(&[b"house"])), &core_key(table.spin), &core_key(table.user),
        &slots::constants::ANALYTICS, slots::SlotsInstruction::PAY_REFERRAL,
    );
    let ix = Instruction::new_with_bytes(
        program(),
        &ix.data,
        ix.accounts.iter().map(|m| AccountMeta { pubkey: fixed(m.pubkey), is_signer: m.is_signer, is_writable: m.is_writable }).collect(),
    );
    let k = |i: usize| ix.accounts[i].pubkey;
    let mut books = grown_books();
    bytemuck::from_bytes_mut::<Counters>(&mut books.data[Analytics::SIZE..]).owed = 700;
    let accounts = vec![
        (k(0), wallet()), (k(1), owned(vec![])), (k(2), wallet()), (k(3), owed_spin(&table, 700)),
        (k(4), wallet()), (k(5), builtin(vault(), "fake_vault")), (k(6), books),
    ];
    let result = with_stand_ins().process_instruction(&ix, &accounts);
    assert_eq!(result.raw_result, Ok(()));
    let settles = calls_to(vault());
    assert_eq!(settles.len(), 1);
    assert_eq!(u64::from_le_bytes(settles[0][settles[0].len() - 8..].try_into().unwrap()), 700);
    let section = section_after(&table, &result.resulting_accounts);
    assert_eq!((section.owed, section.earned), (0, 700));
    assert_eq!(counters_after(&result.resulting_accounts).owed, 0);
    println!("pay_referral: {} CU", result.compute_units_consumed);

    // Fired again with nothing owed, it pays nothing.
    let mut accounts = accounts;
    accounts[3].1 = owed_spin(&table, 0);
    let result = with_stand_ins().process_instruction(&ix, &accounts);
    assert_eq!(result.raw_result, Ok(()));
    assert!(calls_to(vault()).is_empty());
}

fn close_spin(table: &Table) -> Instruction {
    let mut data = 7u64.to_le_bytes().to_vec();
    data.extend_from_slice(table.user.as_ref());
    Instruction::new_with_bytes(program(), &data, vec![
        AccountMeta::new_readonly(fixed(casino_core::ids::OPS_KEY), true),
        AccountMeta::new(pda(&[b"house"]), false),
        AccountMeta::new(table.spin, false),
        AccountMeta::new(fixed(casino_core::ids::EPHEMERAL_VAULT_ID), false),
        AccountMeta::new_readonly(magic(), false),
    ])
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn a_spin_still_owed_is_not_closed_until_it_is_paid() {
    let table = Table::new();
    let ix = close_spin(&table);
    let k = |i: usize| ix.accounts[i].pubkey;
    let accounts = |owed| vec![(k(0), wallet()), (k(1), owned(vec![])), (k(2), owed_spin(&table, owed)), (k(3), wallet()), (k(4), builtin(magic(), "fake_magic"))];
    let result = with_stand_ins().process_instruction(&ix, &accounts(700));
    assert_eq!(result.raw_result, custom(GameError::StillOwed));
    assert!(magic_calls().is_empty());
    let result = with_stand_ins().process_instruction(&ix, &accounts(0));
    assert_eq!(result.raw_result, Ok(()));
    assert_eq!(magic_calls(), vec![14], "closed once nothing is owed");
}

fn as_test(ix: casino_core::chain::Instruction) -> Instruction {
    Instruction::new_with_bytes(
        program(),
        &ix.data,
        ix.accounts.iter().map(|m| AccountMeta { pubkey: fixed(m.pubkey), is_signer: m.is_signer, is_writable: m.is_writable }).collect(),
    )
}

fn link_task(code: Pubkey, player: &Table, amount: u64) -> Instruction {
    as_test(referral::link_instruction(
        &core_key(program()), &core_key(pda(&[b"house"])), &core_key(code), &core_key(player.spin),
        slots::SlotsInstruction::LINK_REFERRAL, amount, 3,
    ))
}

fn link_accounts(ix: &Instruction, code: Account, spin: Account) -> Vec<(Pubkey, Account)> {
    let k = |i: usize| ix.accounts[i].pubkey;
    vec![(k(0), wallet()), (k(1), owned(vec![])), (k(2), code), (k(3), spin), (k(4), wallet()), (k(5), builtin(magic(), "fake_magic"))]
}

/// Any account holding a wallet at the owner's place: another game's, here a stranger program's.
fn foreign_code(owner: Pubkey) -> Account {
    let mut data = vec![7u8; 96];
    data[referral::OWNER_AT..referral::OWNER_AT + 32].copy_from_slice(owner.as_ref());
    Account { lamports: 1_000_000_000, data, owner: Pubkey::new_unique(), executable: false, rent_epoch: 0 }
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn the_link_task_finds_the_code_owner_s_spin_here_and_schedules_the_linking_credit() {
    let player = Table::new();
    let referrer = Table::new();
    let ix = link_task(Pubkey::new_unique(), &player, 500);
    let spin = || owed_spin(&player, 0);
    let result = with_stand_ins().process_instruction(&ix, &link_accounts(&ix, foreign_code(referrer.user), spin()));
    assert_eq!(result.raw_result, Ok(()));
    assert_eq!(magic_calls(), vec![6]);
    assert!(scheduled(&credit_data(500)) && scheduled(referrer.spin.as_ref()) && scheduled(player.spin.as_ref()));
    println!("link_referral: {} CU", result.compute_units_consumed);

    for code in [foreign_code(player.user), foreign_code(Pubkey::default()), wallet()] {
        let result = with_stand_ins().process_instruction(&ix, &link_accounts(&ix, code, spin()));
        assert_eq!(result.raw_result, Ok(()));
        assert!(magic_calls().is_empty(), "the player, no one, or nothing at all links nothing");
    }

    let mut stranger = ix.clone();
    stranger.accounts[0].pubkey = Pubkey::new_unique();
    let result = with_stand_ins().process_instruction(&stranger, &link_accounts(&stranger, foreign_code(referrer.user), spin()));
    assert_eq!(result.raw_result, core(casino_core::CoreError::Unauthorized));
}

fn linking_credit(referrer: &Table, player: &Table, amount: u64) -> Instruction {
    as_test(referral::credit_instruction(
        &core_key(program()), &core_key(pda(&[b"house"])), &core_key(referrer.spin), &slots::constants::ANALYTICS,
        slots::SlotsInstruction::CREDIT_REFERRAL, amount, Some(&core_key(player.spin)),
    ))
}

fn linking_accounts(ix: &Instruction, referrer_spin: Account, player_spin: Account) -> Vec<(Pubkey, Account)> {
    let mut accounts = credit_accounts(ix, referrer_spin);
    let mut unreferred = grown_books();
    bytemuck::from_bytes_mut::<Counters>(&mut unreferred.data[Analytics::SIZE..Analytics::REFERRALS_SIZE]).players_unreferred = 1;
    accounts[3].1 = unreferred;
    accounts.push((ix.accounts[6].pubkey, player_spin));
    accounts
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn the_linking_credit_links_only_to_a_spin_that_exists_then_credits_it() {
    let player = Table::new();
    let referrer = Table::new();
    let ix = linking_credit(&referrer, &player, 500);
    let with_section = |section: referral::Referral| {
        let mut account = owed_spin(&player, 0);
        section.write(&mut account.data, Spin::REFERRAL_AT).unwrap();
        account
    };

    let result = with_stand_ins().process_instruction(&ix, &linking_accounts(&ix, owed_spin(&referrer, 0), owed_spin(&player, 0)));
    assert_eq!(result.raw_result, Ok(()));
    assert_eq!(section_after(&player, &result.resulting_accounts).referrer, referrer.spin.to_bytes());
    assert_eq!(section_after(&referrer, &result.resulting_accounts).owed, 500);
    assert_eq!(counters_after(&result.resulting_accounts),
        Counters { players_referred: 1, players_unreferred: 0, owed: 500, earned: 500 });
    println!("credit_referral, linking: {} CU", result.compute_units_consumed);

    let result = with_stand_ins().process_instruction(&ix, &linking_accounts(&ix, no_spin(), owed_spin(&player, 0)));
    assert_eq!(result.raw_result, Ok(()));
    assert_eq!(section_after(&player, &result.resulting_accounts).referrer(), None, "no spin, no link");
    assert_eq!(counters_after(&result.resulting_accounts).players_referred, 0);

    let result = with_stand_ins().process_instruction(&ix, &linking_accounts(&ix, owed_spin(&referrer, 0), with_section(linked(Pubkey::new_unique(), 4))));
    assert_eq!(result.raw_result, Ok(()));
    assert_eq!(section_after(&referrer, &result.resulting_accounts).owed, 0, "linked to someone else since");

    let result = with_stand_ins().process_instruction(&ix, &linking_accounts(&ix, owed_spin(&referrer, 0), with_section(linked(referrer.spin, 4))));
    assert_eq!(result.raw_result, Ok(()));
    assert_eq!(section_after(&player, &result.resulting_accounts).since, 4);
    assert_eq!(section_after(&referrer, &result.resulting_accounts).owed, 500, "linked to it before: credited once");

    let at_no_cut = linking_credit(&referrer, &player, 0);
    let result = with_stand_ins().process_instruction(&at_no_cut, &linking_accounts(&at_no_cut, owed_spin(&referrer, 0), owed_spin(&player, 0)));
    assert_eq!(result.raw_result, Ok(()));
    assert_eq!(section_after(&player, &result.resulting_accounts).referrer, referrer.spin.to_bytes());
    assert_eq!(section_after(&referrer, &result.resulting_accounts).earned, 0);
}
