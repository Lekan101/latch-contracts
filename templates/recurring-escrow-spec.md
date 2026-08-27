# Recurring Payment Escrow Spec

A "personal contract" template for recurring subscription payments between two parties on Soroban. The owner pre-funds the contract with a lump sum of a single token; a designated payee can pull a fixed amount on a fixed ledger schedule, up to the funded total, without the owner re-authorizing every payment.

This is the contract version of the "Backend Automation" use case from issue #43 — a pre-funded, capped satellite contract rather than an ongoing policy on the main account.

## Why This Template Exists

Two parties agree on a schedule: `amount_per_period` every `period_ledgers`. The owner deposits the total up front. The payee (or anyone relaying on their behalf) actively calls `pull` on schedule — the contract releases the amount if (a) a full period has elapsed since the last successful pull (or since deployment, for the first pull) and (b) the funded balance covers it.

Soroban has no native cron primitive, so nothing pushes funds on a timer; the contract only ever reacts to an explicit `pull`. This keeps the v1 surface small and audit-friendly, at the cost of the payee (or their automation) having to initiate each payment.

## Contract Interface

```rust
fn __constructor(e: Env, owner: Address, payee: Address,
                 amount_per_period: i128, period_ledgers: u32, token: Address)

fn pull(e: Env, to: Address)

fn cancel(e: Env, to: Address)

fn get_config(e: Env) -> EscrowConfig

fn get_state(e: Env) -> EscrowState

fn get_balance(e: Env) -> i128
```

### `__constructor`

Set-once deployment. Stores `EscrowConfig` (owner, payee, `amount_per_period`, `period_ledgers`, token) and an `EscrowState` recording the deployment ledger. Reverts on zero/negative `amount_per_period` (`InvalidAmount`), zero `period_ledgers` (`InvalidPeriod`), or a second initialization (`AlreadyInitialized`).

**No funds move here.** The owner funds the contract after deployment by transferring `token` directly to the contract's address — deliberately no `deposit()` method, same "no deposit needed" reasoning as the timelock vault (#40).

### `pull`

Releases exactly `amount_per_period` of `token` to `to`, then advances the state's `last_pull_ledger` to the current ledger. Payable to any address, but only callable by the `payee` (`payee.require_auth()`).

**Period gating:** `current_ledger - last_pull_ledger >= period_ledgers`, where `last_pull_ledger` starts at the deployment ledger — so the first pull is available a full period after deployment, and each subsequent pull a full period after the previous success. Pulls are all-or-nothing: if the contract's balance is below `amount_per_period`, it reverts (`InsufficientFunds`) rather than partially releasing.

Errors: `TooEarly`, `InsufficientFunds`, `AlreadyCancelled`.

### `cancel`

Owner-only (`owner.require_auth()`). Returns the remaining `token` balance to `to` and sets the state's `cancelled` flag, permanently disabling further `pull`s. The payee has no competing claim to funds not yet pulled, so the owner may always reclaim the remainder. A second cancel reverts (`AlreadyCancelled`).

Events:

- `payment_released` — topics `["payment_released", payee: Address]`, data `[amount: i128]`
- `escrow_cancelled` — topics `["escrow_cancelled", owner: Address]`, data `[refunded: i128]`

## Types

```rust
type amount_per_period = i128   // must be > 0
type period_ledgers    = u32    // must be > 0
```

## Storage

| Key | Type | Notes |
|---|---|---|
| `Config` | `EscrowConfig` | Immutable, set at construction |
| `State` | `EscrowState` | `last_pull_ledger` (deployment ledger initially), `cancelled` |

Both live in instance storage. `load_config`/`load_state` extend the instance TTL on every read so a long-lived, periodically-pulled escrow (or one sitting idle) does not get garbage-collected.

## Security notes

- `require_auth` gates every state change — `payee` for `pull`, `owner` for `cancel`. Funds can only leave via the two entrypoints, both authenticated.
- Transfer amounts are bounded by the funded balance; `pull` reverts rather than releasing dust or going negative.

## What This Is Not

- Not an automatic/cron payment contract — every release requires an explicit `pull` transaction.
- Single-token, single-payee, fixed amount per period for v1. Escalating amounts and multiple payees are out of scope (separate deployments per payee for now).
- Not dependent on deployment via the smart account's account-authorized deployment entrypoint (#39) — the template stands alone; wiring it into the account registry is a follow-up once #39 lands.

## Verification

- `cargo test` — 21 unit tests covering constructor validation, pull period gating (before the first period, between pulls), insufficient-funds rejection, non-payee/non-owner rejection, exact-drain, full/partial balance cancels, events, and direct top-ups (the "no deposit()" path).
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo +nightly fmt --all -- --check`
- `stellar contract build` — compiles to WASM.