use borsh::{BorshDeserialize, BorshSerialize};
use ephemeral_rollups_sdk::consts::EPHEMERAL_VAULT_ID;
use solana_program::{account_info::AccountInfo, program_error::ProgramError, pubkey::Pubkey, entrypoint::ProgramResult};

use crate::constants::is_admin;
use crate::error::GameError;
use crate::utils::{pda, receipt};

/// Drops a spin and returns its rent to the house. Admin only — a spin is paid-for, so closing
/// one at will would destroy a player's bet; this is the escape hatch for a stranded one.
/// Accounts: [admin (signer), house, spin, ephemeral_vault, magic_program]
#[derive(BorshDeserialize, BorshSerialize)]
pub struct CloseSpin {
    pub user: Pubkey,
}

impl CloseSpin {
    #[inline(always)]
    pub fn process<'a>(
        &self,
        admin: &AccountInfo<'a>,
        house: &AccountInfo<'a>,
        spin_account: &AccountInfo<'a>,
        ephemeral_vault: &AccountInfo<'a>,
        magic_program: &AccountInfo<'a>,
    ) -> ProgramResult {
        let program_id = &crate::ID;

        if !admin.is_signer || !is_admin(admin.key) {
            return Err(ProgramError::MissingRequiredSignature);
        }
        if *ephemeral_vault.key != EPHEMERAL_VAULT_ID {
            return Err(GameError::InvalidPDA.into());
        }
        let house_bump = pda::validate(program_id, house, &[b"house"])?;
        pda::validate(program_id, spin_account, &[b"spin", self.user.as_ref()])?;

        receipt::close(magic_program, house, spin_account, ephemeral_vault, house_bump)
    }
}
