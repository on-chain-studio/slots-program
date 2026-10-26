//! The built program, run. `layout.rs` pins what the bytes mean; these check that the deployed
//! artifact dispatches and refuses exactly as it always has — the same accounts in the same order,
//! the same errors for the same mistakes. They load `target/deploy/slots.so`, so they need
//! `cargo build-sbf` first, and are ignored otherwise:
//!
//!     cargo build-sbf && cargo test --test program -- --ignored
//!
//! Point `SBF_OUT_DIR` at another build to run the same checks against it.

use std::cell::RefCell;

use bytemuck::Zeroable;
use casino_core::referral::Referral;
use mollusk_svm::program::{create_keyed_account_for_builtin_program, Builtin};
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
use slots::state::config::{self, Config, MachineConfig, GAMBLE_FAIR, MODE_HOLD};
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
    let strip: Vec<u8> = (0..20).map(|i| (i % 6) as u8).collect();
    SetMachine {
        index: 0,
        mode: MODE_HOLD,
        stake_lamports: 100_000_000,
        row_count: 3,
        rounds: 3,
        gamble_rungs: 0,
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

// ── the stand-ins ─────────────────────────────────────────────────────────────
//
// The settle callback and the referral tasks call into MagicBlock, which is not here, so stand-ins take the Magic program's and the VRF's addresses and record what they
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



/// The machine's stake, which is a slot bet's whole stake.
const STAKE: u64 = 100_000_000;


/// What a test's spin holds: its head, the terms as they were laid out at `size`, the
/// generation where the size has one, and the referral section with the floor's empty room at the
/// current size.
impl Table {
    fn spin_at(&self, size: usize, status: SpinStatus, round: u64, generation: u64) -> Account {
        let mut account = self.spin_account(status, round);
        if size < Spin::WITH_TERMS {
            // Before run rules, terms were 344 bytes; the head is all a bet reads of them.
            account.data.truncate(Spin::SIZE + 344);
        }
        if [Spin::PERSISTENT_SIZE, Spin::OBSERVED_SIZE, Spin::SIZE + 344 + 8].contains(&size) {
            account.data.extend_from_slice(&generation.to_le_bytes());
        }
        if size == Spin::OBSERVED_SIZE {
            account.data.extend_from_slice(bytemuck::bytes_of(&Referral::unlinked()));
            account.data.resize(Spin::OBSERVED_SIZE, 0);
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

fn core_key(k: Pubkey) -> casino_core::chain::Pubkey {
    casino_core::chain::Pubkey::new_from_array(k.to_bytes())
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
    let mut account = table.spin_at(Spin::OBSERVED_SIZE, SpinStatus::Collected, 0, 3);
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
    let table = Table::new();
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
    let before = table.spin_at(Spin::PERSISTENT_SIZE, SpinStatus::Collected, 0, 4);
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
    let mut before = table.spin_at(Spin::OBSERVED_SIZE, SpinStatus::Collected, 0, 2);
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
    let before = table_spin(&referrer, Spin::PERSISTENT_SIZE);
    let ix = credit(&referrer, 1_000);
    let result = with_stand_ins().process_instruction(&ix, &credit_accounts(&ix, before));
    assert_eq!(result.raw_result, Ok(()));
    assert_eq!(magic_calls(), vec![13], "grown in place");
    assert_eq!(referrer.data_after(&result.resulting_accounts).len(), Spin::OBSERVED_SIZE);
    assert_eq!(referrer.generation_after(&result.resulting_accounts), 4);
    let section = section_after(&referrer, &result.resulting_accounts);
    assert_eq!((section.referrer(), section.owed, section.earned), (None, 1_000, 1_000));
    assert_eq!(counters_after(&result.resulting_accounts), Counters { players_unreferred: 1, owed: 1_000, earned: 1_000, ..Counters::default() });
    println!("credit_referral, growing the spin: {} CU", result.compute_units_consumed);
    assert_eq!(&referrer.data_after(&result.resulting_accounts)[Spin::OBSERVED_SIZE - 64..], &[0; 64][..], "the floor's room left empty");
}

fn table_spin(table: &Table, size: usize) -> Account {
    table.spin_at(size, SpinStatus::Collected, 0, 4)
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
    assert_eq!(result.raw_result, Ok(()), "nothing owed is not refused");
    assert!(magic_calls().is_empty(), "and nothing is scheduled");
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
        AccountMeta::new(pda(&[b"analytics"]), false),
    ])
}

fn counted(referred: u64, unreferred: u64) -> Account {
    let mut books = grown_books();
    *bytemuck::from_bytes_mut::<Counters>(&mut books.data[Analytics::SIZE..Analytics::REFERRALS_SIZE]) =
        Counters { players_referred: referred, players_unreferred: unreferred, ..Counters::default() };
    books
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn a_spin_still_owed_is_not_closed_until_it_is_paid() {
    let table = Table::new();
    let ix = close_spin(&table);
    let k = |i: usize| ix.accounts[i].pubkey;
    let accounts = |spin| vec![(k(0), wallet()), (k(1), owned(vec![])), (k(2), spin), (k(3), wallet()), (k(4), builtin(magic(), "fake_magic")), (k(5), counted(4, 6))];
    let result = with_stand_ins().process_instruction(&ix, &accounts(owed_spin(&table, 700)));
    assert_eq!(result.raw_result, custom(GameError::StillOwed));
    assert!(magic_calls().is_empty());
    let result = with_stand_ins().process_instruction(&ix, &accounts(owed_spin(&table, 0)));
    assert_eq!(result.raw_result, Ok(()));
    assert_eq!(magic_calls(), vec![14], "closed once nothing is owed");
    assert_eq!(counters_after(&result.resulting_accounts), Counters { players_referred: 4, players_unreferred: 5, ..Counters::default() });

    let mut linked_spin = owed_spin(&table, 0);
    linked(Table::new().spin, 3).write(&mut linked_spin.data, Spin::REFERRAL_AT).unwrap();
    let result = with_stand_ins().process_instruction(&ix, &accounts(linked_spin));
    assert_eq!(counters_after(&result.resulting_accounts), Counters { players_referred: 3, players_unreferred: 6, ..Counters::default() }, "a linked player comes off the referred");
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

#[test]
#[ignore = "needs cargo build-sbf"]
fn a_linked_spin_s_bet_without_the_magic_context_still_settles_and_credits_nothing() {
    let table = Table::new();
    let mut before = table.spin_at(Spin::OBSERVED_SIZE, SpinStatus::Collected, 0, 2);
    linked(Table::new().spin, 9).write(&mut before.data, Spin::REFERRAL_AT).unwrap();
    let ix = resolve_bet(&table, &[]);
    let mut accounts = bet_accounts(&ix, &table, before, vec![]);
    accounts[7].1 = grown_books();
    let result = with_stand_ins().process_instruction(&ix, &accounts);
    assert_eq!(result.raw_result, Ok(()));
    assert!(magic_calls().is_empty(), "nothing scheduled");
    assert_eq!(table.data_after(&result.resulting_accounts)[..8], spin::DISCRIMINATOR.to_le_bytes(), "the bet itself stands");
}

#[test]
#[ignore = "needs cargo build-sbf"]
fn a_referrer_s_spin_of_the_oldest_layout_is_rewritten_once_collected_then_linked() {
    let (player, referrer) = (Table::new(), Table::new());
    let legacy = |status: SpinStatus| {
        let mut account = referrer.spin_account(status, 0);
        account.data.truncate(Spin::SIZE);
        account.data.resize(Spin::SIZE + 344, 9);
        account.data.extend_from_slice(&6u64.to_le_bytes());
        account
    };
    let ix = linking_credit(&referrer, &player, 500);
    let result = with_stand_ins().process_instruction(&ix, &linking_accounts(&ix, legacy(SpinStatus::Collected), owed_spin(&player, 0)));
    assert_eq!(result.raw_result, Ok(()));
    assert_eq!(referrer.data_after(&result.resulting_accounts).len(), Spin::OBSERVED_SIZE);
    assert_eq!(referrer.generation_after(&result.resulting_accounts), 6, "the generation kept");
    assert_eq!(section_after(&referrer, &result.resulting_accounts).owed, 500);
    assert_eq!(section_after(&player, &result.resulting_accounts).referrer, referrer.spin.to_bytes());
    assert_eq!(counters_after(&result.resulting_accounts), Counters { players_referred: 1, players_unreferred: 1, owed: 500, earned: 500 });

    let result = with_stand_ins().process_instruction(&ix, &linking_accounts(&ix, legacy(SpinStatus::Rolled), owed_spin(&player, 0)));
    assert_eq!(result.raw_result, Ok(()));
    assert_eq!(referrer.data_after(&result.resulting_accounts).len(), Spin::SIZE + 352, "a bet in play is left alone");
    assert_eq!(section_after(&player, &result.resulting_accounts).referrer(), None, "and nobody is linked to it");
}

