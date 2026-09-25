//! The built program, run. `layout.rs` pins what the bytes mean; these check that the deployed
//! artifact dispatches and refuses exactly as it always has — the same accounts in the same order,
//! the same errors for the same mistakes. They load `target/deploy/slots.so`, so they need
//! `cargo build-sbf` first, and are ignored otherwise:
//!
//!     cargo build-sbf && cargo test --test program -- --ignored
//!
//! Point `SBF_OUT_DIR` at another build to run the same checks against it.

use bytemuck::Zeroable;
use mollusk_svm::Mollusk;
use pinocchio::error::ProgramError;
use solana_account::Account;
use solana_instruction::{error::InstructionError, AccountMeta, Instruction};
use solana_pubkey::Pubkey;

use slots::error::GameError;
use slots::instructions::set_machine::{InitSymbol, SetMachine};
use slots::state::config::{MachineConfig, GAMBLE_FAIR, MODE_HOLD};
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
    for unused in [0u64, 15, 24] {
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
    let data = [&19u64.to_le_bytes()[..], &randomness, &round.to_le_bytes(), extra].concat();
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
