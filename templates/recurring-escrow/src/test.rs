#![cfg(test)]

extern crate std;

use soroban_sdk::{
    contract, contractimpl, testutils::Address as _, testutils::Events, testutils::Ledger, Address,
    Env, Vec,
};

use super::*;

// ── mock token ──────────────────────────────────────────────────────────────

#[contract]
pub struct MockToken;

#[contracttype]
pub enum MockTokenKey {
    Balance(Address),
}

#[contractimpl]
impl MockToken {
    pub fn __constructor(_e: Env) {}

    pub fn mint(e: Env, to: Address, amount: i128) {
        let key = MockTokenKey::Balance(to);
        let current: i128 = e.storage().instance().get(&key).unwrap_or(0);
        e.storage().instance().set(&key, &(current + amount));
    }

    pub fn transfer(e: Env, from: Address, to: Address, amount: i128) {
        let from_key = MockTokenKey::Balance(from.clone());
        let to_key = MockTokenKey::Balance(to);
        let from_bal: i128 = e.storage().instance().get(&from_key).unwrap_or(0);
        let to_bal: i128 = e.storage().instance().get(&to_key).unwrap_or(0);
        assert!(from_bal >= amount, "insufficient balance");
        e.storage().instance().set(&from_key, &(from_bal - amount));
        e.storage().instance().set(&to_key, &(to_bal + amount));
    }

    pub fn balance(e: Env, id: Address) -> i128 {
        let key = MockTokenKey::Balance(id);
        e.storage().instance().get(&key).unwrap_or(0)
    }
}

// ── helpers ─────────────────────────────────────────────────────────────────

const AMOUNT: i128 = 100;
const PERIOD: u32 = 10;
const INITIAL_FUND: i128 = 500;

fn setup() -> (Env, Address, Address, Address, Address) {
    let e = Env::default();
    let owner = Address::generate(&e);
    let payee = Address::generate(&e);
    let token_addr = e.register(MockToken, ());
    let escrow_addr = e.register(RecurringEscrow, (&owner, &payee, AMOUNT, PERIOD, &token_addr));

    e.mock_all_auths();

    let token = MockTokenClient::new(&e, &token_addr);
    token.mint(&owner, &1000);
    token.transfer(&owner, &escrow_addr, &INITIAL_FUND);

    (e, owner, payee, token_addr, escrow_addr)
}

fn escrow<'a>(e: &'a Env, addr: &Address) -> RecurringEscrowClient<'a> {
    RecurringEscrowClient::new(e, addr)
}

fn token<'a>(e: &'a Env, addr: &Address) -> MockTokenClient<'a> {
    MockTokenClient::new(e, addr)
}

// ── constructor tests ───────────────────────────────────────────────────────

#[test]
fn constructor_stores_config_and_state() {
    let (e, owner, payee, _token_addr, escrow_addr) = setup();
    let client = escrow(&e, &escrow_addr);

    let config = client.get_config();
    assert_eq!(config.owner, owner);
    assert_eq!(config.payee, payee);
    assert_eq!(config.amount_per_period, AMOUNT);
    assert_eq!(config.period_ledgers, PERIOD);

    let state = client.get_state();
    assert_eq!(state.last_pull_ledger, 0);
    assert!(!state.cancelled);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn constructor_rejects_zero_amount() {
    let e = Env::default();
    let owner = Address::generate(&e);
    let payee = Address::generate(&e);
    let token = Address::generate(&e);

    e.register(RecurringEscrow, (&owner, &payee, 0_i128, PERIOD, &token));
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn constructor_rejects_negative_amount() {
    let e = Env::default();
    let owner = Address::generate(&e);
    let payee = Address::generate(&e);
    let token = Address::generate(&e);

    e.register(RecurringEscrow, (&owner, &payee, -5_i128, PERIOD, &token));
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn constructor_rejects_zero_period() {
    let e = Env::default();
    let owner = Address::generate(&e);
    let payee = Address::generate(&e);
    let token = Address::generate(&e);

    e.register(RecurringEscrow, (&owner, &payee, AMOUNT, 0_u32, &token));
}

// ── pull tests ──────────────────────────────────────────────────────────────

#[test]
fn pull_after_period_succeeds() {
    let (e, _owner, payee, _token_addr, escrow_addr) = setup();
    let client = escrow(&e, &escrow_addr);

    e.ledger().set_sequence_number(20);

    client.pull(&payee);

    let state = client.get_state();
    assert_eq!(state.last_pull_ledger, 20);
    assert_eq!(client.get_balance(), INITIAL_FUND - AMOUNT);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn pull_before_period_rejected() {
    let (e, _owner, payee, _token_addr, escrow_addr) = setup();
    let client = escrow(&e, &escrow_addr);

    e.ledger().set_sequence_number(5);
    client.pull(&payee);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")]
fn pull_too_soon_after_last_pull() {
    let (e, _owner, payee, _token_addr, escrow_addr) = setup();
    let client = escrow(&e, &escrow_addr);

    e.ledger().set_sequence_number(10);
    client.pull(&payee);

    e.ledger().set_sequence_number(15);
    client.pull(&payee);
}

#[test]
fn pull_second_pull_after_full_period() {
    let (e, _owner, payee, _token_addr, escrow_addr) = setup();
    let client = escrow(&e, &escrow_addr);

    e.ledger().set_sequence_number(10);
    client.pull(&payee);

    e.ledger().set_sequence_number(20);
    client.pull(&payee);

    assert_eq!(client.get_balance(), INITIAL_FUND - 2 * AMOUNT);
    assert_eq!(client.get_state().last_pull_ledger, 20);
}

#[test]
#[should_panic]
fn pull_from_non_payee_rejected() {
    let e = Env::default();
    let owner = Address::generate(&e);
    let payee = Address::generate(&e);
    let token_addr = e.register(MockToken, ());
    let escrow_addr = e.register(RecurringEscrow, (&owner, &payee, AMOUNT, PERIOD, &token_addr));
    let attacker = Address::generate(&e);

    let tok = MockTokenClient::new(&e, &token_addr);
    tok.mint(&owner, &1000);

    e.as_contract(&owner, || {
        let tok = MockTokenClient::new(&e, &token_addr);
        tok.transfer(&owner, &escrow_addr, &INITIAL_FUND);
    });

    let client = RecurringEscrowClient::new(&e, &escrow_addr);
    e.ledger().set_sequence_number(10);

    // pull requires payee.require_auth(), but attacker is calling without auth
    client.pull(&attacker);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")]
fn pull_insufficient_funds_rejected() {
    let (e, _owner, payee, _token_addr, escrow_addr) = setup();
    let client = escrow(&e, &escrow_addr);

    for seq in [10, 20, 30, 40, 50] {
        e.ledger().set_sequence_number(seq);
        client.pull(&payee);
    }

    assert_eq!(client.get_balance(), 0);

    e.ledger().set_sequence_number(60);
    client.pull(&payee);
}

#[test]
fn pull_all_funds_exact() {
    let (e, _owner, payee, _token_addr, escrow_addr) = setup();
    let client = escrow(&e, &escrow_addr);

    for seq in [10, 20, 30, 40, 50] {
        e.ledger().set_sequence_number(seq);
        client.pull(&payee);
    }

    assert_eq!(client.get_balance(), 0);
    assert_eq!(client.get_state().last_pull_ledger, 50);
}

// ── cancel tests ────────────────────────────────────────────────────────────

#[test]
fn cancel_returns_funds_to_owner() {
    let (e, owner, _payee, token_addr, escrow_addr) = setup();
    let client = escrow(&e, &escrow_addr);
    let tok = token(&e, &token_addr);

    e.ledger().set_sequence_number(10);
    client.pull(&owner);

    client.cancel();

    assert!(client.get_state().cancelled);
    assert_eq!(client.get_balance(), 0);
    // owner started with 1000, sent 500 to escrow, pulled 100 back = 600
    // cancel refunds remaining 400 = 1000 total
    assert_eq!(tok.balance(&owner), 1000);
}

#[test]
fn cancel_with_full_balance() {
    let (e, owner, _payee, token_addr, escrow_addr) = setup();
    let client = escrow(&e, &escrow_addr);
    let tok = token(&e, &token_addr);

    client.cancel();

    assert!(client.get_state().cancelled);
    assert_eq!(client.get_balance(), 0);
    // owner started with 1000, sent 500, cancel refunds 500 = 1000
    assert_eq!(tok.balance(&owner), 1000);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn cancel_double_cancel_rejected() {
    let (e, _owner, _payee, _token_addr, escrow_addr) = setup();
    let client = escrow(&e, &escrow_addr);

    client.cancel();
    client.cancel();
}

#[test]
#[should_panic]
fn cancel_from_non_owner_rejected() {
    let e = Env::default();
    let owner = Address::generate(&e);
    let payee = Address::generate(&e);
    let token_addr = e.register(MockToken, ());
    let escrow_addr = e.register(RecurringEscrow, (&owner, &payee, AMOUNT, PERIOD, &token_addr));
    let attacker = Address::generate(&e);
    let client = escrow(&e, &escrow_addr);

    // Only authorize attacker, not owner — so owner.require_auth() will fail
    e.mock_auths(&[soroban_sdk::testutils::MockAuth {
        address: &attacker,
        invoke: &soroban_sdk::testutils::MockAuthInvoke {
            contract: &escrow_addr,
            fn_name: "cancel",
            args: Vec::new(&e),
            sub_invokes: &[],
        },
    }]);

    client.cancel();
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")]
fn pull_after_cancel_rejected() {
    let (e, _owner, payee, _token_addr, escrow_addr) = setup();
    let client = escrow(&e, &escrow_addr);

    client.cancel();

    e.ledger().set_sequence_number(10);
    client.pull(&payee);
}

// ── event tests ─────────────────────────────────────────────────────────────

#[test]
fn pull_emits_payment_released_event() {
    let (e, _owner, payee, _token_addr, escrow_addr) = setup();
    let client = escrow(&e, &escrow_addr);

    e.ledger().set_sequence_number(10);
    client.pull(&payee);

    let events = e.events().all();
    assert_eq!(events.events().len(), 1);
}

#[test]
fn cancel_emits_escrow_cancelled_event() {
    let (e, _owner, _payee, _token_addr, escrow_addr) = setup();
    let client = escrow(&e, &escrow_addr);

    client.cancel();

    let events = e.events().all();
    assert_eq!(events.events().len(), 1);
}

// ── balance query ───────────────────────────────────────────────────────────

#[test]
fn get_balance_returns_correct_value() {
    let (e, _owner, _payee, _token_addr, escrow_addr) = setup();
    let client = escrow(&e, &escrow_addr);

    assert_eq!(client.get_balance(), INITIAL_FUND);
}
