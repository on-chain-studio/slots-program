pub mod constants;
pub mod entrypoint;
pub mod error;
pub mod instruction;

pub mod instructions {
    pub mod unknown;
    pub mod initialize;
    pub mod set_machine;
    pub mod grow_config;
    pub mod close_spin;
    pub mod delegation;
    pub mod open_ledger;
    pub mod delegate_treasury;
    pub mod undelegate_treasury;
    pub mod close_ledger;
    pub mod authorize_treasury;
    pub mod set_privacy;
    pub mod withdraw_house;

    pub mod request_bet;
    pub mod resolve_bet;
    pub mod request_reveal;
    pub mod callback_reveal;
    pub mod hold;
    pub mod gamble;
    pub mod request_collect;
    pub mod resolve_collect;
}

pub mod state;

pub mod utils {
    pub mod pda;
    pub mod engine;
    pub mod vrf;
    pub mod vault;
    pub mod receipt;
}
