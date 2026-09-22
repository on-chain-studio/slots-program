//! The chain, as this program uses it — on Pinocchio, under the names the program was written
//! with. An account is a view straight onto the runtime's input rather than a copy of it, and a
//! call to another program is built the way it always was: an `Instruction` of `AccountMeta`s and
//! bytes, handed the accounts it names in any order.

use pinocchio::cpi::{invoke_signed_unchecked, CpiAccount, Seed, Signer};
use pinocchio::instruction::{InstructionAccount, InstructionView};

pub use pinocchio::account::RefMut;
pub use pinocchio::cpi::set_return_data;
pub use pinocchio::error::ProgramError;
pub use pinocchio::{AccountView as AccountInfo, Address as Pubkey, ProgramResult};

/// How far an account may grow in one instruction.
pub const MAX_PERMITTED_DATA_INCREASE: usize = 10_240;

pub const SYSTEM_PROGRAM: Pubkey = Pubkey::from_str_const("11111111111111111111111111111111");

/// The account operations the program was written against, on an account view. A view is a
/// pointer into the runtime's input, so writing through a copy of one writes the account.
pub trait AccountExt {
    fn try_borrow_mut_data<'a>(&'a self) -> Result<RefMut<'a, [u8]>, ProgramError>;
    fn resize(&self, new_len: usize) -> ProgramResult;
}

impl AccountExt for AccountInfo {
    #[inline(always)]
    fn try_borrow_mut_data<'a>(&'a self) -> Result<RefMut<'a, [u8]>, ProgramError> {
        let mut view = *self;
        let data = view.try_borrow_mut()?;
        // SAFETY: the guard points into the runtime's copy of the account and at its borrow flag,
        // not into `view`, so it is good for as long as the account it was taken from.
        Ok(unsafe { core::mem::transmute::<RefMut<'_, [u8]>, RefMut<'a, [u8]>>(data) })
    }

    #[inline(always)]
    fn resize(&self, new_len: usize) -> ProgramResult {
        let mut view = *self;
        pinocchio::Resize::resize(&mut view, new_len)
    }
}

#[derive(Clone)]
pub struct AccountMeta {
    pub pubkey: Pubkey,
    pub is_signer: bool,
    pub is_writable: bool,
}

impl AccountMeta {
    pub fn new(pubkey: Pubkey, is_signer: bool) -> Self {
        Self { pubkey, is_signer, is_writable: true }
    }

    pub fn new_readonly(pubkey: Pubkey, is_signer: bool) -> Self {
        Self { pubkey, is_signer, is_writable: false }
    }
}

pub struct Instruction {
    pub program_id: Pubkey,
    pub accounts: Vec<AccountMeta>,
    pub data: Vec<u8>,
}

pub fn invoke(instruction: &Instruction, accounts: &[AccountInfo]) -> ProgramResult {
    invoke_signed(instruction, accounts, &[])
}

/// Calls `instruction.program_id`, as `solana-program`'s `invoke_signed` does: each account the
/// instruction names is borrow-checked where it is first found in `accounts`, and `accounts` goes
/// to the runtime as given — in any order, with others besides — for the runtime to match. An
/// account the instruction names and `accounts` lacks is the runtime's to refuse, not ours.
pub fn invoke_signed(
    instruction: &Instruction,
    accounts: &[AccountInfo],
    signers_seeds: &[&[&[u8]]],
) -> ProgramResult {
    for meta in &instruction.accounts {
        if let Some(account) = accounts.iter().find(|account| *account.address() == meta.pubkey) {
            if meta.is_writable {
                account.check_borrow_mut()?;
            } else {
                account.check_borrow()?;
            }
        }
    }
    let metas: Vec<InstructionAccount> = instruction
        .accounts
        .iter()
        .map(|meta| InstructionAccount::new(&meta.pubkey, meta.is_writable, meta.is_signer))
        .collect();
    let cpi_accounts: Vec<CpiAccount> = accounts.iter().map(CpiAccount::from).collect();
    let seeds: Vec<Vec<Seed>> = signers_seeds
        .iter()
        .map(|seeds| seeds.iter().map(|seed| Seed::from(*seed)).collect())
        .collect();
    let signers: Vec<Signer> = seeds.iter().map(|seeds| Signer::from(seeds.as_slice())).collect();
    // SAFETY: every account the instruction names that is here has just passed the borrow check
    // for how the instruction uses it.
    unsafe {
        invoke_signed_unchecked(
            &InstructionView {
                program_id: &instruction.program_id,
                data: &instruction.data,
                accounts: &metas,
            },
            &cpi_accounts,
            &signers,
        );
    }
    Ok(())
}

/// The system program's instructions, byte for byte as `solana-system-interface` builds them.
pub mod system_instruction {
    use super::*;

    pub fn create_account(from: &Pubkey, to: &Pubkey, lamports: u64, space: u64, owner: &Pubkey) -> Instruction {
        let mut data = Vec::with_capacity(52);
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&lamports.to_le_bytes());
        data.extend_from_slice(&space.to_le_bytes());
        data.extend_from_slice(owner.as_ref());
        Instruction {
            program_id: SYSTEM_PROGRAM,
            accounts: vec![AccountMeta::new(*from, true), AccountMeta::new(*to, true)],
            data,
        }
    }

    pub fn transfer(from: &Pubkey, to: &Pubkey, lamports: u64) -> Instruction {
        let mut data = Vec::with_capacity(12);
        data.extend_from_slice(&2u32.to_le_bytes());
        data.extend_from_slice(&lamports.to_le_bytes());
        Instruction {
            program_id: SYSTEM_PROGRAM,
            accounts: vec![AccountMeta::new(*from, true), AccountMeta::new(*to, false)],
            data,
        }
    }
}
