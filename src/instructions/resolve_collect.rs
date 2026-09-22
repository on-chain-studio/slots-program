use borsh::{BorshDeserialize, BorshSerialize};
use crate::magicblock::EPHEMERAL_VAULT_ID;
use crate::chain::*;

use crate::error::GameError;
use crate::instructions::request_collect::payout;
use crate::state::analytics::Analytics;
use crate::state::spin::{Spin, SpinStatus};
use crate::utils::{pda, receipt};

/// The settle callback: the payout has moved (or there was none), so the spin closes.
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
        magic_program: &AccountInfo,
        analytics_account: &AccountInfo,
    ) -> ProgramResult {
        let program_id = &crate::ID;

        if *ephemeral_vault.address() != EPHEMERAL_VAULT_ID {
            return Err(GameError::InvalidPDA.into());
        }
        let house_bump = pda::validate(program_id, house, &[b"house"])?;

        receipt::require_callback(vault_authority)?;

        pda::validate(program_id, spin_account, &[b"spin", self.human.as_ref()])?;
        let (machine_id, amount, mint) = {
            let terms = *Spin::terms(spin_account)?;
            let spin = Spin::load(spin_account)?;
            if spin.user != self.human.to_bytes() {
                return Err(GameError::Unauthorized.into());
            }
            // Still `Rolled`: there is no collected state to reach. The spin's existence *is* the
            // unpaid flag, and this callback ends by closing it — a second settle in the same
            // slot finds no account and dies before it can pay twice.
            if spin.status != SpinStatus::Rolled as u64 {
                return Err(GameError::WrongStatus.into());
            }
            // Settled money: the same computation request_collect priced the receipt with,
            // re-derived from the spin before it closes.
            (spin.machine_id, payout(spin, &terms)?, terms.mint)
        };

        pda::validate(program_id, analytics_account, &[b"analytics"])?;
        let a = Analytics::load_mut(analytics_account)?;
        if amount > 0 {
            a.record_payout(&mint, amount);
        }
        if let Some(slot) = a.bets_collected.get_mut(machine_id as usize) {
            Analytics::count(slot);
        }

        receipt::close(magic_program, house, spin_account, ephemeral_vault, house_bump)
    }
}
