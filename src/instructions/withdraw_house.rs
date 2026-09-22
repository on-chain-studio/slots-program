use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::{account_info::AccountInfo, entrypoint::ProgramResult, program_error::ProgramError, pubkey::Pubkey};

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
        admin: &AccountInfo<'a>,
        house: &AccountInfo<'a>,
        house_ledger: &AccountInfo<'a>,
        admin_ledger: &AccountInfo<'a>,
        vault_program: &AccountInfo<'a>,
    ) -> ProgramResult {
        let program_id = &crate::ID;

        if !admin.is_signer || !is_admin(admin.key) {
            return Err(ProgramError::MissingRequiredSignature);
        }
        let house_bump = pda::validate(program_id, house, &[b"house"])?;

        vault::settle(
            vault_program, house_ledger, admin_ledger, house, admin,
            house.key,
            &[b"house", &[house_bump]],
            &self.mint, self.amount,
        )
    }
}
