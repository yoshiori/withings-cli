//! HTTP calls to the Withings API. Every response is an envelope
//! `{"status": 0, "body": {...}}`; a non-zero status is an error even with
//! HTTP 200.

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::Value;

use crate::config::Credentials;
use crate::oauth::TokenBody;

pub const API_BASE: &str = "https://wbsapi.withings.net";
pub const TOKEN_URL: &str = "https://wbsapi.withings.net/v2/oauth2";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// A runaway `more` flag must not loop forever.
const MAX_PAGES: usize = 100;

const STATUS_OK: i64 = 0;
const STATUS_INVALID_TOKEN: i64 = 401;
const STATUS_RATE_LIMITED: i64 = 601;

/// Items of one page (the array under `list_key`) and the offset of the
/// next page when the body says there is one (`more`).
pub fn page_items(body: &Value, list_key: &str) -> (Vec<Value>, Option<i64>) {
    let items = body.get(list_key).and_then(Value::as_array).cloned().unwrap_or_default();
    let more = match body.get("more") {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_i64().is_some_and(|n| n != 0),
        _ => false,
    };
    let next = more.then(|| body.get("offset").and_then(Value::as_i64).unwrap_or(0));
    (items, next)
}

/// The `body` of a successful envelope, or an error that says what to do.
pub fn body_of(text: &str) -> Result<Value> {
    #[derive(Deserialize)]
    struct Envelope {
        status: i64,
        #[serde(default)]
        body: Value,
        #[serde(default)]
        error: Option<String>,
    }
    let envelope: Envelope = serde_json::from_str(text)
        .with_context(|| format!("Withings sent a response that is not JSON: {}", text.chars().take(200).collect::<String>()))?;
    let message = envelope.error.unwrap_or_default();
    match envelope.status {
        STATUS_OK => Ok(envelope.body),
        STATUS_INVALID_TOKEN => bail!("Withings rejected the token ({message}); run `withings auth` to log in again"),
        STATUS_RATE_LIMITED => bail!("Withings rate limit reached ({message}); fetch at most once every 10 minutes"),
        status => bail!("Withings API error {status}: {message}"),
    }
}

pub struct Client {
    http: reqwest::blocking::Client,
}

impl Client {
    pub fn new() -> Result<Self> {
        let http = reqwest::blocking::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("withings-cli/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Client { http })
    }

    fn post(&self, url: &str, bearer: Option<&str>, form: &[(String, String)]) -> Result<Value> {
        let mut request = self.http.post(url).form(form);
        if let Some(token) = bearer {
            request = request.bearer_auth(token);
        }
        let response = request.send().with_context(|| format!("cannot reach {url}; check the network and try again"))?;
        let status = response.status();
        let text = response.text()?;
        if !status.is_success() {
            bail!("{url} answered HTTP {status}: {}", text.chars().take(200).collect::<String>());
        }
        body_of(&text)
    }

    fn token_request(&self, creds: &Credentials, grant: &[(&str, &str)]) -> Result<TokenBody> {
        let mut form = vec![
            ("action".to_string(), "requesttoken".to_string()),
            ("client_id".to_string(), creds.client_id.clone()),
            ("client_secret".to_string(), creds.client_secret.clone()),
        ];
        form.extend(grant.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        Ok(serde_json::from_value(self.post(TOKEN_URL, None, &form)?)?)
    }

    pub fn exchange_code(&self, creds: &Credentials, code: &str) -> Result<TokenBody> {
        self.token_request(
            creds,
            &[("grant_type", "authorization_code"), ("code", code), ("redirect_uri", &creds.redirect_uri)],
        )
    }

    pub fn refresh(&self, creds: &Credentials, refresh_token: &str) -> Result<TokenBody> {
        self.token_request(creds, &[("grant_type", "refresh_token"), ("refresh_token", refresh_token)])
    }

    /// One call to `https://wbsapi.withings.net/<service>` with `action`.
    pub fn call(&self, access_token: &str, service: &str, action: &str, params: &[(String, String)]) -> Result<Value> {
        let mut form = vec![("action".to_string(), action.to_string())];
        form.extend(params.iter().cloned());
        self.post(&format!("{API_BASE}/{}", service.trim_matches('/')), Some(access_token), &form)
    }

    /// Calls `action` page by page and returns every item under `list_key`.
    pub fn call_all(
        &self,
        access_token: &str,
        service: &str,
        action: &str,
        params: &[(String, String)],
        list_key: &str,
    ) -> Result<Vec<Value>> {
        let mut items = Vec::new();
        let mut offset = None;
        for _ in 0..MAX_PAGES {
            let mut page_params = params.to_vec();
            if let Some(offset) = offset {
                page_params.push(("offset".to_string(), format!("{offset}")));
            }
            let (page, next) = page_items(&self.call(access_token, service, action, &page_params)?, list_key);
            items.extend(page);
            match next {
                Some(next) => offset = Some(next),
                None => return Ok(items),
            }
        }
        Err(anyhow!("Withings kept reporting more pages after {MAX_PAGES}; narrow the date range"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_the_body_of_a_successful_envelope() {
        assert_eq!(body_of(r#"{"status":0,"body":{"a":1}}"#).unwrap()["a"], 1);
    }

    #[test]
    fn explains_an_expired_or_revoked_token() {
        let err = body_of(r#"{"status":401,"error":"XRequestID: Not provided invalid_token"}"#).unwrap_err().to_string();
        assert!(err.contains("`withings auth`"), "{err}");
    }

    #[test]
    fn explains_the_rate_limit() {
        let err = body_of(r#"{"status":601,"error":"Too Many Requests"}"#).unwrap_err().to_string();
        assert!(err.contains("10 minutes"), "{err}");
    }

    #[test]
    fn other_errors_keep_the_status_and_message() {
        let err = body_of(r#"{"status":503,"error":"Invalid params"}"#).unwrap_err().to_string();
        assert!(err.contains("503") && err.contains("Invalid params"), "{err}");
        assert!(body_of("<html>").is_err());
    }

    #[test]
    fn page_items_and_the_next_offset() {
        let body = serde_json::json!({"more": 1, "offset": 2, "series": [{"a": 1}, {"a": 2}]});
        assert_eq!(page_items(&body, "series"), (vec![serde_json::json!({"a": 1}), serde_json::json!({"a": 2})], Some(2)));
        let last = serde_json::json!({"more": false, "offset": 0, "activities": []});
        assert_eq!(page_items(&last, "activities"), (vec![], None));
        assert_eq!(page_items(&serde_json::json!({"more": true, "offset": 5}), "series"), (vec![], Some(5)));
        assert_eq!(page_items(&serde_json::json!({}), "series"), (vec![], None));
    }
}
