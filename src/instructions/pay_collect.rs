use borsh::{BorshDeserialize, BorshSerialize};
use casino_core::chain::*;
use casino_core::{observe, pda, task, vault, CoreError};
use casino_core::observe::Observable;

use crate::constants::{ANALYTICS, PAY_ATTEMPTS, PAY_MS};
use crate::instructions::request_collect::payout;
use crate::state::analytics::Analytics;
use crate::state::spin::{Spin, SpinStatus};

/// The house paying a collected spin by itself: a `settle` from the house's ledger to the
/// player's, signed by the house — a credit needs no consent from the side credited — and the spin
/// marked collected. A loss moves nothing and is collected all the same. `RequestCollect`
/// schedules it, so whoever asked, the payout is this program's own task and names ledgers no
/// client or floor transaction could.
///
/// Pays only the grid that was asked for: the spin's generation and round as `RequestCollect` saw
/// them. The task fires several times, so a later fire must not find a newer grid — the player's
/// next bet, or a decision taken after asking — and pay it unasked. Anything but that grid still
/// rolled is left as it is.
/// Accounts: [executor (signer), house, house_ledger, spin, player_ledger, vault_program, analytics]
#[derive(BorshDeserialize, BorshSerialize)]
pub struct PayCollect {
    pub generation: u64,
    pub round: u64,
}

impl PayCollect {
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub fn process(
        &self,
        executor: &AccountInfo,
        house: &AccountInfo,
        house_ledger: &AccountInfo,
        spin_account: &AccountInfo,
        player_ledger: &AccountInfo,
        vault_program: &AccountInfo,
        analytics_account: &AccountInfo,
    ) -> ProgramResult {
        let program_id = &crate::ID;
        if !executor.is_signer() {
            return Err(ProgramError::MissingRequiredSignature);
        }
        let house_bump = pda::validate(program_id, house, &[b"house"])?;
        if *executor.address() != observe::executor(house.address()) {
            return Err(CoreError::Unauthorized.into());
        }
        if *house_ledger.address() != vault::ledger(house.address()) || *analytics_account.address() != ANALYTICS {
            return Err(CoreError::InvalidPDA.into());
        }
        if spin_account.data_len() == 0 || !spin_account.owned_by(program_id) {
            return Ok(());
        }
        if Spin::generation(spin_account)? != self.generation {
            return Ok(());
        }
        let terms = *Spin::terms(spin_account)?;
        let (user, machine_id, amount) = {
            let spin = Spin::load(spin_account)?;
            if spin.status != SpinStatus::Rolled as u64 || spin.round != self.round {
                return Ok(());
            }
            (Pubkey::new_from_array(spin.user), spin.machine_id, payout(spin, &terms)?)
        };
        pda::validate(program_id, spin_account, &[b"spin", user.as_ref()])?;
        if *player_ledger.address() != vault::ledger(&user) {
            return Err(CoreError::InvalidPDA.into());
        }

        if amount > 0 {
            vault::settle(
                vault_program, house_ledger, player_ledger, house, house, house.address(),
                &[b"house", &[house_bump]], &Pubkey::new_from_array(terms.mint), amount,
            )?;
        }
        let a = Analytics::load_mut(analytics_account)?;
        if amount > 0 {
            casino_core::analytics::record_payout(&mut a.payouts, &terms.mint, amount);
        }
        if let Some(slot) = a.bets_collected.get_mut(machine_id as usize) {
            casino_core::analytics::count(slot);
        }

        Spin::load_mut(spin_account)?.status = SpinStatus::Collected as u64;
        match Spin::observable(spin_account) {
            Some(trailer) => Observable { status: observe::SETTLED, flags: 0, paid: amount, ..trailer }.write(spin_account),
            None => Ok(()),
        }
    }
}

/// The task a spin's payout runs under: one per spin, so a newer request replaces one still
/// waiting to fire.
pub fn id(spin: &Pubkey) -> u64 {
    task::id(&[crate::ID.as_ref(), b"collect", spin.as_ref()])
}

/// The `PayCollect` that pays `user`'s spin for the grid at `generation` and `round`.
pub fn instruction(house: &Pubkey, spin: &Pubkey, user: &Pubkey, generation: u64, round: u64) -> Instruction {
    let mut data = crate::SlotsInstruction::PAY_COLLECT.to_le_bytes().to_vec();
    data.extend_from_slice(&generation.to_le_bytes());
    data.extend_from_slice(&round.to_le_bytes());
    Instruction {
        program_id: crate::ID,
        accounts: vec![
            AccountMeta::new_readonly(observe::executor(house), true),
            AccountMeta::new(*house, false),
            AccountMeta::new(vault::ledger(house), false),
            AccountMeta::new(*spin, false),
            AccountMeta::new(vault::ledger(user), false),
            AccountMeta::new_readonly(casino_core::ids::VAULT_PROGRAM, false),
            AccountMeta::new(ANALYTICS, false),
        ],
        data,
    }
}

/// Schedules the house's payout of `spin`, fired at once and again each `PAY_MS` for a ledger
/// that is briefly away.
#[allow(clippy::too_many_arguments)]
pub fn schedule(
    house: &AccountInfo,
    house_seeds: &[&[u8]],
    magic_context: &AccountInfo,
    magic_program: &AccountInfo,
    spin: &Pubkey,
    user: &Pubkey,
    generation: u64,
    round: u64,
) -> ProgramResult {
    let pay = instruction(house.address(), spin, user, generation, round);
    task::schedule(house, house_seeds, magic_context, magic_program, &pay, id(spin), PAY_MS, PAY_ATTEMPTS)
}
