use borsh::{BorshDeserialize, BorshSerialize};
use crate::chain::*;

use crate::constants::is_admin;
use crate::utils::{pda, vault};

/// House only — deliberately no jackpot equivalent.
/// Accounts: [admin (signer), house, house_ledger, admin_ledger, vault_program]
#[derive(BorshDeserialize, BorshSerialize)]
pub struct WithdrawHouse {
    pub mint: Pubkey,
    pub amount: u64,
}


impl WithdrawHouse {
    #[inline(always)]
    pub fn process<'a>(
        &self,
        admin: &AccountInfo,
        house: &AccountInfo,
        house_ledger: &AccountInfo,
        admin_ledger: &AccountInfo,
        vault_program: &AccountInfo,
    ) -> ProgramResult {
        let program_id = &crate::ID;

        if !admin.is_signer() || !is_admin(admin.address()) {
            return Err(ProgramError::MissingRequiredSignature);
        }
        let house_bump = pda::validate(program_id, house, &[b"house"])?;

        vault::settle(
            vault_program, house_ledger, admin_ledger, house, admin,
            house.address(),
            &[b"house", &[house_bump]],
            &self.mint, self.amount,
        )
    }
}
