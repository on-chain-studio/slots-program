use borsh::{BorshDeserialize, BorshSerialize};
use casino_core::chain::*;
use casino_core::ids::PERMISSION_PROGRAM;
use casino_core::{pda, permission, Casino, CoreError};

use crate::constants::{ANALYTICS_READERS, PRIVATE_CASINO, TREASURIES};
use crate::state::analytics::{self, Analytics};
use crate::state::config::{self, Config, INITIAL_MACHINES};
use crate::Slots;

/// Creates `["config"]`, the house PDA and `["analytics"]`, and sets the machine count. Admin
/// only, idempotent — re-running it after a `GrowConfig` must not shrink the shelf or re-create
/// a treasury that already holds the float.
///
/// The house is program-owned so it can be delegated to the rollup: it is the payer inside it,
/// sponsoring each spin's ephemeral rent and the VRF. Analytics is delegated the same way, since
/// the settle callbacks that write it run there; its TEE permission names the analytics readers.
/// Accounts: [initializer, config, house, analytics, permission, permission_program,
///            system_program]
#[derive(BorshDeserialize, BorshSerialize)]
pub struct Initialize {
    pub machine_count: u8,
}

impl Initialize {
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub fn process<'a>(
        &self,
        initializer: &AccountInfo,
        config_account: &AccountInfo,
        house: &AccountInfo,
        analytics_account: &AccountInfo,
        permission: &AccountInfo,
        permission_program: &AccountInfo,
        system_program: &AccountInfo,
    ) -> ProgramResult {
        let program_id = &crate::ID;
        // One account per `TREASURIES` seed, in order.
        let treasuries: [&AccountInfo; TREASURIES.len()] = [house];

        Slots::require_admin(initializer)?;
        if *permission_program.address() != PERMISSION_PROGRAM {
            return Err(CoreError::InvalidPDA.into());
        }
        let config_bump = pda::validate(program_id, config_account, &[b"config"])?;

        let initial = Config::size_for(INITIAL_MACHINES);
        if config_account.data_len() == 0 {
            invoke_signed(
                &system_instruction::create_account(
                    initializer.address(),
                    config_account.address(),
                    pda::min_balance(initial),
                    initial as u64,
                    program_id,
                ),
                &[initializer.clone(), config_account.clone()],
                &[&[b"config", &[config_bump]]],
            )?;
        }

        for (i, seed) in TREASURIES.iter().enumerate() {
            let Some(account) = treasuries.get(i) else {
                return Err(ProgramError::NotEnoughAccountKeys);
            };
            let bump = pda::validate(program_id, account, &[seed])?;
            if account.data_len() == 0 && account.lamports() == 0 {
                invoke_signed(
                    &system_instruction::create_account(
                        initializer.address(),
                        account.address(),
                        pda::min_balance(0),
                        0,
                        program_id,
                    ),
                    &[initializer.clone(), (*account).clone()],
                    &[&[seed, &[bump]]],
                )?;
            }
        }

        let analytics_bump = pda::validate(program_id, analytics_account, &[b"analytics"])?;
        if analytics_account.data_len() == 0 {
            invoke_signed(
                &system_instruction::create_account(
                    initializer.address(),
                    analytics_account.address(),
                    pda::min_balance(Analytics::SIZE),
                    Analytics::SIZE as u64,
                    program_id,
                ),
                &[initializer.clone(), analytics_account.clone()],
                &[&[b"analytics", &[analytics_bump]]],
            )?;
            let a = Analytics::load_mut(analytics_account)?;
            a.discriminator = analytics::DISCRIMINATOR;
            a.version = analytics::VERSION;
        }

        // The TEE permission: counters are the house's books, so only the analytics readers may
        // read the live copy. Both programs are members because the rollup admits an instruction that
        // touches a permissioned account only when the *invoked program* is a member — and the
        // settle that writes these counters is a vault instruction, exactly the reason every
        // ledger's permission names the vault and the game alike. Made once and never rewritten:
        // an update through the ACL program is not something this program does.
        permission::set(
            permission_program,
            analytics_account,
            &[b"analytics", &[analytics_bump]],
            permission,
            initializer,
            system_program,
            permission::members(program_id, &[ANALYTICS_READERS.as_slice(), &[PRIVATE_CASINO]].concat()),
        )?;

        // Grow a short shelf up to the starting size, never shrink — it may hold live machines.
        // Rent must come with the growth or the resize fails the runtime's exemption check.
        if config_account.data_len() < initial {
            let required = pda::min_balance(initial);
            let held = config_account.lamports();
            if held < required {
                invoke(
                    &system_instruction::transfer(
                        initializer.address(), config_account.address(), required - held,
                    ),
                    &[initializer.clone(), config_account.clone(), system_program.clone()],
                )?;
            }
            config_account.resize(initial)?;
        }

        let count = self.machine_count as u64;
        if count as usize > Config::capacity(config_account) {
            return Err(CoreError::ShelfFull.into());
        }

        let c = Config::load_mut(config_account)?;
        c.discriminator = config::DISCRIMINATOR;
        c.version = config::VERSION;
        c.authority = initializer.address().to_bytes();
        // Never shrinks: a machine someone can be mid-bet on must not vanish from under them.
        // Retiring one is a `SetMachine` that rewrites the slot, not a lower count.
        c.count = c.count.max(count);

        Ok(())
    }
}
