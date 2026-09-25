use borsh::{BorshDeserialize, BorshSerialize};
use casino_core::admin::close_player_account;
use casino_core::chain::*;

use crate::Slots;

/// Drops a spin and returns its rent to the house. Admin only — a spin is paid-for, so closing
/// one at will would destroy a player's bet; this is the escape hatch for a stranded one.
/// Accounts: [admin (signer), house, spin, ephemeral_vault, magic_program]
#[derive(BorshDeserialize, BorshSerialize)]
pub struct CloseSpin {
    pub user: Pubkey,
}

impl CloseSpin {
    #[inline(always)]
    pub fn process(
        &self,
        admin: &AccountInfo,
        house: &AccountInfo,
        spin_account: &AccountInfo,
        ephemeral_vault: &AccountInfo,
        magic_program: &AccountInfo,
    ) -> ProgramResult {
        close_player_account::<Slots>(b"spin", &self.user, admin, house, spin_account, ephemeral_vault, magic_program)
    }
}
