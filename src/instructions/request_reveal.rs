use borsh::{BorshDeserialize, BorshSerialize};
use casino_core::chain::*;
use casino_core::{pda, vrf, CoreError};

use crate::state::spin::{Spin, SpinStatus};

/// Asks the VRF for this round's seed. Permissionless and retryable (from `Bought` *or*
/// `Requested`), so a dropped oracle callback cannot strand a paid bet — anyone may re-fire it,
/// and the house pays either way.
/// Accounts: [user, house, spin, identity, oracle_queue, slot_hashes, system_program, vrf_program]
#[derive(BorshDeserialize, BorshSerialize)]
pub struct RequestReveal;

impl RequestReveal {
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub fn process<'a>(
        &self,
        user: &AccountInfo,
        house: &AccountInfo,
        spin_account: &AccountInfo,
        identity: &AccountInfo,
        oracle_queue: &AccountInfo,
        slot_hashes: &AccountInfo,
        system_program: &AccountInfo,
        vrf_program: &AccountInfo,
    ) -> ProgramResult {
        let program_id = &crate::ID;

        let house_bump = pda::validate(program_id, house, &[b"house"])?;
        let identity_bump = pda::validate(program_id, identity, &[b"identity"])?;
        pda::validate(program_id, spin_account, &[b"spin", user.address().as_ref()])?;

        let generation = Spin::generation(spin_account)?;
        let round = {
            let spin = Spin::load_mut(spin_account)?;
            if spin.user != user.address().to_bytes() {
                return Err(CoreError::Unauthorized.into());
            }
            if spin.status != SpinStatus::Bought as u64
                && spin.status != SpinStatus::Requested as u64
            {
                return Err(CoreError::WrongStatus.into());
            }
            spin.status = SpinStatus::Requested as u64;
            spin.round
        };

        // Unique per bet and round, so no request repeats an earlier input.
        let mut caller_seed = spin_account.address().to_bytes();
        for (i, b) in round.to_le_bytes().iter().enumerate() {
            caller_seed[i] ^= b;
        }
        for (i, b) in generation.to_le_bytes().iter().enumerate() {
            caller_seed[8 + i] ^= b;
        }

        vrf::request_randomness(
            program_id, house, identity, identity_bump, oracle_queue, system_program,
            slot_hashes, vrf_program,
            caller_seed,
            crate::SlotsInstruction::CALLBACK_REVEAL.to_le_bytes(),
            vec![vrf::SerializableAccountMeta {
                pubkey: *spin_account.address(),
                is_signer: false,
                is_writable: true,
            }],
            // Reject delayed answers from earlier bets or rounds.
            [round.to_le_bytes(), generation.to_le_bytes()].concat(),
            &[b"house", &[house_bump]],
            true,
        )
    }
}
