//! CPI into the vault program — the only way this game moves value. The game holds no token
//! accounts: it owns the `["house"]` ledger and pays by `settle`. Betting debits the player
//! (their key must consent); paying out credits the player (house signs alone).

use solana_program::{
    account_info::AccountInfo,
    entrypoint::ProgramResult,
    instruction::{AccountMeta, Instruction},
    program::invoke_signed,
    pubkey::Pubkey,
};

use crate::constants::VAULT_PROGRAM;

/// Anchor's `sha256("global:<name>")[..8]`.
const SETTLE_DISC: [u8; 8] = [175, 42, 185, 87, 144, 131, 102, 212];
const DEPOSIT_DISC: [u8; 8] = [242, 35, 198, 137, 82, 225, 242, 182];
const DELEGATE_LEDGER_DISC: [u8; 8] = [159, 3, 197, 64, 7, 12, 101, 66];
const UNDELEGATE_DISC: [u8; 8] = [131, 148, 180, 198, 91, 104, 42, 238];
const WITHDRAW_DISC: [u8; 8] = [183, 18, 70, 156, 148, 109, 161, 34];
const CLOSE_LEDGER_DISC: [u8; 8] = [236, 179, 19, 235, 59, 77, 121, 118];
const OPEN_PDA_LEDGER_DISC: [u8; 8] = [129, 231, 253, 170, 87, 172, 11, 29];
const MAKE_PUBLIC_DISC: [u8; 8] = [41, 76, 102, 98, 184, 102, 132, 29];
const MAKE_PDA_LEDGER_PRIVATE_DISC: [u8; 8] = [200, 46, 191, 221, 30, 12, 10, 96];
const AUTHORIZE_PDA_LEDGER_DISC: [u8; 8] = [103, 17, 112, 146, 88, 193, 202, 156];

/// Borsh `Vec<Vec<u8>>` — how the vault takes a PDA's seeds as proof of ownership.
fn seeds_arg(seeds: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(seeds.len() as u32).to_le_bytes());
    for s in seeds {
        out.extend_from_slice(&(s.len() as u32).to_le_bytes());
        out.extend_from_slice(s);
    }
    out
}

/// Fixed offset of slot-0 SOL in a vault ledger (HEADER 116 + slot-0 mint 32). Couples to the
/// vault's layout: a header change there silently returns a wrong pot rather than failing.
const SOL_AMOUNT_AT: usize = 116 + 32;

/// The SOL a ledger holds, read directly so nothing can drift from it.
pub fn sol_balance(ledger: &AccountInfo) -> Result<u64, solana_program::program_error::ProgramError> {
    let data = ledger.try_borrow_data()?;
    let bytes = data
        .get(SOL_AMOUNT_AT..SOL_AMOUNT_AT + 8)
        .ok_or(solana_program::program_error::ProgramError::InvalidAccountData)?;
    Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
}

/// `["ledger", owner]` in the vault program.
pub fn ledger(owner: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b"ledger", owner.as_ref()], &VAULT_PROGRAM).0
}

/// Moves `amount` of `mint` between two ledgers — bookkeeping only, so it runs in the rollup.
/// `program_side`'s meta must set `is_signer`: a PDA signs only via `invoke_signed`, never on arrival.
#[allow(clippy::too_many_arguments)]
pub fn settle<'a>(
    vault_program: &AccountInfo<'a>,
    src_ledger: &AccountInfo<'a>,
    dst_ledger: &AccountInfo<'a>,
    src_authority: &AccountInfo<'a>,
    dst_authority: &AccountInfo<'a>,
    program_side: &Pubkey,
    signer_seeds: &[&[u8]],
    mint: &Pubkey,
    amount: u64,
) -> ProgramResult {
    let mut data = Vec::with_capacity(48);
    data.extend_from_slice(&SETTLE_DISC);
    data.extend_from_slice(mint.as_ref());
    data.extend_from_slice(&amount.to_le_bytes());

    let ix = Instruction {
        program_id: VAULT_PROGRAM,
        accounts: vec![
            AccountMeta::new(*src_ledger.key, false),
            AccountMeta::new(*dst_ledger.key, false),
            AccountMeta::new_readonly(
                *src_authority.key,
                src_authority.is_signer || src_authority.key == program_side,
            ),
            AccountMeta::new_readonly(
                *dst_authority.key,
                dst_authority.is_signer || dst_authority.key == program_side,
            ),
        ],
        data,
    };

    invoke_signed(
        &ix,
        &[
            vault_program.clone(),
            src_ledger.clone(),
            dst_ledger.clone(),
            src_authority.clone(),
            dst_authority.clone(),
        ],
        &[signer_seeds],
    )
}

/// Deposits into the house ledger (bootstraps it on first use). Same-owner-only, so the house
/// signs for itself. `min_free`/`slot_increase` size it for every mint the game pays out.
#[allow(clippy::too_many_arguments)]
pub fn deposit<'a>(
    vault_program: &AccountInfo<'a>,
    house: &AccountInfo<'a>,
    ledger: &AccountInfo<'a>,
    permission: &AccountInfo<'a>,
    permission_program: &AccountInfo<'a>,
    reserve: &AccountInfo<'a>,
    reserve_token: &AccountInfo<'a>,
    house_token: &AccountInfo<'a>,
    token_program: &AccountInfo<'a>,
    system_program: &AccountInfo<'a>,
    sponsor: &AccountInfo<'a>,
    this_program: &AccountInfo<'a>,
    this_program_data: &AccountInfo<'a>,
    house_seeds: &[&[u8]],
    mint: &Pubkey,
    amount: u64,
    min_free: u16,
    slot_increase: u16,
) -> ProgramResult {
    let mut data = Vec::with_capacity(89);
    data.extend_from_slice(&DEPOSIT_DISC);
    data.extend_from_slice(mint.as_ref());
    data.extend_from_slice(&amount.to_le_bytes());
    data.push(1);
    data.extend_from_slice(&min_free.to_le_bytes());
    data.push(1);
    data.extend_from_slice(&slot_increase.to_le_bytes());

    let ix = Instruction {
        program_id: VAULT_PROGRAM,
        accounts: vec![
            AccountMeta::new(*house.key, true),
            AccountMeta::new(*ledger.key, false),
            AccountMeta::new(*permission.key, false),
            AccountMeta::new_readonly(*permission_program.key, false),
            AccountMeta::new(*reserve.key, false),
            // SOL path: these are System Program placeholders, never writable — mirror the caller's flag.
            AccountMeta {
                pubkey: *reserve_token.key,
                is_signer: false,
                is_writable: reserve_token.is_writable,
            },
            AccountMeta {
                pubkey: *house_token.key,
                is_signer: false,
                is_writable: house_token.is_writable,
            },
            AccountMeta::new_readonly(*token_program.key, false),
            AccountMeta::new_readonly(*system_program.key, false),
            // Funder; the vault requires it to be our upgrade authority.
            AccountMeta::new(*sponsor.key, true),
            AccountMeta::new_readonly(*this_program.key, false),
            AccountMeta::new_readonly(*this_program_data.key, false),
        ],
        data,
    };

    invoke_signed(
        &ix,
        &[
            vault_program.clone(), house.clone(), ledger.clone(), permission.clone(),
            permission_program.clone(), reserve.clone(), reserve_token.clone(),
            house_token.clone(), token_program.clone(), system_program.clone(),
            sponsor.clone(), this_program.clone(), this_program_data.clone(),
        ],
        &[house_seeds],
    )
}

/// Hands a treasury ledger to a rollup validator — only its owner (this program's PDA) can.
#[allow(clippy::too_many_arguments)]
pub fn delegate_ledger<'a>(
    vault_program: &AccountInfo<'a>,
    payer: &AccountInfo<'a>,
    treasury: &AccountInfo<'a>,
    buffer: &AccountInfo<'a>,
    delegation_record: &AccountInfo<'a>,
    delegation_metadata: &AccountInfo<'a>,
    ledger: &AccountInfo<'a>,
    delegation_program: &AccountInfo<'a>,
    system_program: &AccountInfo<'a>,
    treasury_seeds: &[&[u8]],
    validator: &Pubkey,
) -> ProgramResult {
    let mut data = Vec::with_capacity(41);
    data.extend_from_slice(&DELEGATE_LEDGER_DISC);
    data.push(1); // Some(validator)
    data.extend_from_slice(validator.as_ref());

    // The #[delegate] macro inserts buffer/record/metadata *before* the delegated field.
    let ix = Instruction {
        program_id: VAULT_PROGRAM,
        accounts: vec![
            AccountMeta::new(*payer.key, true),
            AccountMeta::new_readonly(*treasury.key, true),
            AccountMeta::new(*buffer.key, false),
            AccountMeta::new(*delegation_record.key, false),
            AccountMeta::new(*delegation_metadata.key, false),
            AccountMeta::new(*ledger.key, false),
            AccountMeta::new_readonly(VAULT_PROGRAM, false),
            AccountMeta::new_readonly(*delegation_program.key, false),
            AccountMeta::new_readonly(*system_program.key, false),
        ],
        data,
    };

    invoke_signed(
        &ix,
        &[
            vault_program.clone(), payer.clone(), treasury.clone(), buffer.clone(),
            delegation_record.clone(), delegation_metadata.clone(), ledger.clone(),
            delegation_program.clone(), system_program.clone(),
        ],
        &[treasury_seeds],
    )
}

/// Ends a treasury ledger's rollup session. Only the ledger's owner (this program's PDA) may.
#[allow(clippy::too_many_arguments)]
pub fn undelegate_ledger<'a>(
    vault_program: &AccountInfo<'a>,
    payer: &AccountInfo<'a>,
    treasury: &AccountInfo<'a>,
    ledger: &AccountInfo<'a>,
    magic_program: &AccountInfo<'a>,
    magic_context: &AccountInfo<'a>,
    fees_vault: &AccountInfo<'a>,
    treasury_seeds: &[&[u8]],
) -> ProgramResult {
    // Admin pays the commit (the only writable non-delegated identity); treasury signs as owner.
    let ix = Instruction {
        program_id: VAULT_PROGRAM,
        accounts: vec![
            AccountMeta::new(*payer.key, true),
            AccountMeta::new_readonly(*treasury.key, true),
            AccountMeta::new(*ledger.key, false),
            AccountMeta::new_readonly(*magic_program.key, false),
            AccountMeta::new(*magic_context.key, false),
            AccountMeta::new(*fees_vault.key, false),
        ],
        data: UNDELEGATE_DISC.to_vec(),
    };

    invoke_signed(
        &ix,
        &[
            vault_program.clone(), payer.clone(), treasury.clone(), ledger.clone(),
            magic_program.clone(), magic_context.clone(), fees_vault.clone(),
        ],
        &[treasury_seeds],
    )
}

/// Takes float back out of the house ledger (mirror of `deposit`). The vault derives the
/// destination from the signer, so value can only land back on the house.
#[allow(clippy::too_many_arguments)]
pub fn withdraw<'a>(
    vault_program: &AccountInfo<'a>,
    house: &AccountInfo<'a>,
    ledger: &AccountInfo<'a>,
    reserve: &AccountInfo<'a>,
    reserve_token: &AccountInfo<'a>,
    house_token: &AccountInfo<'a>,
    token_program: &AccountInfo<'a>,
    system_program: &AccountInfo<'a>,
    receiver: &AccountInfo<'a>,
    this_program: &AccountInfo<'a>,
    this_program_data: &AccountInfo<'a>,
    house_seeds: &[&[u8]],
    mint: &Pubkey,
    amount: u64,
) -> ProgramResult {
    let mut data = Vec::with_capacity(48);
    data.extend_from_slice(&WITHDRAW_DISC);
    data.extend_from_slice(mint.as_ref());
    data.extend_from_slice(&amount.to_le_bytes());

    let ix = Instruction {
        program_id: VAULT_PROGRAM,
        accounts: vec![
            AccountMeta::new(*house.key, true),
            AccountMeta::new(*ledger.key, false),
            AccountMeta::new(*reserve.key, false),
            // placeholders on the SOL path — mirror the caller rather than demand write access
            AccountMeta {
                pubkey: *reserve_token.key,
                is_signer: false,
                is_writable: reserve_token.is_writable,
            },
            AccountMeta {
                pubkey: *house_token.key,
                is_signer: false,
                is_writable: house_token.is_writable,
            },
            AccountMeta::new_readonly(*token_program.key, false),
            AccountMeta::new_readonly(*system_program.key, false),
            // Receiver; must be our upgrade authority. Both sign: the PDA (via seeds) and the authority.
            AccountMeta::new(*receiver.key, true),
            AccountMeta::new_readonly(*this_program.key, false),
            AccountMeta::new_readonly(*this_program_data.key, false),
        ],
        data,
    };

    invoke_signed(
        &ix,
        &[
            vault_program.clone(), receiver.clone(), this_program.clone(),
            this_program_data.clone(),
            house.clone(), ledger.clone(), reserve.clone(),
            reserve_token.clone(), house_token.clone(), token_program.clone(),
            system_program.clone(),
        ],
        &[house_seeds],
    )
}

/// Flips a treasury ledger's privacy: `make_public` deletes its permission, `make_pda_ledger_private`
/// restores it (proving ownership with the treasury seeds).
#[allow(clippy::too_many_arguments)]
pub fn set_privacy<'a>(
    program_id: &Pubkey,
    vault_program: &AccountInfo<'a>,
    payer: &AccountInfo<'a>,
    treasury: &AccountInfo<'a>,
    ledger: &AccountInfo<'a>,
    permission: &AccountInfo<'a>,
    permission_program: &AccountInfo<'a>,
    system_program: &AccountInfo<'a>,
    treasury_seeds: &[&[u8]],
    public: bool,
) -> ProgramResult {
    // The two vault structs order their accounts differently; follow each exactly.
    let (accounts, data) = if public {
        (
            vec![
                AccountMeta::new_readonly(*treasury.key, true),
                AccountMeta::new_readonly(*ledger.key, false),
                AccountMeta::new(*permission.key, false),
                AccountMeta::new(*payer.key, true),
                AccountMeta::new_readonly(*permission_program.key, false),
            ],
            MAKE_PUBLIC_DISC.to_vec(),
        )
    } else {
        let mut data = Vec::with_capacity(64);
        data.extend_from_slice(&MAKE_PDA_LEDGER_PRIVATE_DISC);
        data.extend_from_slice(program_id.as_ref());
        data.extend_from_slice(&seeds_arg(treasury_seeds));
        (
            vec![
                AccountMeta::new_readonly(*treasury.key, true),
                AccountMeta::new(*payer.key, true),
                AccountMeta::new(*ledger.key, false),
                AccountMeta::new(*permission.key, false),
                AccountMeta::new_readonly(*permission_program.key, false),
                AccountMeta::new_readonly(*system_program.key, false),
            ],
            data,
        )
    };
    invoke_signed(
        &Instruction { program_id: VAULT_PROGRAM, accounts, data },
        &[
            vault_program.clone(), treasury.clone(), payer.clone(), ledger.clone(),
            permission.clone(), permission_program.clone(), system_program.clone(),
        ],
        &[treasury_seeds],
    )
}

pub fn open_ledger<'a>(
    program_id: &Pubkey,
    vault_program: &AccountInfo<'a>,
    payer: &AccountInfo<'a>,
    treasury: &AccountInfo<'a>,
    ledger: &AccountInfo<'a>,
    permission: &AccountInfo<'a>,
    permission_program: &AccountInfo<'a>,
    system_program: &AccountInfo<'a>,
    treasury_seeds: &[&[u8]],
    slots: u16,
) -> ProgramResult {
    let mut data = Vec::with_capacity(64);
    data.extend_from_slice(&OPEN_PDA_LEDGER_DISC);
    data.extend_from_slice(&slots.to_le_bytes());
    data.extend_from_slice(program_id.as_ref());
    data.extend_from_slice(&seeds_arg(treasury_seeds));

    invoke_signed(
        &Instruction {
            program_id: VAULT_PROGRAM,
            accounts: vec![
                AccountMeta::new_readonly(*treasury.key, true),
                AccountMeta::new(*payer.key, true),
                AccountMeta::new(*ledger.key, false),
                AccountMeta::new(*permission.key, false),
                AccountMeta::new_readonly(*permission_program.key, false),
                AccountMeta::new_readonly(*system_program.key, false),
            ],
            data,
        },
        &[
            vault_program.clone(), treasury.clone(), payer.clone(), ledger.clone(),
            permission.clone(), permission_program.clone(), system_program.clone(),
        ],
        &[treasury_seeds],
    )
}

/// One-time migration: backfills a ledger's stored member program so `settle_receipt` accepts it.
/// Temporary — removed with the vault's `authorize_pda_ledger` before mainnet.
pub fn authorize_ledger<'a>(
    program_id: &Pubkey,
    vault_program: &AccountInfo<'a>,
    treasury: &AccountInfo<'a>,
    ledger: &AccountInfo<'a>,
    treasury_seeds: &[&[u8]],
) -> ProgramResult {
    let mut data = Vec::with_capacity(48);
    data.extend_from_slice(&AUTHORIZE_PDA_LEDGER_DISC);
    data.extend_from_slice(program_id.as_ref());
    data.extend_from_slice(&seeds_arg(treasury_seeds));

    invoke_signed(
        &Instruction {
            program_id: VAULT_PROGRAM,
            accounts: vec![
                AccountMeta::new_readonly(*treasury.key, true),
                AccountMeta::new(*ledger.key, false),
            ],
            data,
        },
        &[vault_program.clone(), treasury.clone(), ledger.clone()],
        &[treasury_seeds],
    )
}

/// Closes a treasury ledger and its permission, sweeping balances back to the house. `extra` carries
/// each non-zero mint's `(reserve_token, house_token)` pair, in entry order.
pub fn close_ledger<'a>(
    vault_program: &AccountInfo<'a>,
    rent_payer: &AccountInfo<'a>,
    house: &AccountInfo<'a>,
    ledger: &AccountInfo<'a>,
    reserve: &AccountInfo<'a>,
    permission: &AccountInfo<'a>,
    permission_program: &AccountInfo<'a>,
    token_program: &AccountInfo<'a>,
    system_program: &AccountInfo<'a>,
    extra: &[AccountInfo<'a>],
    house_seeds: &[&[u8]],
) -> ProgramResult {
    let mut accounts = vec![
        AccountMeta::new(*house.key, true),
        AccountMeta::new(*rent_payer.key, true),
        AccountMeta::new(*ledger.key, false),
        AccountMeta::new(*reserve.key, false),
        AccountMeta::new(*permission.key, false),
        AccountMeta::new_readonly(*permission_program.key, false),
        AccountMeta::new_readonly(*token_program.key, false),
        AccountMeta::new_readonly(*system_program.key, false),
    ];
    let mut infos = vec![
        vault_program.clone(), house.clone(), rent_payer.clone(), ledger.clone(),
        reserve.clone(), permission.clone(), permission_program.clone(),
        token_program.clone(), system_program.clone(),
    ];
    for a in extra {
        accounts.push(AccountMeta::new(*a.key, false));
        infos.push(a.clone());
    }

    invoke_signed(
        &Instruction { program_id: VAULT_PROGRAM, accounts, data: CLOSE_LEDGER_DISC.to_vec() },
        &infos,
        &[house_seeds],
    )
}
