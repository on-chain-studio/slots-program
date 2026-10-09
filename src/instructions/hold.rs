use borsh::{BorshDeserialize, BorshSerialize};
use casino_core::chain::*;
use casino_core::{observe, pda, CoreError};

use crate::error::GameError;
use crate::state::config::MODE_HOLD;
use crate::state::spin::{Spin, SpinStatus};
use crate::utils::engine;

/// Applies the seen grid and commits which reels ride into the respin. Signed by the player or
/// the recorded consenter — never permissionless, or a stranger could choose a victim's holds.
///
/// The ordering is the security property: the mask is committed *here*, and only then can
/// `RequestReveal` ask for the next seed. No instruction exists that can produce round N+1's
/// randomness while round N's decision is open.
/// Accounts: [signer, spin]
#[derive(BorshDeserialize, BorshSerialize)]
pub struct Hold {
    pub mask: u8,
}

impl Hold {
    #[inline(always)]
    pub fn process<'a>(
        &self,
        signer: &AccountInfo,
        spin_account: &AccountInfo,
    ) -> ProgramResult {
        let program_id = &crate::ID;

        if !signer.is_signer() {
            return Err(ProgramError::MissingRequiredSignature);
        }

        let terms = *Spin::terms(spin_account)?;
        let spin = Spin::load_mut(spin_account)?;

        let key = signer.address().to_bytes();
        if key != spin.user && key != spin.consenter {
            return Err(CoreError::Unauthorized.into());
        }
        pda::validate(program_id, spin_account, &[b"spin", spin.user.as_ref()])?;

        if terms.mode != MODE_HOLD {
            return Err(CoreError::WrongStatus.into());
        }
        if spin.status != SpinStatus::Rolled as u64 {
            return Err(GameError::NotRolled.into());
        }
        // The last grid is collected, never held past.
        if spin.round + 1 >= terms.rounds() {
            return Err(GameError::RoundsExhausted.into());
        }
        if u64::from(self.mask) & !terms.reel_mask() != 0 {
            return Err(GameError::InvalidHold.into());
        }

        // Apply the pending seed under the mask committed *before* it was requested — that is
        // what `spin.hold` still holds. Only then does the new mask take its place.
        let stops = engine::spin(&terms, &spin.seed, spin.hold as u8, &spin.stops())?;
        spin.set_stops(&stops);
        spin.seed = [0u8; 32];
        spin.hold = u64::from(self.mask);
        spin.round += 1;
        spin.status = SpinStatus::Bought as u64;

        // Waiting on a seed again, for the same bet and whoever watches it. A round decision
        // takes no more stake, and the last result shows what it paid until the next one lands.
        Spin::observe(spin_account, observe::PENDING)
    }
}
