use borsh::BorshDeserialize;
use solana_program::{account_info::AccountInfo, program_error::ProgramError, pubkey::Pubkey, entrypoint::ProgramResult};

use crate::error::GameError;
use crate::instruction::ProcessInstruction;
use crate::state::config::MODE_HOLD;
use crate::state::spin::{Spin, SpinStatus};
use crate::utils::{engine, pda};

/// Applies the seen grid and commits which reels ride into the respin. Signed by the player or
/// the recorded consenter — never permissionless, or a stranger could choose a victim's holds.
///
/// The ordering is the security property: the mask is committed *here*, and only then can
/// `RequestReveal` ask for the next seed. No instruction exists that can produce round N+1's
/// randomness while round N's decision is open.
/// Accounts: [signer, spin]
#[derive(BorshDeserialize)]
pub struct Hold {
    pub mask: u8,
}

impl ProcessInstruction for Hold {
    fn process(&self, program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
        let [signer, spin_account, ..] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        if !signer.is_signer {
            return Err(ProgramError::MissingRequiredSignature);
        }

        let terms = *Spin::terms(spin_account)?;
        let spin = Spin::load_mut(spin_account)?;

        let key = signer.key.to_bytes();
        if key != spin.user && key != spin.consenter {
            return Err(GameError::Unauthorized.into());
        }
        pda::validate(program_id, spin_account, &[b"spin", spin.user.as_ref()])?;

        if terms.mode != MODE_HOLD {
            return Err(GameError::WrongStatus.into());
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

        Ok(())
    }
}
