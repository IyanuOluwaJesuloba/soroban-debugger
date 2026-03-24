#![no_std]

use soroban_sdk::{contract, contracterror, contractimpl, contracttype, Address, Env};

//! WARNING: INSECURE EXAMPLE CONTRACT (TUTORIAL ONLY)
//!
//! This contract intentionally includes vulnerable `*_buggy` entrypoints to demonstrate how
//! authorization mistakes show up in the debugger and traces.
//!
//! Do NOT deploy this contract and do NOT copy/paste `*_buggy` functions into real contracts.
//! Use the secure counterparts (`withdraw`, `admin_mint`) as the baseline patterns.

#[derive(Clone)]
#[contracttype]
pub enum DataKey {
    Admin,
    Balance(Address),
}

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum WalletError {
    Unauthorized = 1,
    InsufficientBalance = 2,
}

#[contract]
pub struct Wallet;

impl Wallet {
    fn read_admin(env: &Env) -> Result<Address, WalletError> {
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .ok_or(WalletError::Unauthorized)
    }
}

#[contractimpl]
impl Wallet {
    /// Initialize the wallet with an admin.
    pub fn initialize(env: Env, admin: Address) {
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
    }

    /// Deposit funds (anyone can deposit to anyone).
    pub fn deposit(env: Env, to: Address, amount: i128) {
        let current = Self::get_balance(env.clone(), to.clone());
        env.storage()
            .persistent()
            .set(&DataKey::Balance(to), &(current + amount));
    }

    /// WARNING: INSECURE (`*_buggy`) TUTORIAL FUNCTION — DO NOT COPY/DEPLOY.
    ///
    /// Intentionally omits `from.require_auth()` so the debugger can surface a missing-auth bug.
    /// Use `withdraw` instead.
    pub fn withdraw_buggy(env: Env, from: Address, amount: i128) -> Result<(), WalletError> {
        // BUG: Missing from.require_auth()!
        let balance = Self::get_balance(env.clone(), from.clone());
        if balance < amount {
            return Err(WalletError::InsufficientBalance);
        }

        env.storage()
            .persistent()
            .set(&DataKey::Balance(from), &(balance - amount));

        Ok(())
    }

    /// FIXED: Withdraw with proper authorization.
    pub fn withdraw(env: Env, from: Address, amount: i128) -> Result<(), WalletError> {
        // FIXED: Require authorization from the withdrawing address
        from.require_auth();

        let balance = Self::get_balance(env.clone(), from.clone());
        if balance < amount {
            return Err(WalletError::InsufficientBalance);
        }

        env.storage()
            .persistent()
            .set(&DataKey::Balance(from), &(balance - amount));

        Ok(())
    }

    /// WARNING: INSECURE (`*_buggy`) TUTORIAL FUNCTION — DO NOT COPY/DEPLOY.
    ///
    /// Intentionally fetches the admin from storage but omits `admin.require_auth()`.
    /// Use `admin_mint` instead.
    pub fn admin_mint_buggy(env: Env, to: Address, amount: i128) -> Result<(), WalletError> {
        // BUG: We check if admin exists but don't verify the caller is the admin!
        let _admin: Address = Self::read_admin(&env)?;

        // Missing: admin.require_auth()

        let balance = Self::get_balance(env.clone(), to.clone());
        env.storage()
            .persistent()
            .set(&DataKey::Balance(to), &(balance + amount));

        Ok(())
    }

    /// FIXED: Admin mint with proper authorization.
    pub fn admin_mint(env: Env, to: Address, amount: i128) -> Result<(), WalletError> {
        let admin: Address = Self::read_admin(&env)?;

        // FIXED: Require admin authorization
        admin.require_auth();

        let balance = Self::get_balance(env.clone(), to.clone());
        env.storage()
            .persistent()
            .set(&DataKey::Balance(to), &(balance + amount));

        Ok(())
    }

    /// Get balance for an address.
    pub fn get_balance(env: Env, account: Address) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::Balance(account))
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::{testutils::Address as _, Env};

    #[test]
    fn test_buggy_withdraw_allows_unauthorized() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register(Wallet, ());
        let client = WalletClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let alice = Address::generate(&env);

        client.initialize(&admin);
        client.deposit(&alice, &1000);

        // This should fail but doesn't because withdraw_buggy is missing auth check
        // In the buggy version, anyone could withdraw from alice's account!
        let result = client.try_withdraw_buggy(&alice, &500);
        assert!(result.is_ok()); // Succeeds even though it shouldn't!
    }

    #[test]
    fn test_fixed_withdraw_requires_auth() {
        let env = Env::default();

        let contract_id = env.register(Wallet, ());
        let client = WalletClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let alice = Address::generate(&env);

        env.mock_all_auths();
        client.initialize(&admin);
        client.deposit(&alice, &1000);

        // The fixed version properly requires authorization
        client.withdraw(&alice, &500);
        assert_eq!(client.get_balance(&alice), 500);
    }

    #[test]
    fn test_buggy_admin_mint_missing_auth() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register(Wallet, ());
        let client = WalletClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let alice = Address::generate(&env);

        client.initialize(&admin);

        // Buggy version doesn't check if caller is admin
        let result = client.try_admin_mint_buggy(&alice, &5000);
        assert!(result.is_ok()); // Succeeds even though caller might not be admin!
    }

    #[test]
    fn test_fixed_admin_mint() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register(Wallet, ());
        let client = WalletClient::new(&env, &contract_id);

        let admin = Address::generate(&env);
        let alice = Address::generate(&env);

        client.initialize(&admin);
        client.admin_mint(&alice, &5000);

        assert_eq!(client.get_balance(&alice), 5000);
    }
}
