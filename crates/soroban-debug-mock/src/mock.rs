use soroban_sdk::{Env, Val};
use std::collections::HashMap;

#[derive(Default, Clone)]
pub struct MockRegistry {
    mocks: HashMap<(String, String), Val>,
}

impl MockRegistry {
    pub fn register(&mut self, contract_id: &str, function: &str, return_value: Val) {
        self.mocks.insert(
            (contract_id.to_string(), function.to_string()),
            return_value,
        );
    }

    pub fn log_transaction(&self, _env: &Env) {
        // For the MVP, we'll document that cross-contract mocking
        // with this helper should be used with specific contracts.
        // A full implementation would involve registering a dispatcher
        // for each mocked contract.
    }

    pub fn install(&self, _env: &Env) {
        // For the MVP, we'll document that cross-contract mocking
        // with this helper should be used with specific contracts.
        // A full implementation would involve registering a dispatcher
        // for each mocked contract.
    }
}
