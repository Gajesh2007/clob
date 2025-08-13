use crate::{AccountCache, PublicKeyCache, ExecutionPlaneError};
use common_types::{UserID, Account, AssetID};
use ed25519_dalek::{SigningKey, VerifyingKey};
use rand::rngs::OsRng;
use rand::RngCore;
use rust_decimal::Decimal;
use std::collections::BTreeMap;

/// Manages the creation and funding of users.
/// In a real system, this would be replaced with a module that interacts
/// with L1 smart contracts for deposits.
pub struct UserManager {
    account_cache: AccountCache,
    public_key_cache: PublicKeyCache,
}

impl UserManager {
    pub fn new(account_cache: AccountCache, public_key_cache: PublicKeyCache) -> Self {
        Self { account_cache, public_key_cache }
    }

    /// Creates a new user, generates a keypair, and initializes an empty account.
    /// Returns the new UserID and the private key bytes for the client to use.
    pub fn create_user(&self) -> (UserID, [u8; 32]) {
        let mut csprng = OsRng;
        let mut secret_bytes = [0u8; 32];
        csprng.fill_bytes(&mut secret_bytes);
        let signing_key = SigningKey::from_bytes(&secret_bytes);
        let verifying_key: VerifyingKey = (&signing_key).into();

        // In a real system, UserID would be derived from the public key or be a counter.
        // For simplicity, we'll use a random u64.
        let user_id = UserID(rand::random());

        let new_account = Account {
            user_id,
            balances: BTreeMap::new(),
        };

        self.public_key_cache.insert(user_id, verifying_key);
        self.account_cache.insert(user_id, new_account);

        (user_id, secret_bytes)
    }

    /// Credits a user's account with a specified amount of a given asset.
    /// This simulates a deposit from an L1 contract.
    pub fn deposit(&self, user_id: UserID, asset_id: AssetID, amount: Decimal) -> Result<(), ExecutionPlaneError> {
        let mut account = self.account_cache.get_mut(&user_id)
            .ok_or(ExecutionPlaneError::UserNotFound(user_id))?;
        
        *account.balances.entry(asset_id).or_default() += amount;

        Ok(())
    }
}