//! Recurring payment escrow — a "personal contract" template.
//!
//! The owner pre-funds the contract with a lump sum of a single token. A
//! designated payee can then `pull` a fixed `amount_per_period` once every
//! `period_ledgers`, without the owner having to re-authorize each payment.
//! The owner may `cancel` at any time to reclaim the remaining balance and
//! permanently stop future pulls.

#![no_std]

use soroban_sdk::{
    contract, contracterror, contractevent, contractimpl, contracttype, panic_with_error,
    token::Client as TokenClient, Address, Env,
};

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EscrowStorageKey {
    Config,
    State,
}

/// Immutable escrow configuration, set once at construction.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EscrowConfig {
    /// The address that funded the escrow and may cancel it.
    pub owner: Address,
    /// The only address authorized to call `pull`.
    pub payee: Address,
    /// Fixed amount released on each successful pull.
    pub amount_per_period: i128,
    /// Minimum number of ledgers that must elapse between successful pulls.
    pub period_ledgers: u32,
    /// The token this escrow pays out.
    pub token: Address,
}

/// Mutable escrow state.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EscrowState {
    /// Ledger sequence of the last successful pull, or the deployment ledger
    /// for the first pull.
    pub last_pull_ledger: u32,
    /// Whether the owner has cancelled the escrow, permanently disabling
    /// further `pull`s.
    pub cancelled: bool,
}

/// Error codes for [`RecurringEscrow`] operations.
///
/// This is a standalone, independently-deployed contract, not a module of the
/// upstream `stellar-accounts` crate, so it is not part of that crate's shared
/// error-numbering convention. Numbering starts fresh at `1`.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum EscrowError {
    /// The contract has already been initialized.
    AlreadyInitialized = 1,
    /// A full period has not elapsed since the last pull (or since deployment,
    /// for the first pull).
    TooEarly = 2,
    /// The escrow's token balance cannot cover `amount_per_period`.
    InsufficientFunds = 3,
    /// The escrow has already been cancelled.
    AlreadyCancelled = 4,
    /// `amount_per_period` was zero or negative.
    InvalidAmount = 5,
    /// `period_ledgers` was zero.
    InvalidPeriod = 6,
}

/// Emitted when a payment is released from the escrow.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaymentReleased {
    #[topic]
    pub payee: Address,
    pub amount: i128,
}

/// Emitted when the owner cancels the escrow and reclaims the remaining
/// balance.
#[contractevent]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EscrowCancelled {
    #[topic]
    pub owner: Address,
    pub refunded: i128,
}

#[contract]
pub struct RecurringEscrow;

#[contractimpl]
impl RecurringEscrow {
    /// Deploys the escrow with its immutable configuration. No funds are
    /// moved here — the owner funds the contract by transferring `token` to
    /// its address after deployment.
    ///
    /// # Arguments
    ///
    /// * `e` - Access to the Soroban environment.
    /// * `owner` - The address that funded the escrow and may cancel it.
    /// * `payee` - The only address authorized to call `pull`.
    /// * `amount_per_period` - Fixed amount released on each successful pull.
    /// * `period_ledgers` - Minimum number of ledgers between successful pulls.
    /// * `token` - The token this escrow pays out.
    ///
    /// # Errors
    ///
    /// * [`EscrowError::InvalidAmount`] - `amount_per_period` was zero or
    ///   negative.
    /// * [`EscrowError::InvalidPeriod`] - `period_ledgers` was zero.
    pub fn __constructor(
        e: Env,
        owner: Address,
        payee: Address,
        amount_per_period: i128,
        period_ledgers: u32,
        token: Address,
    ) {
        if e.storage().instance().has(&EscrowStorageKey::Config) {
            panic_with_error!(&e, EscrowError::AlreadyInitialized);
        }
        if amount_per_period <= 0 {
            panic_with_error!(&e, EscrowError::InvalidAmount);
        }
        if period_ledgers == 0 {
            panic_with_error!(&e, EscrowError::InvalidPeriod);
        }

        let config = EscrowConfig { owner, payee, amount_per_period, period_ledgers, token };
        let state = EscrowState { last_pull_ledger: e.ledger().sequence(), cancelled: false };

        e.storage().instance().set(&EscrowStorageKey::Config, &config);
        e.storage().instance().set(&EscrowStorageKey::State, &state);
        e.storage().instance().extend_ttl(100, 1_555_200);
    }

    /// Releases `amount_per_period` of `token` to `to`. Only the `payee` may
    /// initiate a pull, although the funds may be sent to any address. At
    /// least `period_ledgers` must have elapsed since the last successful
    /// pull — or since deployment, for the first pull. Reverts rather than
    /// partially releasing when the escrow cannot cover the amount.
    ///
    /// # Arguments
    ///
    /// * `e` - Access to the Soroban environment.
    /// * `to` - The address receiving the released payment.
    ///
    /// # Errors
    ///
    /// * [`EscrowError::TooEarly`] - Less than `period_ledgers` ledgers have
    ///   elapsed since the last pull, or since deployment.
    /// * [`EscrowError::InsufficientFunds`] - The escrow's token balance is
    ///   below `amount_per_period`.
    /// * [`EscrowError::AlreadyCancelled`] - The escrow has been cancelled.
    ///
    /// # Events
    ///
    /// * `payment_released` - `[payee: Address]`, `[amount: i128]`
    pub fn pull(e: Env, to: Address) {
        let config = load_config(&e);
        let mut state = load_state(&e);

        config.payee.require_auth();

        if state.cancelled {
            panic_with_error!(&e, EscrowError::AlreadyCancelled);
        }

        let current_ledger = e.ledger().sequence();
        if current_ledger.wrapping_sub(state.last_pull_ledger) < config.period_ledgers {
            panic_with_error!(&e, EscrowError::TooEarly);
        }

        let token_client = TokenClient::new(&e, &config.token);
        let balance = token_client.balance(&e.current_contract_address());
        if balance < config.amount_per_period {
            panic_with_error!(&e, EscrowError::InsufficientFunds);
        }

        token_client.transfer(&e.current_contract_address(), &to, &config.amount_per_period);

        state.last_pull_ledger = current_ledger;
        e.storage().instance().set(&EscrowStorageKey::State, &state);

        emit_payment_released(&e, &config.payee, config.amount_per_period);
    }

    /// Cancels the escrow, returning the remaining `token` balance to `to`
    /// and permanently disabling further `pull`s. Only the `owner` may cancel.
    ///
    /// # Arguments
    ///
    /// * `e` - Access to the Soroban environment.
    /// * `to` - The address receiving the remaining balance.
    ///
    /// # Errors
    ///
    /// * [`EscrowError::AlreadyCancelled`] - The escrow has already been
    ///   cancelled.
    ///
    /// # Events
    ///
    /// * `escrow_cancelled` - `[owner: Address]`, `[refunded: i128]`
    pub fn cancel(e: Env, to: Address) {
        let config = load_config(&e);
        let mut state = load_state(&e);

        config.owner.require_auth();

        if state.cancelled {
            panic_with_error!(&e, EscrowError::AlreadyCancelled);
        }

        let token_client = TokenClient::new(&e, &config.token);
        let balance = token_client.balance(&e.current_contract_address());

        if balance > 0 {
            token_client.transfer(&e.current_contract_address(), &to, &balance);
        }

        state.cancelled = true;
        e.storage().instance().set(&EscrowStorageKey::State, &state);

        emit_escrow_cancelled(&e, &config.owner, balance);
    }

    /// Returns the escrow's immutable configuration.
    pub fn get_config(e: Env) -> EscrowConfig {
        load_config(&e)
    }

    /// Returns the escrow's current mutable state.
    pub fn get_state(e: Env) -> EscrowState {
        load_state(&e)
    }

    /// Returns the contract's current `token` balance.
    pub fn get_balance(e: Env) -> i128 {
        let config = load_config(&e);
        let token_client = TokenClient::new(&e, &config.token);
        token_client.balance(&e.current_contract_address())
    }
}

fn load_config(e: &Env) -> EscrowConfig {
    e.storage().instance().extend_ttl(100, 1_555_200);
    e.storage()
        .instance()
        .get(&EscrowStorageKey::Config)
        .expect("config is always set by the constructor")
}

fn load_state(e: &Env) -> EscrowState {
    e.storage().instance().extend_ttl(100, 1_555_200);
    e.storage()
        .instance()
        .get(&EscrowStorageKey::State)
        .expect("state is always set by the constructor")
}

fn emit_payment_released(e: &Env, payee: &Address, amount: i128) {
    PaymentReleased { payee: payee.clone(), amount }.publish(e);
}

fn emit_escrow_cancelled(e: &Env, owner: &Address, refunded: i128) {
    EscrowCancelled { owner: owner.clone(), refunded }.publish(e);
}

#[cfg(test)]
mod test;
