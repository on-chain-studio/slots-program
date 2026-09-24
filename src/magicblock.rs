//! The MagicBlock calls this program makes — delegation, the undelegation callback, ephemeral
//! accounts, commit-and-undelegate, and TEE permissions — written out as the bytes
//! `ephemeral-rollups-sdk` 0.14.4 sends, step for step. That SDK is solana-program only; its
//! Pinocchio sibling is a later protocol version, and a call that moved would be a different call.

use crate::chain::*;

pub const MAGIC_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("Magic11111111111111111111111111111111111111");
pub const EPHEMERAL_VAULT_ID: Pubkey =
    Pubkey::from_str_const("MagicVau1t999999999999999999999999999999999");
pub const DELEGATION_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("DELeGGvXpWV2fqJUhqcF5ZSYMS4JTLjteaAMARRSaeSh");
pub const PERMISSION_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("ACLseoPoyC3cBqoUtkbjZ4aDrkurZW86v19pXz2XQnp1");

/// `MagicBlockInstruction` variants, bincode-indexed.
const CREATE_EPHEMERAL_ACCOUNT: u32 = 12;
const SCHEDULE_INTENT_BUNDLE: u32 = 11;

const DELEGATE_BUFFER_TAG: &[u8] = b"buffer";

// ---------------------------------------------------------------------------------------------
// Rent

/// The rent-exempt minimum for `space` bytes, as `solana-program`'s `Rent::minimum_balance` has
/// it: bytes times the rate, times the exemption threshold. Pinocchio's own `Rent` reads the rate
/// as already including the threshold, which is only so once SIMD-0194 is live; until then it
/// quotes half.
pub fn minimum_balance(space: usize) -> Result<u64, ProgramError> {
    #[repr(C)]
    #[derive(Default)]
    struct Rent {
        lamports_per_byte_year: u64,
        exemption_threshold: f64,
        burn_percent: u8,
    }
    let mut rent = Rent::default();
    #[cfg(target_os = "solana")]
    {
        // The syscall `solana-program`'s `Rent::get` makes, deprecated since in favour of the
        // generic `sol_get_sysvar`; the same bytes either way.
        #[allow(deprecated)]
        let result =
            unsafe { pinocchio::syscalls::sol_get_rent_sysvar(&mut rent as *mut Rent as *mut u8) };
        if result != 0 {
            return Err(ProgramError::UnsupportedSysvar);
        }
    }
    #[cfg(not(target_os = "solana"))]
    {
        rent.lamports_per_byte_year = 3480;
        rent.exemption_threshold = 2.0;
    }
    let _ = rent.burn_percent;
    Ok((((128 + space as u64) * rent.lamports_per_byte_year) as f64 * rent.exemption_threshold) as u64)
}

// ---------------------------------------------------------------------------------------------
// System program

fn allocate(account: &Pubkey, space: u64) -> Instruction {
    let mut data = Vec::with_capacity(12);
    data.extend_from_slice(&8u32.to_le_bytes());
    data.extend_from_slice(&space.to_le_bytes());
    Instruction { program_id: SYSTEM_PROGRAM, accounts: vec![AccountMeta::new(*account, true)], data }
}

fn assign(account: &Pubkey, owner: &Pubkey) -> Instruction {
    let mut data = Vec::with_capacity(36);
    data.extend_from_slice(&1u32.to_le_bytes());
    data.extend_from_slice(owner.as_ref());
    Instruction { program_id: SYSTEM_PROGRAM, accounts: vec![AccountMeta::new(*account, true)], data }
}

fn with_bump<'a>(seeds: &[&'a [u8]], bump: &'a [u8]) -> Vec<&'a [u8]> {
    let mut all = seeds.to_vec();
    all.push(bump);
    all
}

/// The SDK's `create_pda`: a fresh account is created outright; one that already holds lamports
/// is topped up (when it must be rent exempt), allocated and assigned.
fn create_pda(
    target: &AccountInfo,
    owner: &Pubkey,
    space: usize,
    seeds: &[&[&[u8]]],
    system_program: &AccountInfo,
    payer: &AccountInfo,
    rent_exempt: bool,
) -> ProgramResult {
    let minimum = minimum_balance(space)?;
    if target.lamports() == 0 {
        let lamports = if rent_exempt { minimum } else { 0 };
        invoke_signed(
            &system_instruction::create_account(
                payer.address(), target.address(), lamports, space as u64, owner,
            ),
            &[*payer, *target, *system_program],
            seeds,
        )?;
    } else {
        if rent_exempt {
            let shortfall = minimum.saturating_sub(target.lamports());
            if shortfall > 0 {
                invoke(
                    &system_instruction::transfer(payer.address(), target.address(), shortfall),
                    &[*payer, *target, *system_program],
                )?;
            }
        }
        invoke_signed(&allocate(target.address(), space as u64), &[*target, *system_program], seeds)?;
        invoke_signed(&assign(target.address(), owner), &[*target, *system_program], seeds)?;
    }
    Ok(())
}

fn close_pda_with_system_transfer(
    target: &AccountInfo,
    seeds: &[&[&[u8]]],
    destination: &AccountInfo,
    system_program: &AccountInfo,
) -> ProgramResult {
    target.resize(0)?;
    let mut view = *target;
    unsafe { view.assign(&SYSTEM_PROGRAM) };
    if target.lamports() > 0 {
        invoke_signed(
            &system_instruction::transfer(target.address(), destination.address(), target.lamports()),
            &[*target, *destination, *system_program],
            seeds,
        )?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Delegation

/// Hands `pda` to the delegation program: its data is parked in a buffer, the account emptied
/// and given away, the delegation recorded, and the buffer closed back to the payer.
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
    let buffer_seeds: &[&[u8]] = &[DELEGATE_BUFFER_TAG, pda.address().as_ref()];
    let (_, pda_bump) = Pubkey::find_program_address(pda_seeds, owner_program.address());
    let (_, buffer_bump) = Pubkey::find_program_address(buffer_seeds, owner_program.address());
    let pda_bump = [pda_bump];
    let buffer_bump = [buffer_bump];
    let pda_signer = with_bump(pda_seeds, &pda_bump);
    let buffer_signer = with_bump(buffer_seeds, &buffer_bump);
    let pda_signer: &[&[&[u8]]] = &[&pda_signer];
    let buffer_signer: &[&[&[u8]]] = &[&buffer_signer];

    let data_len = pda.data_len();
    create_pda(buffer, owner_program.address(), data_len, buffer_signer, system_program, payer, false)?;
    {
        let from = pda.try_borrow()?;
        let mut to = buffer.try_borrow_mut_data()?;
        to.copy_from_slice(&from);
    }
    pda.try_borrow_mut_data()?.fill(0);

    if delegation_program.address() != &DELEGATION_PROGRAM_ID || system_program.address() != &SYSTEM_PROGRAM {
        return Err(ProgramError::IncorrectProgramId);
    }
    if pda.owner() != &SYSTEM_PROGRAM {
        let mut view = *pda;
        unsafe { view.assign(&SYSTEM_PROGRAM) };
    }
    if pda.owner() != &DELEGATION_PROGRAM_ID {
        invoke_signed(
            &assign(pda.address(), &DELEGATION_PROGRAM_ID),
            &[*pda, *system_program],
            pda_signer,
        )?;
    }

    let mut data = vec![0u8; 8];
    data.extend_from_slice(&commit_frequency_ms.to_le_bytes());
    data.extend_from_slice(&(pda_seeds.len() as u32).to_le_bytes());
    for seed in pda_seeds {
        data.extend_from_slice(&(seed.len() as u32).to_le_bytes());
        data.extend_from_slice(seed);
    }
    match validator {
        Some(validator) => {
            data.push(1);
            data.extend_from_slice(validator.as_ref());
        }
        None => data.push(0),
    }
    invoke_signed(
        &Instruction {
            program_id: DELEGATION_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new(*payer.address(), true),
                AccountMeta::new(*pda.address(), true),
                AccountMeta::new_readonly(*owner_program.address(), false),
                AccountMeta::new(*buffer.address(), false),
                AccountMeta::new(*delegation_record.address(), false),
                AccountMeta::new(*delegation_metadata.address(), false),
                AccountMeta::new_readonly(*system_program.address(), false),
            ],
            data,
        },
        &[*payer, *pda, *owner_program, *buffer, *delegation_record, *delegation_metadata, *system_program],
        pda_signer,
    )?;

    close_pda_with_system_transfer(buffer, buffer_signer, payer, system_program)
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
    if !buffer.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if *buffer.owner() != DELEGATION_PROGRAM_ID {
        return Err(ProgramError::InvalidAccountOwner);
    }
    let seeds: Vec<&[u8]> = account_seeds.iter().map(|seed| seed.as_slice()).collect();
    let (_, bump) = Pubkey::find_program_address(&seeds, owner_program);
    let bump = [bump];
    let signer = with_bump(&seeds, &bump);
    create_pda(delegated_account, owner_program, buffer.data_len(), &[&signer], system_program, payer, true)?;

    let mut data = delegated_account.try_borrow_mut_data()?;
    let buffer_data = buffer.try_borrow()?;
    data.copy_from_slice(&buffer_data);
    Ok(())
}

/// Schedules `accounts` to be committed and undelegated: one `ScheduleIntentBundle`, no actions.
pub fn commit_and_undelegate(
    payer: &AccountInfo,
    magic_context: &AccountInfo,
    magic_program: &AccountInfo,
    magic_fee_vault: Option<&AccountInfo>,
    committed: &[AccountInfo],
) -> ProgramResult {
    let mut all: Vec<AccountInfo> = vec![*payer, *magic_context];
    all.extend(magic_fee_vault.copied());
    all.extend_from_slice(committed);
    // Each account once, where it first appears; the intent names them by that position.
    let mut unique: Vec<AccountInfo> = Vec::with_capacity(all.len());
    for account in all {
        if !unique.iter().any(|seen| seen.address() == account.address()) {
            unique.push(account);
        }
    }
    let index = |key: &Pubkey| unique.iter().position(|a| a.address() == key).unwrap() as u8;
    let indices: Vec<u8> = committed.iter().map(|a| index(a.address())).collect();

    let mut data = Vec::with_capacity(32);
    data.extend_from_slice(&SCHEDULE_INTENT_BUNDLE.to_le_bytes());
    data.push(0); // commit: None
    data.push(1); // commit_and_undelegate: Some
    data.extend_from_slice(&0u32.to_le_bytes()); // CommitTypeArgs::Standalone
    data.extend_from_slice(&(indices.len() as u64).to_le_bytes());
    data.extend_from_slice(&indices);
    data.extend_from_slice(&0u32.to_le_bytes()); // UndelegateTypeArgs::Standalone
    data.push(0); // commit_finalize: None
    data.push(0); // commit_finalize_and_undelegate: None
    data.extend_from_slice(&0u64.to_le_bytes()); // standalone_actions: []

    let metas = unique
        .iter()
        .map(|account| {
            let key = account.address();
            if key == payer.address() {
                AccountMeta::new(*key, true)
            } else if key == magic_context.address()
                || Some(key) == magic_fee_vault.map(|vault| vault.address())
            {
                AccountMeta::new(*key, false)
            } else {
                AccountMeta { pubkey: *key, is_signer: account.is_signer(), is_writable: account.is_writable() }
            }
        })
        .collect();
    // The Magic program and no other: the caller's account is only the runtime's handle to it.
    if magic_program.address() != &MAGIC_PROGRAM_ID {
        return Err(ProgramError::IncorrectProgramId);
    }
    invoke(&Instruction { program_id: MAGIC_PROGRAM_ID, accounts: metas, data }, &unique)
}

// ---------------------------------------------------------------------------------------------
// Ephemeral accounts

/// Creates `ephemeral` inside the rollup, `sponsor` paying its rent from `vault`. Both sign.
pub fn create_ephemeral_account(
    sponsor: &AccountInfo,
    ephemeral: &AccountInfo,
    vault: &AccountInfo,
    data_len: u32,
    signers_seeds: &[&[&[u8]]],
) -> ProgramResult {
    let mut data = Vec::with_capacity(8);
    data.extend_from_slice(&CREATE_EPHEMERAL_ACCOUNT.to_le_bytes());
    data.extend_from_slice(&data_len.to_le_bytes());
    invoke_signed(
        &Instruction {
            program_id: MAGIC_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new(*sponsor.address(), true),
                AccountMeta::new(*ephemeral.address(), true),
                AccountMeta::new(*vault.address(), false),
            ],
            data,
        },
        &[*sponsor, *ephemeral, *vault],
        signers_seeds,
    )
}

// ---------------------------------------------------------------------------------------------
// Permissions

fn members_arg(data: &mut Vec<u8>, members: &[Pubkey]) {
    data.push(1); // Some
    data.extend_from_slice(&(members.len() as u32).to_le_bytes());
    for member in members {
        data.push(0); // flags
        data.extend_from_slice(member.as_ref());
    }
}

/// A TEE permission on `permissioned`, naming `members`. The permissioned account signs.
pub fn create_permission(
    permissioned: &AccountInfo,
    permission: &AccountInfo,
    payer: &AccountInfo,
    system_program: &AccountInfo,
    members: &[Pubkey],
    signers_seeds: &[&[&[u8]]],
) -> ProgramResult {
    let mut data = 0u64.to_le_bytes().to_vec();
    members_arg(&mut data, members);
    invoke_signed(
        &Instruction {
            program_id: PERMISSION_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new_readonly(*permissioned.address(), true),
                AccountMeta::new(*permission.address(), false),
                AccountMeta::new(*payer.address(), true),
                AccountMeta::new_readonly(*system_program.address(), false),
            ],
            data,
        },
        &[*permissioned, *permission, *payer, *system_program],
        signers_seeds,
    )
}

/// Replaces the members of an existing permission.
pub fn update_permission(
    authority: (&AccountInfo, bool),
    permissioned: (&AccountInfo, bool),
    permission: &AccountInfo,
    members: &[Pubkey],
    signers_seeds: &[&[&[u8]]],
) -> ProgramResult {
    let mut data = 1u64.to_le_bytes().to_vec();
    members_arg(&mut data, members);
    invoke_signed(
        &Instruction {
            program_id: PERMISSION_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new_readonly(*authority.0.address(), authority.1),
                AccountMeta::new_readonly(*permissioned.0.address(), permissioned.1),
                AccountMeta::new(*permission.address(), false),
            ],
            data,
        },
        &[*authority.0, *permissioned.0, *permission],
        signers_seeds,
    )
}

/// The ACL discriminator + member flags, from `ephemeral-rollups-sdk` v0.14
/// (`access_control::instructions::create_ephemeral_permission`, `structs::member`).
const CREATE_EPHEMERAL_PERMISSION: u64 = 6;
const AUTHORITY_FLAG: u8 = 1 << 0;
/// Full read (logs, balances, messages, signatures, account data) minus authority — a reader must
/// never be able to rewrite the member list through the ACL program, only the permissioned PDA can.
const MEMBER_READ: u8 = 0xFF & !AUTHORITY_FLAG;

const UPDATE_EPHEMERAL_PERMISSION: u64 = 7;

/// Discriminator, then `is_private`, then each member as `flags:u8 ++ pubkey:[u8;32]` — the SDK's
/// `EphemeralMembersArgs`, shared by create and update.
fn private_members_data(discriminator: u64, members: &[Pubkey]) -> Vec<u8> {
    let mut data = discriminator.to_le_bytes().to_vec();
    data.push(1); // is_private = true
    for member in members {
        data.push(MEMBER_READ);
        data.extend_from_slice(member.as_ref());
    }
    data
}

/// A PRIVATE **ephemeral** (ER-only) permission on `permissioned`, naming `members`. ER-only:
/// `payer` (a PDA) fronts the ephemeral rent — no basenet account, nothing to delegate or commit.
/// `permissioned` and `payer` both sign via their seeds. Account order and data layout are the
/// SDK's `CreateEphemeralPermission` (disc 6).
pub fn create_ephemeral_permission(
    payer: &AccountInfo,
    permissioned: &AccountInfo,
    permission: &AccountInfo,
    vault: &AccountInfo,
    magic_program: &AccountInfo,
    members: &[Pubkey],
    signers_seeds: &[&[&[u8]]],
) -> ProgramResult {
    let data = private_members_data(CREATE_EPHEMERAL_PERMISSION, members);
    invoke_signed(
        &Instruction {
            program_id: PERMISSION_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new(*payer.address(), true),
                AccountMeta::new_readonly(*permissioned.address(), true),
                AccountMeta::new(*permission.address(), false),
                AccountMeta::new(*vault.address(), false),
                AccountMeta::new_readonly(*magic_program.address(), false),
            ],
            data,
        },
        &[*payer, *permissioned, *permission, *vault, *magic_program],
        signers_seeds,
    )
}

/// Replaces the members of a PRIVATE ephemeral permission with `members`. The permissioned account
/// authorises by signing (its seeds): its owning program is the permission's default authority.
/// Account order is the SDK's `UpdateEphemeralPermission` (disc 7) with `authority_is_signer` false.
pub fn update_ephemeral_permission(
    payer: &AccountInfo,
    permissioned: &AccountInfo,
    permission: &AccountInfo,
    vault: &AccountInfo,
    magic_program: &AccountInfo,
    members: &[Pubkey],
    signers_seeds: &[&[&[u8]]],
) -> ProgramResult {
    let data = private_members_data(UPDATE_EPHEMERAL_PERMISSION, members);
    invoke_signed(
        &Instruction {
            program_id: PERMISSION_PROGRAM_ID,
            accounts: vec![
                AccountMeta::new(*payer.address(), true),
                AccountMeta::new_readonly(*permissioned.address(), false), // authority
                AccountMeta::new_readonly(*permissioned.address(), true),  // permissioned account
                AccountMeta::new(*permission.address(), false),
                AccountMeta::new(*vault.address(), false),
                AccountMeta::new_readonly(*magic_program.address(), false),
            ],
            data,
        },
        &[*payer, *permissioned, *permissioned, *permission, *vault, *magic_program],
        signers_seeds,
    )
}

/// Whether a private ephemeral permission names exactly `members`, in order, with read flags.
/// The ACL program seats the owning program first by default, so ours follow it. Layout:
/// discriminator, bump, permissioned (32), private, then 33-byte `flags ++ pubkey` members.
pub fn ephemeral_permission_matches(
    permission: &AccountInfo,
    members: &[Pubkey],
) -> Result<bool, ProgramError> {
    let data = permission.try_borrow()?;
    if data.len() != 35 + 33 * (1 + members.len()) || data[34] != 1 {
        return Ok(false);
    }
    Ok(members.iter().enumerate().all(|(i, member)| {
        let at = 35 + 33 * (i + 1);
        data[at] == MEMBER_READ && &data[at + 1..at + 33] == member.as_ref()
    }))
}
