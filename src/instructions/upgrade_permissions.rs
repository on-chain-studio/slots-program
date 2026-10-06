use casino_core::chain::*;
use casino_core::magicblock::MEMBER_READ;
use casino_core::{pda, permission};

use crate::instructions::resolve_bet::spin_members;

pub struct UpgradePermissions;

impl UpgradePermissions {
    #[allow(clippy::too_many_arguments)]
    pub fn process(&self, user: &AccountInfo, spin: &AccountInfo, spin_permission: &AccountInfo,
        house: &AccountInfo, ephemeral_vault: &AccountInfo, magic_program: &AccountInfo,
        permission_program: &AccountInfo) -> ProgramResult {
        let spin_bump = pda::validate(&crate::ID, spin, &[b"spin", user.address().as_ref()])?;
        let house_bump = pda::validate(&crate::ID, house, &[b"house"])?;
        if spin_permission.data_len() == 0 {
            return Ok(());
        }
        if spin_permission.address() != &permission::address(spin.address()) {
            return Err(ProgramError::InvalidSeeds);
        }
        permission::upgrade_ephemeral(
            &crate::ID, permission_program, spin, &[b"spin", user.address().as_ref(), &[spin_bump]],
            spin_permission, house, &[b"house", &[house_bump]], ephemeral_vault, magic_program,
            spin_members(user.address()), MEMBER_READ,
        )
    }
}
