use crate::chain::*;
use crate::error::GameError;

pub fn validate(
    program_id: &Pubkey,
    pda_account: &AccountInfo,
    pda_seeds: &[&[u8]],
) -> Result<u8, ProgramError> {
    let (pda, bump) = Pubkey::find_program_address(pda_seeds, program_id);
    if &pda != pda_account.address() {
        return Err(GameError::InvalidPDA.into());
    }
    Ok(bump)
}

/// Approximate rent-exempt minimum balance.
/// (space + 128) × 3480 lamports/byte/year × 2 years of exemption.
pub const fn min_balance(space: usize) -> u64 {
    ((space as u64) + 128) * 6960
}
