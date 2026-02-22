use crate::error::AppError;

const SERVICE_NAME: &str = "semantic-file-search";
const KEY_NAME: &str = "gemini-api-key";

/// Store the Gemini API key in the OS keychain.
pub fn store_api_key(key: &str) -> Result<(), AppError> {
    let entry = keyring::Entry::new(SERVICE_NAME, KEY_NAME)
        .map_err(|e| AppError::Keychain(format!("failed to create keychain entry: {e}")))?;
    entry
        .set_password(key)
        .map_err(|e| AppError::Keychain(format!("failed to store API key: {e}")))?;
    Ok(())
}

/// Retrieve the Gemini API key from the OS keychain.
/// Returns `Ok(None)` if no key is stored.
pub fn get_api_key() -> Result<Option<String>, AppError> {
    let entry = keyring::Entry::new(SERVICE_NAME, KEY_NAME)
        .map_err(|e| AppError::Keychain(format!("failed to create keychain entry: {e}")))?;
    match entry.get_password() {
        Ok(key) => Ok(Some(key)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(AppError::Keychain(format!(
            "failed to retrieve API key: {e}"
        ))),
    }
}

/// Delete the Gemini API key from the OS keychain.
/// Returns `Ok(())` even if no key was stored.
pub fn delete_api_key() -> Result<(), AppError> {
    let entry = keyring::Entry::new(SERVICE_NAME, KEY_NAME)
        .map_err(|e| AppError::Keychain(format!("failed to create keychain entry: {e}")))?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(AppError::Keychain(format!("failed to delete API key: {e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Keychain tests interact with real OS keychain and must run
    // sequentially to avoid race conditions. We consolidate them
    // into a single test that covers the full lifecycle.

    #[test]
    fn test_keychain_lifecycle_store_get_delete() {
        // 1. Ensure clean state
        let _ = delete_api_key();

        // 2. Verify no key stored (may fail if no keychain backend)
        match get_api_key() {
            Ok(key) => assert_eq!(key, None, "expected no key after delete"),
            Err(_) => return, // No keychain backend available; skip
        }

        // 3. Store a key
        if store_api_key("test-lifecycle-key").is_err() {
            return; // No keychain backend available; skip
        }

        // 4. Verify retrieval
        let key = get_api_key().unwrap();
        assert_eq!(key, Some("test-lifecycle-key".to_string()));

        // 5. Delete and verify gone
        delete_api_key().unwrap();
        let key = get_api_key().unwrap();
        assert_eq!(key, None, "expected no key after final delete");
    }
}
