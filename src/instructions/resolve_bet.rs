use borsh::BorshDeserialize;
use ephemeral_rollups_sdk::consts::EPHEMERAL_VAULT_ID;
use ephemeral_rollups_sdk::ephemeral_accounts::EphemeralAccount;
use solana_program::{account_info::AccountInfo, program_error::ProgramError, pubkey::Pubkey, entrypoint::ProgramResult};

use crate::error::GameError;
use crate::instruction::ProcessInstruction;
use crate::state::analytics::Analytics;
use crate::state::spin::{self, Spin, SpinStatus};
use crate::state::Config;
use crate::utils::{pda, receipt};

/// Turns a settled stake into a spin. The seed comes later via `RequestReveal`, so a failed VRF
/// request cannot unwind a bet that is already paid for.
/// Accounts: [receipt, vault_authority (signer), config, house, spin, ephemeral_vault,
///            magic_program, analytics]
#[derive(BorshDeserialize)]
pub struct ResolveBet {
    pub human: Pubkey,
    pub machine_id: u64,
    /// The session key that consented to the receipt — recorded so hold and gamble can accept
    /// its signature. The vault checked it against the user's ledger before settling, which is
    /// the only authorisation this program could not do itself.
    pub consenter: Pubkey,
}

impl ProcessInstruction for ResolveBet {
    fn process(&self, program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
        let [_receipt_account, vault_authority, config_account, house, spin_account,
             ephemeral_vault, _magic_program, analytics_account, ..] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        if *ephemeral_vault.key != EPHEMERAL_VAULT_ID {
            return Err(GameError::InvalidPDA.into());
        }
        pda::validate(program_id, config_account, &[b"config"])?;
        let house_bump = pda::validate(program_id, house, &[b"house"])?;

        receipt::require_callback(vault_authority)?;

        let spin_bump = pda::validate(
            program_id, spin_account, &[b"spin", self.human.as_ref()],
        )?;
        // The account's existence is the one-bet-at-a-time mutex: a second stake settling while a
        // spin is live fails here — and having failed, the vault's post-CPI assertion unwinds the
        // whole settle, so the player is not charged for a spin that was never created.
        if spin_account.data_len() != 0 {
            return Err(GameError::AlreadyInitialized.into());
        }

        let terms = *Config::machine(config_account, self.machine_id)?;

        EphemeralAccount::new(house, spin_account, ephemeral_vault)
            .with_signer_seeds(&[
                &[b"house", &[house_bump]],
                &[b"spin", self.human.as_ref(), &[spin_bump]],
            ])
            .create(Spin::WITH_TERMS as u32)?;

        {
            let s = Spin::load_mut(spin_account)?;
            s.discriminator = spin::DISCRIMINATOR;
            s.version = spin::VERSION;
            s.user = self.human.to_bytes();
            s.consenter = self.consenter.to_bytes();
            s.machine_id = self.machine_id;
            s.status = SpinStatus::Bought as u64;
            s.round = 0;
            s.hold = 0;
            s.pending = 0;
        }
        Spin::write_terms(spin_account, &terms)?;

        // This callback only fires on a settled stake, so the count is settled money.
        pda::validate(program_id, analytics_account, &[b"analytics"])?;
        let a = Analytics::load_mut(analytics_account)?;
        a.lamports_in = a.lamports_in.saturating_add(terms.stake_lamports);
        if let Some(slot) = a.bets_placed.get_mut(self.machine_id as usize) {
            Analytics::count(slot);
        }

        Ok(())
    }
}
