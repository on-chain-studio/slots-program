use borsh::{BorshDeserialize, BorshSerialize};
use casino_core::admin::close_player_account;
use casino_core::chain::*;

use crate::error::GameError;
use crate::state::spin::Spin;
use crate::Slots;

/// Drops a spin and returns its rent to the house. Admin only — a spin is paid-for, so closing
/// one at will would destroy a player's bet; this is the escape hatch for a stranded one. A spin
/// whose player is owed referral earnings is refused until they are paid: what is owed lives on
/// the spin, so closing it would hand the referrer's money to the house.
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
        if Spin::referral(spin_account).is_some_and(|r| r.owed > 0) {
            return Err(GameError::StillOwed.into());
        }
        close_player_account::<Slots>(b"spin", &self.user, admin, house, spin_account, ephemeral_vault, magic_program)
    }
}
