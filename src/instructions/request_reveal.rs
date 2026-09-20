use borsh::BorshDeserialize;
use solana_program::{account_info::AccountInfo, program_error::ProgramError, pubkey::Pubkey, entrypoint::ProgramResult};

use crate::error::GameError;
use crate::instruction::{ix, ProcessInstruction};
use crate::state::spin::{Spin, SpinStatus};
use crate::utils::{pda, vrf};

/// Asks the VRF for this round's seed. Permissionless and retryable (from `Bought` *or*
/// `Requested`), so a dropped oracle callback cannot strand a paid bet — anyone may re-fire it,
/// and the house pays either way.
/// Accounts: [user, house, spin, identity, oracle_queue, slot_hashes, system_program, vrf_program]
#[derive(BorshDeserialize)]
pub struct RequestReveal;

impl ProcessInstruction for RequestReveal {
    fn process(&self, program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
        let [user, house, spin_account, identity, oracle_queue, slot_hashes,
             system_program, vrf_program, ..] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        let house_bump = pda::validate(program_id, house, &[b"house"])?;
        let identity_bump = pda::validate(program_id, identity, &[b"identity"])?;
        pda::validate(program_id, spin_account, &[b"spin", user.key.as_ref()])?;

        let round = {
            let spin = Spin::load_mut(spin_account)?;
            if spin.user != user.key.to_bytes() {
                return Err(GameError::Unauthorized.into());
            }
            if spin.status != SpinStatus::Bought as u64
                && spin.status != SpinStatus::Requested as u64
            {
                return Err(GameError::WrongStatus.into());
            }
            spin.status = SpinStatus::Requested as u64;
            spin.round
        };

        // The round folded into the caller seed gives each round of the same spin its own
        // entropy stream — the account key alone would repeat per round, and per bet.
        let mut caller_seed = spin_account.key.to_bytes();
        for (i, b) in round.to_le_bytes().iter().enumerate() {
            caller_seed[i] ^= b;
        }

        vrf::request_randomness(
            program_id, house, identity, identity_bump, oracle_queue, system_program,
            slot_hashes, vrf_program,
            caller_seed,
            ix::CallbackReveal.to_le_bytes(),
            vec![vrf::SerializableAccountMeta {
                pubkey: *spin_account.key,
                is_signer: false,
                is_writable: true,
            }],
            // The round rides the callback so a stale callback from an earlier round is refused
            // rather than landing as this round's seed. That matters here in a way it did not for
            // scratch cards: the player *decides* between rounds, so an already-seen seed must
            // never be allowed to become a later round's randomness.
            round.to_le_bytes().to_vec(),
            &[b"house", &[house_bump]],
            true,
        )
    }
}
