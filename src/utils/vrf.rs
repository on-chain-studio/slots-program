use borsh::BorshSerialize;
use solana_program::{
    account_info::AccountInfo,
    entrypoint::ProgramResult,
    instruction::{AccountMeta, Instruction},
    program::invoke_signed,
    pubkey::Pubkey,
};

use crate::constants::VRF_PROGRAM;

/// Wire format of the MagicBlock VRF `RequestRandomness` instruction
/// (mirrors ephemeral-vrf-sdk, hand-rolled to stay off its dependency tree).
#[derive(BorshSerialize)]
struct RequestRandomness {
    caller_seed: [u8; 32],
    callback_program_id: Pubkey,
    callback_discriminator: Vec<u8>,
    callback_accounts_metas: Vec<SerializableAccountMeta>,
    callback_args: Vec<u8>,
}

#[derive(BorshSerialize, Clone)]
pub struct SerializableAccountMeta {
    pub pubkey: Pubkey,
    pub is_signer: bool,
    pub is_writable: bool,
}

/// CPIs into the VRF program. The oracle later calls back into this program with
/// `callback_discriminator` ++ 32 bytes of randomness, signed by the VRF identity.
///
/// Accounts: payer (signer, **writable**), our ["identity"] PDA (signs via seeds),
/// oracle queue (writable), system program, slot hashes sysvar.
///
/// The payer being writable is why it cannot be the player's wallet: inside the rollup a
/// wallet is an undelegated basenet account and can sign but never be written. `payer_seeds`
/// lets a program PDA — the house — sign for itself and carry the cost instead.
#[allow(clippy::too_many_arguments)]
pub fn request_randomness<'a>(
    program_id: &Pubkey,
    payer: &AccountInfo<'a>,
    identity: &AccountInfo<'a>,
    identity_bump: u8,
    oracle_queue: &AccountInfo<'a>,
    system_program: &AccountInfo<'a>,
    slot_hashes: &AccountInfo<'a>,
    vrf_program: &AccountInfo<'a>,
    caller_seed: [u8; 32],
    callback_discriminator: [u8; 8],
    callback_accounts: Vec<SerializableAccountMeta>,
    // callback_args is echoed back verbatim in the callback, after the randomness. The reveal
    // path uses it to carry the round, so a stale callback from an earlier round cannot land as
    // a later one's seed — the disclosure trap of every machine with a decision between rounds.
    callback_args: Vec<u8>,
    payer_seeds: &[&[u8]],
    ephemeral: bool,
) -> ProgramResult {
    let payload = RequestRandomness {
        caller_seed,
        callback_program_id: *program_id,
        callback_discriminator: callback_discriminator.to_vec(),
        callback_accounts_metas: callback_accounts,
        callback_args,
    };
    // 8-byte VRF instruction discriminator: 3 = ephemeral queue, 8 = regular queue
    let mut data = vec![if ephemeral { 3u8 } else { 8u8 }, 0, 0, 0, 0, 0, 0, 0];
    payload.serialize(&mut data).map_err(|_| solana_program::program_error::ProgramError::InvalidInstructionData)?;

    invoke_signed(
        &Instruction {
            program_id: VRF_PROGRAM,
            accounts: vec![
                AccountMeta::new(*payer.key, true),
                AccountMeta::new_readonly(*identity.key, true),
                AccountMeta::new(*oracle_queue.key, false),
                AccountMeta::new_readonly(*system_program.key, false),
                AccountMeta::new_readonly(*slot_hashes.key, false),
            ],
            data,
        },
        &[
            payer.clone(),
            identity.clone(),
            oracle_queue.clone(),
            system_program.clone(),
            slot_hashes.clone(),
            vrf_program.clone(),
        ],
        &[&[b"identity", &[identity_bump]], payer_seeds],
    )
}
