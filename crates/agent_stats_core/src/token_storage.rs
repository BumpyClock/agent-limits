use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Error type for token storage operations.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenStorageError {
    /// Token not found in storage.
    NotFound,
    /// Platform-specific error occurred.
    PlatformError(String),
    /// Invalid token provided.
    InvalidToken,
}

impl fmt::Display for TokenStorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TokenStorageError::NotFound => write!(formatter, "Token not found"),
            TokenStorageError::PlatformError(message) => {
                write!(formatter, "Platform error: {}", message)
            }
            TokenStorageError::InvalidToken => write!(formatter, "Invalid token"),
        }
    }
}

impl std::error::Error for TokenStorageError {}

impl From<keyring::Error> for TokenStorageError {
    fn from(error: keyring::Error) -> Self {
        match error {
            keyring::Error::NoEntry => TokenStorageError::NotFound,
            _ => TokenStorageError::PlatformError(error.to_string()),
        }
    }
}

/// Secure cross-platform token storage using the system keychain.
///
/// Uses:
/// - macOS: Keychain
/// - Linux: Secret Service API
/// - Windows: Credential Manager
#[derive(Clone)]
pub struct SecureTokenStorage {
    service_name: String,
}

impl fmt::Debug for SecureTokenStorage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SecureTokenStorage")
            .field("service_name", &self.service_name)
            .finish()
    }
}

impl SecureTokenStorage {
    /// Creates a new secure token storage instance.
    ///
    /// The `service_name` is used as the service identifier in the system keychain.
    pub fn new(service_name: impl Into<String>) -> Self {
        Self {
            service_name: service_name.into(),
        }
    }

    /// Stores a token securely for the given account.
    ///
    /// # Errors
    ///
    /// Returns `TokenStorageError::InvalidToken` if the token is empty.
    /// Returns `TokenStorageError::PlatformError` if the platform storage fails.
    pub fn store(&self, account: &str, token: &str) -> Result<(), TokenStorageError> {
        if token.trim().is_empty() {
            return Err(TokenStorageError::InvalidToken);
        }

        let entry = keyring::Entry::new(&self.service_name, account)?;
        entry.set_password(token)?;
        Ok(())
    }

    /// Retrieves a token from secure storage for the given account.
    ///
    /// # Errors
    ///
    /// Returns `TokenStorageError::NotFound` if no token exists.
    /// Returns `TokenStorageError::PlatformError` if retrieval fails.
    pub fn retrieve(&self, account: &str) -> Result<String, TokenStorageError> {
        let entry = keyring::Entry::new(&self.service_name, account)?;
        let token = entry.get_password()?;
        Ok(token)
    }

    /// Deletes a token from secure storage for the given account.
    ///
    /// # Errors
    ///
    /// Returns `TokenStorageError::NotFound` if no token exists.
    /// Returns `TokenStorageError::PlatformError` if deletion fails.
    pub fn delete(&self, account: &str) -> Result<(), TokenStorageError> {
        let entry = keyring::Entry::new(&self.service_name, account)?;
        entry.delete_credential()?;
        Ok(())
    }

    /// Checks if secure storage is available on this platform.
    ///
    /// Attempts a test operation to verify functionality.
    pub fn is_available(&self) -> bool {
        let test_account = "__test_availability__";
        let test_token = "test";

        // Try to store and retrieve a test entry
        if self.store(test_account, test_token).is_err() {
            return false;
        }

        let result = self.retrieve(test_account).is_ok();

        // Clean up the test entry (ignore errors during cleanup)
        let _ = self.delete(test_account);

        result
    }
}

impl Default for SecureTokenStorage {
    fn default() -> Self {
        Self::new("agent-stats")
    }
}

/// A cached token entry with expiration information.
#[derive(Debug, Clone)]
struct CachedToken {
    token: String,
    expires_at: Instant,
}

impl CachedToken {
    fn new(token: String, ttl: Duration) -> Self {
        Self {
            token,
            expires_at: Instant::now() + ttl,
        }
    }

    fn is_expired(&self) -> bool {
        Instant::now() > self.expires_at
    }
}

/// Thread-safe in-memory token cache with TTL support.
///
/// Tokens are stored in memory with a time-to-live (TTL) and automatically
/// expire after the specified duration.
#[derive(Debug, Clone)]
pub struct TokenCache {
    cache: Arc<RwLock<HashMap<String, CachedToken>>>,
    default_ttl: Duration,
}

impl TokenCache {
    /// Creates a new token cache with the default TTL of 30 minutes.
    pub fn new() -> Self {
        Self::with_ttl(Duration::from_secs(30 * 60))
    }

    /// Creates a new token cache with a custom default TTL.
    ///
    /// # Arguments
    ///
    /// * `default_ttl` - The default time-to-live for cached tokens.
    pub fn with_ttl(default_ttl: Duration) -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            default_ttl,
        }
    }

    /// Stores a token in the cache.
    ///
    /// Uses the default TTL. To use a custom TTL, use `store_with_ttl`.
    pub async fn store(&self, account: impl Into<String>, token: impl Into<String>) {
        self.store_with_ttl(account, token, self.default_ttl).await;
    }

    /// Stores a token in the cache with a custom TTL.
    ///
    /// # Arguments
    ///
    /// * `account` - The account identifier for the token.
    /// * `token` - The token string to cache.
    /// * `ttl` - The time-to-live for this specific token.
    pub async fn store_with_ttl(
        &self,
        account: impl Into<String>,
        token: impl Into<String>,
        ttl: Duration,
    ) {
        let account = account.into();
        let token = token.into();
        let cached = CachedToken::new(token, ttl);

        let mut cache = self.cache.write().await;
        cache.insert(account, cached);
    }

    /// Retrieves a token from the cache if it exists and is not expired.
    ///
    /// Returns `None` if the token is not found or has expired.
    pub async fn retrieve(&self, account: &str) -> Option<String> {
        let cache = self.cache.read().await;

        if let Some(cached) = cache.get(account)
            && !cached.is_expired()
        {
            return Some(cached.token.clone());
        }

        None
    }

    /// Clears all expired tokens from the cache.
    ///
    /// This is useful for periodic cleanup to prevent memory growth.
    pub async fn clear_expired(&self) {
        let mut cache = self.cache.write().await;
        cache.retain(|_, cached| !cached.is_expired());
    }

    /// Clears all tokens from the cache.
    pub async fn clear(&self) {
        let mut cache = self.cache.write().await;
        cache.clear();
    }

    /// Returns the number of tokens currently in the cache.
    pub async fn len(&self) -> usize {
        let cache = self.cache.read().await;
        cache.len()
    }

    /// Returns true if the cache contains no tokens.
    pub async fn is_empty(&self) -> bool {
        let cache = self.cache.read().await;
        cache.is_empty()
    }
}

impl Default for TokenCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Manages token storage combining secure persistent storage with in-memory caching.
///
/// This provides a unified interface for token management:
/// - Tokens are retrieved from cache first (fast)
/// - If not in cache, tokens are loaded from secure storage
/// - New tokens are stored in both cache and secure storage
#[derive(Debug, Clone)]
pub struct TokenManager {
    storage: SecureTokenStorage,
    cache: TokenCache,
}

impl TokenManager {
    /// Creates a new token manager with the given service name.
    ///
    /// Uses default cache TTL of 30 minutes.
    pub fn new(service_name: impl Into<String>) -> Self {
        Self {
            storage: SecureTokenStorage::new(service_name),
            cache: TokenCache::new(),
        }
    }

    /// Creates a new token manager with custom cache TTL.
    pub fn with_ttl(service_name: impl Into<String>, ttl: Duration) -> Self {
        Self {
            storage: SecureTokenStorage::new(service_name),
            cache: TokenCache::with_ttl(ttl),
        }
    }

    /// Retrieves a token, checking cache first, then secure storage.
    ///
    /// If found in secure storage but not in cache, the token is added to the cache.
    pub async fn get_token(&self, account: &str) -> Result<String, TokenStorageError> {
        // Check cache first
        if let Some(token) = self.cache.retrieve(account).await {
            return Ok(token);
        }

        // Load from secure storage
        let token = self.storage.retrieve(account)?;

        // Store in cache for future requests
        self.cache.store(account, &token).await;

        Ok(token)
    }

    /// Stores a token in both cache and secure storage.
    pub async fn store_token(
        &self,
        account: &str,
        token: &str,
    ) -> Result<(), TokenStorageError> {
        // Store in secure storage first
        self.storage.store(account, token)?;

        // Then cache it
        self.cache.store(account, token).await;

        Ok(())
    }

    /// Deletes a token from both cache and secure storage.
    pub async fn delete_token(&self, account: &str) -> Result<(), TokenStorageError> {
        // Delete from cache (ignore errors - may not exist)
        let mut cache = self.cache.cache.write().await;
        cache.remove(account);
        drop(cache);

        // Delete from secure storage
        self.storage.delete(account)?;

        Ok(())
    }

    /// Returns a reference to the underlying secure storage.
    pub fn storage(&self) -> &SecureTokenStorage {
        &self.storage
    }

    /// Returns a reference to the underlying cache.
    pub fn cache(&self) -> &TokenCache {
        &self.cache
    }
}

impl Default for TokenManager {
    fn default() -> Self {
        Self::new("agent-stats")
    }
}

/// Provider account names for use with token storage.
pub mod accounts {
    /// GitHub Copilot account identifier.
    pub const COPILOT: &str = "copilot";

    /// Z AI account identifier.
    pub const ZAI: &str = "zai";

    /// MiniMax account identifier.
    pub const MINIMAX: &str = "minimax";

    /// Kimi account identifier.
    pub const KIMI: &str = "kimi";

    /// Kimi K2 account identifier.
    pub const KIMIK2: &str = "kimik2";

    /// Synthetic account identifier.
    pub const SYNTHETIC: &str = "synthetic";

    /// Claude account identifier.
    pub const CLAUDE: &str = "claude";
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper to generate unique account names for test isolation
    fn test_account(name: &str) -> String {
        format!("{}_{}", name, uuid::Uuid::new_v4())
    }

    #[test]
    fn test_token_storage_error_display() {
        assert_eq!(TokenStorageError::NotFound.to_string(), "Token not found");
        assert_eq!(
            TokenStorageError::PlatformError("test".to_string()).to_string(),
            "Platform error: test"
        );
        assert_eq!(
            TokenStorageError::InvalidToken.to_string(),
            "Invalid token"
        );
    }

    #[test]
    fn test_token_storage_error_from_keyring() {
        let keyring_error = keyring::Error::NoEntry;
        let storage_error: TokenStorageError = keyring_error.into();
        assert_eq!(storage_error, TokenStorageError::NotFound);
    }

    #[test]
    fn test_secure_token_storage_new() {
        let storage = SecureTokenStorage::new("test-service");
        assert_eq!(storage.service_name, "test-service");
    }

    #[test]
    fn test_secure_token_storage_default() {
        let storage = SecureTokenStorage::default();
        assert_eq!(storage.service_name, "agent-stats");
    }

    #[test]
    fn test_secure_token_storage_store_empty_token() {
        let storage = SecureTokenStorage::new("test");
        let result = storage.store("account", "");
        assert!(matches!(result, Err(TokenStorageError::InvalidToken)));

        let result = storage.store("account", "   ");
        assert!(matches!(result, Err(TokenStorageError::InvalidToken)));
    }

    #[test]
    fn test_secure_token_storage_store_and_retrieve() {
        let storage = SecureTokenStorage::new("agent-stats-test");

        // Skip test if secure storage is not available
        if !storage.is_available() {
            println!("Skipping test: secure storage not available");
            return;
        }

        let account = test_account("store_retrieve");

        // Store a token
        storage.store(&account, "my-secret-token").unwrap();

        // Retrieve it
        let retrieved = storage.retrieve(&account).unwrap();
        assert_eq!(retrieved, "my-secret-token");

        // Clean up
        let _ = storage.delete(&account);
    }

    #[test]
    fn test_secure_token_storage_retrieve_not_found() {
        let storage = SecureTokenStorage::new("agent-stats-test");
        let account = test_account("nonexistent");

        let result = storage.retrieve(&account);
        assert!(matches!(result, Err(TokenStorageError::NotFound)));
    }

    #[test]
    fn test_secure_token_storage_delete() {
        let storage = SecureTokenStorage::new("agent-stats-test");

        // Skip test if secure storage is not available
        if !storage.is_available() {
            println!("Skipping test: secure storage not available");
            return;
        }

        let account = test_account("delete_test");

        // Store and delete
        storage.store(&account, "token").unwrap();
        storage.delete(&account).unwrap();

        // Should not be found after deletion
        let result = storage.retrieve(&account);
        assert!(matches!(result, Err(TokenStorageError::NotFound)));
    }

    #[test]
    fn test_secure_token_storage_delete_not_found() {
        let storage = SecureTokenStorage::new("agent-stats-test");
        let account = test_account("delete_nonexistent");

        let result = storage.delete(&account);
        assert!(matches!(result, Err(TokenStorageError::NotFound)));
    }

    #[test]
    fn test_secure_token_storage_is_available() {
        let storage = SecureTokenStorage::new("agent-stats-test");
        // This may return true or false depending on the environment
        // We just verify it doesn't panic
        let _available = storage.is_available();
    }

    #[tokio::test]
    async fn test_token_cache_new() {
        let cache = TokenCache::new();
        assert!(cache.is_empty().await);
        assert_eq!(cache.len().await, 0);
    }

    #[tokio::test]
    async fn test_token_cache_with_ttl() {
        let ttl = Duration::from_secs(60);
        let cache = TokenCache::with_ttl(ttl);

        cache.store("account", "token").await;

        // Should be retrievable immediately
        let retrieved = cache.retrieve("account").await;
        assert_eq!(retrieved, Some("token".to_string()));
    }

    #[tokio::test]
    async fn test_token_cache_store_and_retrieve() {
        let cache = TokenCache::new();

        cache.store("account1", "token1").await;
        cache.store("account2", "token2").await;

        assert_eq!(cache.retrieve("account1").await, Some("token1".to_string()));
        assert_eq!(cache.retrieve("account2").await, Some("token2".to_string()));
        assert_eq!(cache.len().await, 2);
    }

    #[tokio::test]
    async fn test_token_cache_retrieve_not_found() {
        let cache = TokenCache::new();

        let result = cache.retrieve("nonexistent").await;
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_token_cache_expiration() {
        let cache = TokenCache::with_ttl(Duration::from_millis(50));

        cache.store("account", "token").await;

        // Should be available immediately
        assert_eq!(cache.retrieve("account").await, Some("token".to_string()));

        // Wait for expiration
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Should be expired now
        assert_eq!(cache.retrieve("account").await, None);
    }

    #[tokio::test]
    async fn test_token_cache_custom_ttl() {
        let cache = TokenCache::new();

        // Store with very short TTL
        cache
            .store_with_ttl("account", "token", Duration::from_millis(50))
            .await;

        // Should be available immediately
        assert_eq!(cache.retrieve("account").await, Some("token".to_string()));

        // Wait for expiration
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Should be expired
        assert_eq!(cache.retrieve("account").await, None);
    }

    #[tokio::test]
    async fn test_token_cache_clear_expired() {
        let cache = TokenCache::with_ttl(Duration::from_millis(50));

        cache.store("expired", "token1").await;
        cache
            .store_with_ttl("not_expired", "token2", Duration::from_secs(60))
            .await;

        // Wait for first token to expire
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Clear expired
        cache.clear_expired().await;

        // Only the non-expired token should remain
        assert_eq!(cache.len().await, 1);
        assert_eq!(
            cache.retrieve("not_expired").await,
            Some("token2".to_string())
        );
    }

    #[tokio::test]
    async fn test_token_cache_clear() {
        let cache = TokenCache::new();

        cache.store("account1", "token1").await;
        cache.store("account2", "token2").await;

        assert_eq!(cache.len().await, 2);

        cache.clear().await;

        assert_eq!(cache.len().await, 0);
        assert!(cache.is_empty().await);
    }

    #[tokio::test]
    async fn test_token_cache_default() {
        let cache: TokenCache = Default::default();
        assert!(cache.is_empty().await);
    }

    #[tokio::test]
    async fn test_token_manager_new() {
        let manager = TokenManager::new("test-service");
        assert!(manager.cache().is_empty().await);
    }

    #[tokio::test]
    async fn test_token_manager_default() {
        let manager: TokenManager = Default::default();
        assert!(manager.cache().is_empty().await);
    }

    #[tokio::test]
    async fn test_token_manager_store_and_get() {
        let manager = TokenManager::new("agent-stats-test-manager");

        // Skip test if secure storage is not available
        if !manager.storage().is_available() {
            println!("Skipping test: secure storage not available");
            return;
        }

        let account = test_account("manager_test");

        // Store token
        manager.store_token(&account, "secret").await.unwrap();

        // Get from cache (first time)
        let token = manager.get_token(&account).await.unwrap();
        assert_eq!(token, "secret");

        // Get from cache (second time - should be cached)
        let token = manager.get_token(&account).await.unwrap();
        assert_eq!(token, "secret");

        // Clean up
        let _ = manager.delete_token(&account).await;
    }

    #[tokio::test]
    async fn test_token_manager_get_not_found() {
        let manager = TokenManager::new("agent-stats-test");
        let account = test_account("nonexistent");

        let result = manager.get_token(&account).await;
        assert!(matches!(result, Err(TokenStorageError::NotFound)));
    }

    #[tokio::test]
    async fn test_token_manager_delete() {
        let manager = TokenManager::new("agent-stats-test-manager");

        // Skip test if secure storage is not available
        if !manager.storage().is_available() {
            println!("Skipping test: secure storage not available");
            return;
        }

        let account = test_account("delete_manager");

        // Store and cache
        manager.store_token(&account, "token").await.unwrap();

        // Verify it's cached
        assert!(manager.cache().retrieve(&account).await.is_some());

        // Delete
        manager.delete_token(&account).await.unwrap();

        // Verify removed from cache
        assert!(manager.cache().retrieve(&account).await.is_none());

        // Verify removed from storage
        let result = manager.get_token(&account).await;
        assert!(matches!(result, Err(TokenStorageError::NotFound)));
    }

    #[tokio::test]
    async fn test_token_manager_with_ttl() {
        let manager = TokenManager::with_ttl("agent-stats-test", Duration::from_millis(50));

        // Skip test if secure storage is not available
        if !manager.storage().is_available() {
            println!("Skipping test: secure storage not available");
            return;
        }

        let account = test_account("ttl_test");

        // Store token
        manager.store_token(&account, "short_lived").await.unwrap();

        // Should be available
        assert_eq!(manager.get_token(&account).await.unwrap(), "short_lived");

        // Wait for expiration
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Cache should be expired, will try to load from storage
        // Since we stored with short TTL, cache expired but storage still has it
        let token = manager.get_token(&account).await.unwrap();
        assert_eq!(token, "short_lived");

        // Clean up
        let _ = manager.delete_token(&account).await;
    }

    #[test]
    fn test_accounts_constants() {
        assert_eq!(accounts::COPILOT, "copilot");
        assert_eq!(accounts::ZAI, "zai");
        assert_eq!(accounts::MINIMAX, "minimax");
        assert_eq!(accounts::KIMI, "kimi");
        assert_eq!(accounts::KIMIK2, "kimik2");
        assert_eq!(accounts::SYNTHETIC, "synthetic");
        assert_eq!(accounts::CLAUDE, "claude");
    }

    #[test]
    fn test_cached_token_is_expired() {
        let token = CachedToken::new("test".to_string(), Duration::from_millis(1));
        assert!(!token.is_expired());

        std::thread::sleep(Duration::from_millis(10));
        assert!(token.is_expired());
    }
}
