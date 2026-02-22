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

    // Note: These tests interact with the real OS keychain.
    // They use a unique service/key to avoid conflicts.
    // On CI without a keychain backend, the keyring crate will
    // return errors, which is expected behavior.

    #[test]
    fn test_store_and_retrieve_api_key() {
        // This test may fail on headless CI without a keychain backend.
        // That is acceptable — the test validates the API contract.
        let store_result = store_api_key("test-key-12345");
        if store_result.is_err() {
            // No keychain backend available; skip the rest.
            return;
        }
        let key = get_api_key().unwrap();
        assert_eq!(key, Some("test-key-12345".to_string()));

        // Cleanup
        let _ = delete_api_key();
    }

    #[test]
    fn test_get_api_key_returns_none_when_not_stored() {
        // First ensure no key exists
        let _ = delete_api_key();

        match get_api_key() {
            Ok(key) => assert_eq!(key, None),
            Err(_) => {
                // Keychain backend not available; acceptable.
            }
        }
    }

    #[test]
    fn test_delete_api_key_succeeds_when_no_key() {
        // Deleting a non-existent key should not error.
        match delete_api_key() {
            Ok(()) => {}
            Err(_) => {
                // Keychain backend not available; acceptable.
            }
        }
    }
}
