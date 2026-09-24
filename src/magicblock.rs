//! The MagicBlock calls this program makes — delegation, the undelegation callback, ephemeral
//! accounts, commit-and-undelegate, and TEE permissions — through `ephemeral-rollups-pinocchio`,
//! MagicBlock's own crate for Pinocchio programs. Nothing here spells out a wire format; each
//! function only shapes this program's accounts and seeds into the crate's call.
//!
//! Permissions are created, never updated: on the TEE an `UpdateEphemeralPermission` drops the
//! owning program from the list and the rollup then refuses it for good (devnet, 2026-09-25).

use crate::chain::*;
use ephemeral_rollups_pinocchio as erp;
use erp::acl::{EphemeralMembersArgs, Member, MemberFlags, MembersArgs};
use erp::ephemeral_accounts::EphemeralAccount;
use pinocchio::cpi::{Seed, Signer};

pub use erp::acl::PERMISSION_PROGRAM_ID;
pub use erp::consts::{DELEGATION_PROGRAM_ID, EPHEMERAL_VAULT_ID, MAGIC_PROGRAM_ID};

/// The most members a permission here ever names: the program's own seat and a few more.
const MAX_MEMBERS: usize = 8;
const PERMISSION_DATA: usize = erp::acl::data_buffer_size(MAX_MEMBERS);

/// Pinocchio's `Seed`s for one signer's seed list.
fn seeds<'a>(parts: &[&'a [u8]]) -> Vec<Seed<'a>> {
    parts.iter().map(|part| Seed::from(*part)).collect()
}

// ---------------------------------------------------------------------------------------------
// Delegation

/// Hands `pda` to the delegation program, to be run by `validator` until it is undelegated.
#[allow(clippy::too_many_arguments)]
pub fn delegate_account(
    payer: &AccountInfo,
    pda: &AccountInfo,
    owner_program: &AccountInfo,
    buffer: &AccountInfo,
    delegation_record: &AccountInfo,
    delegation_metadata: &AccountInfo,
    delegation_program: &AccountInfo,
    system_program: &AccountInfo,
    pda_seeds: &[&[u8]],
    commit_frequency_ms: u32,
    validator: Option<Pubkey>,
) -> ProgramResult {
    if delegation_program.address() != &DELEGATION_PROGRAM_ID || system_program.address() != &SYSTEM_PROGRAM {
        return Err(ProgramError::IncorrectProgramId);
    }
    let (_, bump) = Pubkey::find_program_address(pda_seeds, owner_program.address());
    let mut accounts = [
        *payer, *pda, *owner_program, *buffer, *delegation_record, *delegation_metadata, *system_program,
    ];
    erp::instruction::delegate_account(
        &mut accounts,
        pda_seeds,
        bump,
        erp::types::DelegateConfig { commit_frequency_ms, validator },
    )
}

/// The delegation program's callback: recreates the account from the buffer it signs for.
pub fn undelegate_account(
    delegated_account: &AccountInfo,
    owner_program: &Pubkey,
    buffer: &AccountInfo,
    payer: &AccountInfo,
    system_program: &AccountInfo,
    account_seeds: &[Vec<u8>],
) -> ProgramResult {
    if system_program.address() != &SYSTEM_PROGRAM {
        return Err(ProgramError::IncorrectProgramId);
    }
    // The crate reads the seeds back out of the callback's own encoding: a count, then each
    // seed length-prefixed — the same bytes the delegation program sent us.
    let mut args = Vec::with_capacity(4 + account_seeds.iter().map(|seed| 4 + seed.len()).sum::<usize>());
    args.extend_from_slice(&(account_seeds.len() as u32).to_le_bytes());
    for seed in account_seeds {
        args.extend_from_slice(&(seed.len() as u32).to_le_bytes());
        args.extend_from_slice(seed);
    }
    let mut delegated = *delegated_account;
    erp::instruction::undelegate(&mut delegated, owner_program, buffer, payer, &args)
}

/// Schedules `committed` to be committed to basenet and undelegated.
pub fn commit_and_undelegate(
    payer: &AccountInfo,
    magic_context: &AccountInfo,
    magic_program: &AccountInfo,
    magic_fee_vault: Option<&AccountInfo>,
    committed: &[AccountInfo],
) -> ProgramResult {
    if magic_program.address() != &MAGIC_PROGRAM_ID {
        return Err(ProgramError::IncorrectProgramId);
    }
    // The intent bundle, not the crate's older schedule-commit call: that one puts the fee
    // vault where the Magic program expects a committed account and is refused on the TEE.
    let mut bundle = erp::intent_bundle::MagicIntentBundleBuilder::new(*payer, *magic_context, *magic_program);
    if let Some(vault) = magic_fee_vault {
        bundle = bundle.magic_fee_vault(*vault);
    }
    let mut data = [0u8; 256];
    bundle.commit_and_undelegate(committed).build_and_invoke(&mut data)
}

// ---------------------------------------------------------------------------------------------
// Ephemeral accounts

/// Creates `ephemeral` inside the rollup, `sponsor` paying its rent into `vault`. Both sign.
pub fn create_ephemeral_account(
    sponsor: &AccountInfo,
    ephemeral: &AccountInfo,
    vault: &AccountInfo,
    magic_program: &AccountInfo,
    data_len: u32,
    signers_seeds: &[&[&[u8]]],
) -> ProgramResult {
    if magic_program.address() != &MAGIC_PROGRAM_ID {
        return Err(ProgramError::IncorrectProgramId);
    }
    let lists: Vec<Vec<Seed>> = signers_seeds.iter().map(|list| seeds(list)).collect();
    let signers: Vec<Signer> = lists.iter().map(|list| Signer::from(list.as_slice())).collect();
    EphemeralAccount::new(sponsor, ephemeral, vault, magic_program).with_signers(&signers).create(data_len)
}

/// Drops `ephemeral`, its rent going back to `sponsor`, who signs.
pub fn close_ephemeral_account(
    sponsor: &AccountInfo,
    ephemeral: &AccountInfo,
    vault: &AccountInfo,
    magic_program: &AccountInfo,
    signers_seeds: &[&[&[u8]]],
) -> ProgramResult {
    if magic_program.address() != &MAGIC_PROGRAM_ID {
        return Err(ProgramError::IncorrectProgramId);
    }
    let lists: Vec<Vec<Seed>> = signers_seeds.iter().map(|list| seeds(list)).collect();
    let signers: Vec<Signer> = lists.iter().map(|list| Signer::from(list.as_slice())).collect();
    EphemeralAccount::new(sponsor, ephemeral, vault, magic_program).with_signers(&signers).close()
}

// ---------------------------------------------------------------------------------------------
// Permissions

/// Members with a flag byte each, in a fixed buffer.
fn members_with(flags: u8, keys: &[Pubkey], buffer: &mut [Member; MAX_MEMBERS]) -> Result<usize, ProgramError> {
    if keys.len() > MAX_MEMBERS {
        return Err(ProgramError::InvalidArgument);
    }
    for (slot, key) in buffer.iter_mut().zip(keys) {
        *slot = Member { flags: MemberFlags::from_acl_flag_byte(flags), pubkey: *key };
    }
    Ok(keys.len())
}

fn empty_members() -> [Member; MAX_MEMBERS] {
    core::array::from_fn(|_| Member { flags: MemberFlags::default(), pubkey: Pubkey::default() })
}

/// A basenet permission on `permissioned`, naming `members` with plain read access. The
/// permissioned account signs by its seeds.
pub fn create_permission(
    permissioned: &AccountInfo,
    permission: &AccountInfo,
    payer: &AccountInfo,
    system_program: &AccountInfo,
    members: &[Pubkey],
    signers_seeds: &[&[&[u8]]],
) -> ProgramResult {
    let mut buffer = empty_members();
    let count = members_with(0, members, &mut buffer)?;
    let list = seeds(signers_seeds.first().ok_or(ProgramError::InvalidArgument)?);
    erp::acl::cpi_create_permission(
        permissioned, permission, payer, system_program, &PERMISSION_PROGRAM_ID,
        MembersArgs { members: Some(&buffer[..count]) },
        Some(Signer::from(list.as_slice())),
    )
}

/// Full read — logs, balances, messages, signatures, account data — minus authority: a reader
/// must never be able to rewrite the member list through the ACL program.
const MEMBER_READ: u8 = 0xFF & !MemberFlags::AUTHORITY;

/// A PRIVATE ephemeral (ER-only) permission on `permissioned`, naming `members` as readers. The
/// ACL program seats the owning program itself. `payer` (a PDA) fronts the rent; it and
/// `permissioned` sign by their seeds.
#[allow(clippy::too_many_arguments)]
pub fn create_ephemeral_permission(
    payer: &AccountInfo,
    permissioned: &AccountInfo,
    permission: &AccountInfo,
    vault: &AccountInfo,
    magic_program: &AccountInfo,
    permission_program: &AccountInfo,
    members: &[Pubkey],
    signers_seeds: &[&[&[u8]]],
) -> ProgramResult {
    if permission_program.address() != &PERMISSION_PROGRAM_ID {
        return Err(ProgramError::IncorrectProgramId);
    }
    let mut buffer = empty_members();
    let count = members_with(MEMBER_READ, members, &mut buffer)?;
    let lists: Vec<Vec<Seed>> = signers_seeds.iter().map(|list| seeds(list)).collect();
    let signers: Vec<Signer> = lists.iter().map(|list| Signer::from(list.as_slice())).collect();
    erp::acl::CreateEphemeralPermission {
        permissioned_account: permissioned,
        permission,
        payer,
        vault,
        magic_program,
        permission_program,
        args: EphemeralMembersArgs { is_private: true, members: &buffer[..count] },
    }
    .invoke_signed::<PERMISSION_DATA>(&signers)
}

