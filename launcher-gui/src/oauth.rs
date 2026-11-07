//! OAuth 2.0 PKCE flow for Zwift login.
//!
//! This module implements the authorization code flow with PKCE (Proof Key for Code Exchange)
//! against the Zwift OAuth servers. It is used by the launcher GUI to obtain a token that
//! can be passed to ZwiftApp.exe via `--token=<json>`.

use serde_json::Value;
use std::path::{Path, PathBuf};

const AUTH_URL: &str = "https://secure.zwift.com/auth/realms/zwift/protocol/openid-connect/auth";
const TOKEN_URL: &str = "https://secure.zwift.com/auth/realms/zwift/protocol/openid-connect/token";
const REDIRECT_URI: &str = "http://zwift/";
const CLIENT_ID: &str = "Game_Launcher";
const SCOPE: &str = "openid";

/// Generate a random PKCE code verifier (base64url-encoded 32 bytes).
pub fn generate_pkce_verifier() -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).expect("Failed to generate random bytes");
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Compute the PKCE code challenge from a verifier using SHA256.
pub fn compute_pkce_challenge(verifier: &str) -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let result = hasher.finalize();
    URL_SAFE_NO_PAD.encode(result)
}

/// Generate a random state for CSRF protection.
pub fn generate_random_state() -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).expect("Failed to generate random bytes");
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Build the OAuth authorization URL.
pub fn build_authorization_url(challenge: &str, state: &str) -> String {
    format!(
        "{}?client_id={}&response_type=code&redirect_uri={}&scope={}&state={}&code_challenge={}&code_challenge_method=S256",
        AUTH_URL,
        urlencoding::encode(CLIENT_ID),
        urlencoding::encode(REDIRECT_URI),
        urlencoding::encode(SCOPE),
        urlencoding::encode(state),
        urlencoding::encode(challenge),
    )
}

/// Exchange an authorization code for an OAuth token.
pub fn exchange_code_for_token(code: &str, verifier: &str) -> Result<Value, String> {
    let body = format!(
        "client_id={}&grant_type=authorization_code&code={}&redirect_uri={}&code_verifier={}",
        urlencoding::encode(CLIENT_ID),
        urlencoding::encode(code),
        urlencoding::encode(REDIRECT_URI),
        urlencoding::encode(verifier),
    );
    let response = ureq::post(TOKEN_URL)
        .set("Accept", "application/json")
        .set("Content-Type", "application/x-www-form-urlencoded")
        .send_string(&body)
        .map_err(|e| format!("Token request failed: {e}"))?;
    let body: String = response
        .into_string()
        .map_err(|e| format!("Failed to read token response body: {e}"))?;
    let json: Value =
        serde_json::from_str(&body).map_err(|e| format!("Invalid token response: {e}"))?;
    if json["access_token"].as_str().is_none_or(str::is_empty) {
        return Err("Token response has no access token".into());
    }
    Ok(json)
}

/// Save the token JSON to a file accessible only by its owner.
pub fn save_token(token: &Value, path: &Path) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let json_str = serde_json::to_string_pretty(token)
        .map_err(|e| format!("Failed to serialize token: {e}"))?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("Failed to open token file: {e}"))?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("Failed to protect token file: {e}"))?;
    file.write_all(json_str.as_bytes())
        .map_err(|e| format!("Failed to write token file: {e}"))?;
    Ok(())
}

/// Exchange an authorization code for a token and save it to the standard token path.
pub fn exchange_and_save(code: &str, verifier: &str) -> Result<(), String> {
    let token = exchange_code_for_token(code, verifier)?;
    let token_path = std::env::var("SNAP_USER_COMMON")
        .map(|common| PathBuf::from(common).join("pairing-token.json"))
        .map_err(|_| "SNAP_USER_COMMON is not set".to_owned())?;
    save_token(&token, &token_path)
}
