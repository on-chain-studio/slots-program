use borsh::BorshDeserialize;
use ephemeral_rollups_sdk::access_control::instructions::{CreatePermissionCpiBuilder, UpdatePermissionCpiBuilder};
use ephemeral_rollups_sdk::access_control::structs::{Member, MembersArgs};
use solana_program::{account_info::AccountInfo, program_error::ProgramError, pubkey::Pubkey, entrypoint::ProgramResult, program::{invoke, invoke_signed}};
use solana_system_interface::instruction as system_instruction;

use crate::constants::{is_admin, ADMIN_PUBKEYS, PERMISSION_PROGRAM, TREASURIES, VAULT_PROGRAM};
use crate::error::GameError;
use crate::instruction::ProcessInstruction;
use crate::state::analytics::{self, Analytics};
use crate::state::config::{self, Config, INITIAL_MACHINES};
use crate::utils::pda;

/// Creates `["config"]`, the house PDA and `["analytics"]`, and sets the machine count. Admin
/// only, idempotent — re-running it after a `GrowConfig` must not shrink the shelf or re-create
/// a treasury that already holds the float.
///
/// The house is program-owned so it can be delegated to the rollup: it is the payer inside it,
/// sponsoring each spin's ephemeral rent and the VRF. Analytics is delegated the same way, since
/// the settle callbacks that write it run there; its TEE permission names the admins.
/// Accounts: [initializer, config, house, analytics, permission, permission_program,
///            system_program]
#[derive(BorshDeserialize)]
pub struct Initialize {
    pub machine_count: u8,
}

impl ProcessInstruction for Initialize {
    fn process(&self, program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
        let [initializer, config_account, treasuries @ ..] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };
        let [_, _, _, analytics_account, permission, permission_program, system_program, ..] =
            accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        if !initializer.is_signer || !is_admin(initializer.key) {
            return Err(ProgramError::MissingRequiredSignature);
        }
        if *permission_program.key != PERMISSION_PROGRAM {
            return Err(GameError::InvalidPDA.into());
        }
        let config_bump = pda::validate(program_id, config_account, &[b"config"])?;

        let initial = Config::size_for(INITIAL_MACHINES);
        if config_account.data_len() == 0 {
            invoke_signed(
                &system_instruction::create_account(
                    initializer.key,
                    config_account.key,
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
                        initializer.key,
                        account.key,
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
                    initializer.key,
                    analytics_account.key,
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

        // The TEE permission: counters are the house's books, so only the admins may read the
        // live copy. Both programs are members because the rollup admits an instruction that
        // touches a permissioned account only when the *invoked program* is a member — and the
        // settle that writes these counters is a vault instruction, exactly the reason every
        // ledger's permission names the vault and the game alike. The set is enforced on every
        // run (update when the account already exists), so changing it is one re-run away.
        let mut members = vec![
            Member { flags: 0, pubkey: *program_id },
            Member { flags: 0, pubkey: VAULT_PROGRAM },
        ];
        members.extend(ADMIN_PUBKEYS.iter().map(|k| Member { flags: 0, pubkey: *k }));
        let args = MembersArgs { members: Some(members) };
        let seeds: &[&[u8]] = &[b"analytics", &[analytics_bump]];
        if permission.data_len() == 0 {
            CreatePermissionCpiBuilder::new(permission_program)
                .permissioned_account(analytics_account)
                .permission(permission)
                .payer(initializer)
                .system_program(system_program)
                .args(args)
                .invoke_signed(&[seeds])
                .map_err(|_| ProgramError::InvalidAccountData)?;
        } else {
            UpdatePermissionCpiBuilder::new(permission_program)
                .authority(analytics_account, false)
                .permissioned_account(analytics_account, true)
                .permission(permission)
                .args(args)
                .invoke_signed(&[seeds])
                .map_err(|_| ProgramError::InvalidAccountData)?;
        }

        // Grow a short shelf up to the starting size, never shrink — it may hold live machines.
        // Rent must come with the growth or the resize fails the runtime's exemption check.
        if config_account.data_len() < initial {
            let required = pda::min_balance(initial);
            let held = config_account.lamports();
            if held < required {
                invoke(
                    &system_instruction::transfer(
                        initializer.key, config_account.key, required - held,
                    ),
                    &[initializer.clone(), config_account.clone(), system_program.clone()],
                )?;
            }
            config_account.resize(initial)?;
        }

        let count = self.machine_count as u64;
        if count as usize > Config::capacity(config_account) {
            return Err(GameError::ShelfFull.into());
        }

        let c = Config::load_mut(config_account)?;
        c.discriminator = config::DISCRIMINATOR;
        c.version = config::VERSION;
        c.authority = initializer.key.to_bytes();
        // Never shrinks: a machine someone can be mid-bet on must not vanish from under them.
        // Retiring one is a `SetMachine` that rewrites the slot, not a lower count.
        c.machine_count = c.machine_count.max(count);

        Ok(())
    }
}
