use crate::chain::*;
use ephemeral_rollups_pinocchio::vrf::{scoped_vrf_identity, RequestRandomness, RequestRandomnessCpi};
use pinocchio::cpi::{Seed, Signer};
use pinocchio::instruction::InstructionAccount;

#[derive(Clone)]
pub struct SerializableAccountMeta {
    pub pubkey: Pubkey,
    pub is_signer: bool,
    pub is_writable: bool,
}

/// The identity that signs this program's callbacks: `["identity", program]` at the VRF program.
pub fn callback_identity(program_id: &Pubkey) -> Pubkey {
    scoped_vrf_identity(program_id).0
}

/// Asks the VRF program for randomness, scoped to this program. The oracle later calls back
/// with `callback_discriminator` ++ 32 bytes of randomness ++ `callback_args`, signed by
/// [`callback_identity`].
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
    payer: &AccountInfo,
    identity: &AccountInfo,
    identity_bump: u8,
    oracle_queue: &AccountInfo,
    system_program: &AccountInfo,
    slot_hashes: &AccountInfo,
    vrf_program: &AccountInfo,
    caller_seed: [u8; 32],
    callback_discriminator: [u8; 8],
    callback_accounts: Vec<SerializableAccountMeta>,
    // callback_args is echoed back verbatim in the callback, after the randomness. The reveal
    // path uses it to carry the round, so a stale callback from an earlier round cannot land as
    // a later one's seed — the disclosure trap of every machine with a decision between rounds.
    callback_args: Vec<u8>,
    payer_seeds: &[&[u8]],
    high_priority: bool,
) -> ProgramResult {
    let metas: Vec<InstructionAccount> = callback_accounts
        .iter()
        .map(|meta| InstructionAccount::new(&meta.pubkey, meta.is_writable, meta.is_signer))
        .collect();
    let request = RequestRandomness {
        high_priority,
        caller_seed,
        callback_program_id: program_id,
        callback_discriminator: &callback_discriminator,
        callback_accounts_metas: &metas,
        callback_args: &callback_args,
    };
    let cpi = RequestRandomnessCpi {
        payer,
        program_identity: identity,
        oracle_queue,
        system_program,
        slot_hashes,
        vrf_program,
        request,
    };
    let mut data = vec![0u8; cpi.serialized_size()];
    let identity_bump = [identity_bump];
    let identity_seeds = [Seed::from(b"identity".as_slice()), Seed::from(identity_bump.as_slice())];
    let payer_seeds: Vec<Seed> = payer_seeds.iter().map(|seed| Seed::from(*seed)).collect();
    cpi.invoke_signed(&mut data, &[Signer::from(identity_seeds.as_slice()), Signer::from(payer_seeds.as_slice())])
}
