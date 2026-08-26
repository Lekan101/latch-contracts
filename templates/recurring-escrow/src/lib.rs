#![no_std]

use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, panic_with_error, Address,
    Env,
};

mod test;

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    Config,
    State,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EscrowConfig {
    pub owner: Address,
    pub payee: Address,
    pub amount_per_period: i128,
    pub period_ledgers: u32,
    pub token: Address,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EscrowState {
    pub last_pull_ledger: u32,
    pub cancelled: bool,
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum EscrowError {
    AlreadyInitialized = 1,
    NotOwner = 2,
    NotPayee = 3,
    TooEarly = 4,
    InsufficientFunds = 5,
    AlreadyCancelled = 6,
    NotCancelled = 7,
}

#[contractevent]
pub struct PaymentReleased {
    pub payee: Address,
    pub amount: i128,
}

#[contractevent]
pub struct EscrowCancelled {
    pub owner: Address,
    pub refunded: i128,
}

#[contract]
pub struct RecurringEscrow;

#[contractimpl]
impl RecurringEscrow {
    pub fn __constructor(
        e: Env,
        owner: Address,
        payee: Address,
        amount_per_period: i128,
        period_ledgers: u32,
        token: Address,
    ) {
        if e.storage().instance().has(&DataKey::Config) {
            panic_with_error!(&e, EscrowError::AlreadyInitialized);
        }
        if amount_per_period <= 0 {
            panic_with_error!(&e, EscrowError::InsufficientFunds);
        }
        if period_ledgers == 0 {
            panic_with_error!(&e, EscrowError::TooEarly);
        }

        let config = EscrowConfig { owner, payee, amount_per_period, period_ledgers, token };
        let state = EscrowState { last_pull_ledger: 0, cancelled: false };

        e.storage().instance().set(&DataKey::Config, &config);
        e.storage().instance().set(&DataKey::State, &state);
        e.storage().instance().extend_ttl(100, 1_555_200);
    }

    pub fn pull(e: Env, to: Address) {
        let config: EscrowConfig = e.storage().instance().get(&DataKey::Config).unwrap();
        let mut state: EscrowState = e.storage().instance().get(&DataKey::State).unwrap();

        config.payee.require_auth();

        if state.cancelled {
            panic_with_error!(&e, EscrowError::AlreadyCancelled);
        }

        let current_ledger = e.ledger().sequence();
        let elapsed = if state.last_pull_ledger == 0 {
            current_ledger
        } else {
            current_ledger.wrapping_sub(state.last_pull_ledger)
        };

        if elapsed < config.period_ledgers {
            panic_with_error!(&e, EscrowError::TooEarly);
        }

        let token_client = soroban_sdk::token::Client::new(&e, &config.token);
        let balance = token_client.balance(&e.current_contract_address());
        if balance < config.amount_per_period {
            panic_with_error!(&e, EscrowError::InsufficientFunds);
        }

        token_client.transfer(&e.current_contract_address(), &to, &config.amount_per_period);

        state.last_pull_ledger = current_ledger;
        e.storage().instance().set(&DataKey::State, &state);

        PaymentReleased { payee: config.payee, amount: config.amount_per_period }.publish(&e);
    }

    pub fn cancel(e: Env) {
        let config: EscrowConfig = e.storage().instance().get(&DataKey::Config).unwrap();
        let mut state: EscrowState = e.storage().instance().get(&DataKey::State).unwrap();

        config.owner.require_auth();

        if state.cancelled {
            panic_with_error!(&e, EscrowError::AlreadyCancelled);
        }

        let token_client = soroban_sdk::token::Client::new(&e, &config.token);
        let balance = token_client.balance(&e.current_contract_address());

        if balance > 0 {
            token_client.transfer(&e.current_contract_address(), &config.owner, &balance);
        }

        state.cancelled = true;
        e.storage().instance().set(&DataKey::State, &state);

        EscrowCancelled { owner: config.owner, refunded: balance }.publish(&e);
    }

    pub fn get_config(e: Env) -> EscrowConfig {
        e.storage().instance().get(&DataKey::Config).unwrap()
    }

    pub fn get_state(e: Env) -> EscrowState {
        e.storage().instance().get(&DataKey::State).unwrap()
    }

    pub fn get_balance(e: Env) -> i128 {
        let config: EscrowConfig = e.storage().instance().get(&DataKey::Config).unwrap();
        let token_client = soroban_sdk::token::Client::new(&e, &config.token);
        token_client.balance(&e.current_contract_address())
    }
}
