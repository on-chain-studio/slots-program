use borsh::BorshDeserialize;
use solana_program::{account_info::AccountInfo, program_error::ProgramError, pubkey::Pubkey, entrypoint::ProgramResult};

use crate::instructions::*;

pub trait ProcessInstruction {
    fn process(&self, program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult;
}

const UNDELEGATE_DISC: [u8; 8] = [196, 28, 41, 206, 48, 37, 51, 167];

macro_rules! instructions {
    ($($name:ident($ty:ty)),* $(,)?) => {
        #[derive(BorshDeserialize)]
        pub enum Instruction {
            $($name($ty),)*
        }

        impl ProcessInstruction for Instruction {
            fn process(&self, program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
                match self {
                    $(Self::$name(i) => i.process(program_id, accounts),)*
                }
            }
        }

        /// Each variant's borsh index, which is its position in the list above and therefore
        /// the discriminator a caller sends.
        pub mod ix {
            #![allow(non_upper_case_globals, dead_code)]
            instructions!(@idx 0u64; $($name),*);
        }
    };
    (@idx $i:expr; $head:ident $(, $rest:ident)*) => {
        pub const $head: u64 = $i;
        instructions!(@idx $i + 1; $($rest),*);
    };
    (@idx $i:expr;) => {};
}

// Position is the wire discriminator: dense, append-only, placeholders kept — renumbering would
// silently repoint old clients. The slots below are reserved now so the flow instructions land on
// fixed numbers as they are written, rather than shuffling every client when one arrives.
instructions! {
    Unknown(unknown::Unknown),
    Initialize(initialize::Initialize),
    Delegate(delegation::Delegate),
    Undelegate(delegation::Undelegate),
    RequestUndelegation(delegation::RequestUndelegation),
    SetMachine(set_machine::SetMachine),
    GrowConfig(grow_config::GrowConfig),
    CloseSpin(close_spin::CloseSpin),
    OpenLedger(open_ledger::OpenLedger),
    DelegateTreasury(delegate_treasury::DelegateTreasury),
    UndelegateTreasury(undelegate_treasury::UndelegateTreasury),
    CloseLedger(close_ledger::CloseLedger),
    AuthorizeTreasury(authorize_treasury::AuthorizeTreasury),
    SetPrivacy(set_privacy::SetPrivacy),
    WithdrawHouse(withdraw_house::WithdrawHouse),
    Reserved15(unknown::Unknown),
    RequestBet(request_bet::RequestBet),
    ResolveBet(resolve_bet::ResolveBet),
    RequestReveal(request_reveal::RequestReveal),
    CallbackReveal(callback_reveal::CallbackReveal),
    Hold(hold::Hold),
    Gamble(gamble::Gamble),
    RequestCollect(request_collect::RequestCollect),
    ResolveCollect(resolve_collect::ResolveCollect),
}

pub fn dispatch(program_id: &Pubkey, accounts: &[AccountInfo], input: &[u8]) -> ProgramResult {
    if input.len() < 8 {
        return Err(ProgramError::InvalidInstructionData);
    }
    let variant = if input[..8] == UNDELEGATE_DISC {
        ix::Undelegate
    } else {
        u64::from_le_bytes(input[..8].try_into().unwrap())
    };
    let index = u8::try_from(variant).map_err(|_| ProgramError::InvalidInstructionData)?;

    let mut buf = Vec::with_capacity(1 + input.len() - 8);
    buf.push(index);
    buf.extend_from_slice(&input[8..]);

    // `deserialize`, not `try_from_slice`: a settle callback carries args the handler reads
    // from the receipt instead, so trailing bytes are expected.
    Instruction::deserialize(&mut &buf[..])
        .map_err(|_| ProgramError::InvalidInstructionData)?
        .process(program_id, accounts)
}
