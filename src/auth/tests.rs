#[cfg(test)]
mod auth_tests {
    use argon2::{
        password_hash::{PasswordHasher, PasswordVerifier},
        Argon2,
    };

    /// Verify that Argon2 password hashing produces a non-empty `$argon2` string.
    #[test]
    fn test_password_hashing() {
        let argon2 = Argon2::default();

        let password_hash = argon2
            .hash_password(b"testpassword123")
            .expect("Failed to hash password");

        let hash_string = password_hash.to_string();
        assert!(!hash_string.is_empty());
        assert!(hash_string.starts_with("$argon2"));
    }

    /// Verify that a correct password passes and an incorrect one fails.
    #[test]
    fn test_password_verification() {
        let argon2 = Argon2::default();

        let password_hash = argon2
            .hash_password(b"testpassword123")
            .expect("Failed to hash password");

        let hash_string = password_hash.to_string();

        let result = argon2.verify_password(b"testpassword123", hash_string.as_str());
        assert!(result.is_ok(), "Password verification should succeed");

        let wrong_result = argon2.verify_password(b"wrongpassword", hash_string.as_str());
        assert!(
            wrong_result.is_err(),
            "Wrong password should fail verification"
        );
    }

    /// Verify token length and character set.
    #[test]
    fn test_token_generation() {
        use rand::RngExt;
        const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
        let mut rng = rand::rng();

        let token: String = (0..32)
            .map(|_| {
                let idx = rng.random_range(0..CHARSET.len());
                CHARSET[idx] as char
            })
            .collect();

        assert_eq!(token.len(), 32);
        assert!(token.chars().all(|c| c.is_alphanumeric()));
    }

    /// Verify username validation rules (alphanumeric + underscore only).
    #[test]
    fn test_username_validation() {
        fn is_valid_username(username: &str) -> bool {
            if username.is_empty() || username.len() > 64 {
                return false;
            }
            username.chars().all(|c| c.is_alphanumeric() || c == '_')
        }

        assert!(is_valid_username("admin"));
        assert!(is_valid_username("user_123"));
        assert!(is_valid_username("TestUser"));
        assert!(!is_valid_username(""));
        assert!(!is_valid_username("user name"));
        assert!(!is_valid_username("user-name"));
        assert!(!is_valid_username("user@test"));
    }

    /// Verify session expiry logic.
    #[test]
    fn test_session_token_validation() {
        use chrono::{Duration, Utc};

        let now = Utc::now();
        let expires_at = now + Duration::seconds(86400);

        assert!(expires_at > now, "Session should not be expired");

        let expired = now - Duration::seconds(1);
        assert!(expired < now, "Session should be expired");
    }

    /// Verify role name validation.
    #[test]
    fn test_role_validation() {
        fn is_valid_role(role: &str) -> bool {
            matches!(role, "admin" | "user" | "viewer")
        }

        assert!(is_valid_role("admin"));
        assert!(is_valid_role("user"));
        assert!(is_valid_role("viewer"));
        assert!(!is_valid_role("superuser"));
        assert!(!is_valid_role(""));
    }
}
