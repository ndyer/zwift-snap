//! OAuth 2.0 PKCE flow for Zwift login.
//!
//! This module implements the authorization code flow with PKCE (Proof Key for Code Exchange)
//! against the Zwift OAuth servers. It is used by the launcher GUI to obtain a token that
//! can be passed to ZwiftApp.exe via `--token=<json>`.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

const AUTH_URL: &str = "https://secure.zwift.com/auth/realms/zwift/protocol/openid-connect/auth";
const TOKEN_URL: &str = "https://secure.zwift.com/auth/realms/zwift/protocol/openid-connect/token";
const REDIRECT_URI: &str = "http://zwift/";
const CLIENT_ID: &str = "Game_Launcher";
const SCOPE: &str = "openid";
const TOKEN_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

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

/// Build a readable message from a failed token response. Only the `error` and
/// `error_description` fields are shown, never the rest of the body.
fn describe_token_error(status: u16, body: &str) -> String {
    let detail = serde_json::from_str::<Value>(body).ok().and_then(|json| {
        let error = json["error"].as_str()?;
        Some(match json["error_description"].as_str() {
            Some(description) if !description.is_empty() => format!("{error}: {description}"),
            _ => error.to_owned(),
        })
    });
    match detail {
        Some(detail) => format!("Token request failed (HTTP {status}): {detail}"),
        None => format!("Token request failed (HTTP {status})"),
    }
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
    let response = match ureq::post(TOKEN_URL)
        .timeout(TOKEN_REQUEST_TIMEOUT)
        .set("Accept", "application/json")
        .set("Content-Type", "application/x-www-form-urlencoded")
        .send_string(&body)
    {
        Ok(response) => response,
        Err(ureq::Error::Status(status, response)) => {
            let text = response.into_string().unwrap_or_default();
            return Err(describe_token_error(status, &text));
        }
        Err(e) => return Err(format!("Token request failed: {e}")),
    };
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

/// Identity claims read out of an access token, used to label an account card.
///
/// Every field is optional: a token that is not a JWT, or whose payload carries
/// no such claim, simply yields an empty value rather than an error.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Claims {
    pub sub: Option<String>,
    pub preferred_username: Option<String>,
    pub email: Option<String>,
}

/// Read the identity claims from `token`'s `access_token`. The JWT signature is
/// deliberately *not* verified: this only labels an account whose token the
/// OAuth exchange has already obtained. A token that is not a JWT (or whose
/// payload is not a JSON object) yields empty claims.
pub fn token_claims(token: &Value) -> Claims {
    let Some(payload) = token["access_token"].as_str().and_then(jwt_payload) else {
        return Claims::default();
    };
    let string = |name: &str| {
        payload
            .get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    Claims {
        sub: string("sub"),
        preferred_username: string("preferred_username"),
        email: string("email"),
    }
}

/// Decode the payload (the middle, dot-separated segment) of a JWT into JSON.
/// Returns `None` unless that segment is base64url-decodable JSON, so a token
/// in any other shape is tolerated rather than rejected.
fn jwt_payload(jwt: &str) -> Option<Value> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let payload = jwt.split('.').nth(1)?.trim_end_matches('=');
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Exchange an authorization code for a token and save it to the standard token path.
pub fn exchange_and_save(code: &str, verifier: &str) -> Result<(), String> {
    let token = exchange_code_for_token(code, verifier)?;
    let token_path = std::env::var("SNAP_USER_COMMON")
        .map(|common| PathBuf::from(common).join("pairing-token.json"))
        .map_err(|_| "SNAP_USER_COMMON is not set".to_owned())?;
    save_token(&token, &token_path)
}

#[cfg(test)]
mod tests {
    use super::{describe_token_error, token_claims, Claims};
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use serde_json::json;

    #[test]
    fn token_error_shows_error_and_description() {
        let body = r#"{"error":"invalid_grant","error_description":"Code not valid"}"#;
        assert_eq!(
            describe_token_error(400, body),
            "Token request failed (HTTP 400): invalid_grant: Code not valid"
        );
    }

    #[test]
    fn token_error_without_description_shows_error_only() {
        assert_eq!(
            describe_token_error(401, r#"{"error":"unauthorized_client"}"#),
            "Token request failed (HTTP 401): unauthorized_client"
        );
    }

    #[test]
    fn token_error_with_unparseable_body_shows_status_only() {
        assert_eq!(
            describe_token_error(502, "<html>bad gateway</html>"),
            "Token request failed (HTTP 502)"
        );
    }

    /// Build a token whose `access_token` is a JWT carrying `payload`.
    fn token_with_payload(payload: &serde_json::Value) -> serde_json::Value {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256"}"#);
        let body = URL_SAFE_NO_PAD.encode(payload.to_string().as_bytes());
        json!({ "access_token": format!("{header}.{body}.signature") })
    }

    #[test]
    fn claims_are_read_from_the_jwt_payload() {
        let token = token_with_payload(&json!({
            "sub": "abc-123",
            "preferred_username": "sara.campbell",
            "email": "sara@example.com"
        }));
        assert_eq!(
            token_claims(&token),
            Claims {
                sub: Some("abc-123".to_owned()),
                preferred_username: Some("sara.campbell".to_owned()),
                email: Some("sara@example.com".to_owned()),
            }
        );
    }

    #[test]
    fn missing_or_empty_claims_are_none() {
        let token = token_with_payload(&json!({ "sub": "", "email": 42 }));
        assert_eq!(
            token_claims(&token),
            Claims {
                sub: None,
                preferred_username: None,
                email: None,
            }
        );
    }

    #[test]
    fn padded_payload_is_accepted() {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
        let body = URL_SAFE_NO_PAD.encode(br#"{"sub":"x"}"#);
        let token = json!({ "access_token": format!("{header}.{body}=.sig") });
        assert_eq!(token_claims(&token).sub.as_deref(), Some("x"));
    }

    #[test]
    fn non_jwt_tokens_yield_empty_claims() {
        for value in [
            json!({ "access_token": "not-a-jwt" }),
            json!({ "access_token": "a.@@@.c" }),
            json!({ "refresh_token": "abc" }),
            json!({ "access_token": 7 }),
            json!(null),
        ] {
            assert_eq!(token_claims(&value), Claims::default());
        }
    }

    #[test]
    fn non_object_payload_yields_empty_claims() {
        let token = token_with_payload(&json!(["not", "an", "object"]));
        assert_eq!(token_claims(&token), Claims::default());
    }
}
