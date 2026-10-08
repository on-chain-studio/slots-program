use borsh::{BorshDeserialize, BorshSerialize};
use casino_core::chain::*;
use casino_core::magicblock::EPHEMERAL_VAULT_ID;
use casino_core::{observe, pda, receipt, CoreError};

use crate::instructions::request_collect::payout;
use crate::state::analytics::Analytics;
use crate::state::spin::{Spin, SpinStatus};

/// The settle callback marks the retained result as paid.
/// Accounts: [receipt, vault_authority (signer), house, spin, ephemeral_vault, magic_program,
///            analytics]
#[derive(BorshDeserialize, BorshSerialize)]
pub struct ResolveCollect {
    pub human: Pubkey,
}

impl ResolveCollect {
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub fn process<'a>(
        &self,
        _receipt_account: &AccountInfo,
        vault_authority: &AccountInfo,
        house: &AccountInfo,
        spin_account: &AccountInfo,
        ephemeral_vault: &AccountInfo,
        _magic_program: &AccountInfo,
        analytics_account: &AccountInfo,
    ) -> ProgramResult {
        let program_id = &crate::ID;

        if *ephemeral_vault.address() != EPHEMERAL_VAULT_ID {
            return Err(CoreError::InvalidPDA.into());
        }
        pda::validate(program_id, house, &[b"house"])?;

        receipt::require_callback(vault_authority)?;

        pda::validate(program_id, spin_account, &[b"spin", self.human.as_ref()])?;
        let (machine_id, amount, mint) = {
            let terms = *Spin::terms(spin_account)?;
            let spin = Spin::load(spin_account)?;
            if spin.user != self.human.to_bytes() {
                return Err(CoreError::Unauthorized.into());
            }
            if spin.status != SpinStatus::Rolled as u64 {
                return Err(CoreError::WrongStatus.into());
            }
            (spin.machine_id, payout(spin, &terms)?, terms.mint)
        };

        pda::validate(program_id, analytics_account, &[b"analytics"])?;
        let a = Analytics::load_mut(analytics_account)?;
        if amount > 0 {
            casino_core::analytics::record_payout(&mut a.payouts, &mint, amount);
        }
        if let Some(slot) = a.bets_collected.get_mut(machine_id as usize) {
            casino_core::analytics::count(slot);
        }

        Spin::load_mut(spin_account)?.status = SpinStatus::Collected as u64;
        Spin::observe(spin_account, observe::SETTLED)
    }
}
