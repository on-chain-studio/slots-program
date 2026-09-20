use solana_program::{account_info::AccountInfo, program_error::ProgramError, pubkey::Pubkey, entrypoint::ProgramResult};
use crate::error::GameError;

pub fn validate(
    program_id: &Pubkey,
    pda_account: &AccountInfo,
    pda_seeds: &[&[u8]],
) -> Result<u8, ProgramError> {
    let (pda, bump) = Pubkey::find_program_address(pda_seeds, program_id);
    if &pda != pda_account.key {
        return Err(GameError::InvalidPDA.into());
    }
    Ok(bump)
}

pub fn close(receiver: &AccountInfo, account_to_close: &AccountInfo) -> ProgramResult {
    let lamports = account_to_close.lamports();
    **receiver.try_borrow_mut_lamports()? += lamports;
    **account_to_close.try_borrow_mut_lamports()? = 0;
    account_to_close.data.borrow_mut().fill(0);
    Ok(())
}

/// Approximate rent-exempt minimum balance.
/// (space + 128) × 3480 lamports/byte/year × 2 years of exemption.
pub const fn min_balance(space: usize) -> u64 {
    ((space as u64) + 128) * 6960
}

/// Moves lamports between accounts this program owns.
pub fn transfer_owned(from: &AccountInfo, to: &AccountInfo, lamports: u64) -> ProgramResult {
    let mut from_l = from.try_borrow_mut_lamports()?;
    if **from_l < lamports {
        return Err(GameError::InsufficientFunds.into());
    }
    **from_l -= lamports;
    **to.try_borrow_mut_lamports()? += lamports;
    Ok(())
}
