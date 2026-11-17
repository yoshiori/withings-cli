//! Withings OAuth 2.0: the consent URL, the redirect it comes back with, and
//! the tokens it yields.

use anyhow::{Result, bail};
use url::Url;
use serde::{Deserialize, Serialize};

pub const AUTHORIZE_URL: &str = "https://account.withings.com/oauth2_user/authorize2";
/// Body measurements need user.metrics; activity, sleep and workouts need
/// user.activity.
pub const SCOPE: &str = "user.info,user.metrics,user.activity";

/// Refresh this long before the 3-hour access token actually expires, so a
/// slow request never goes out with a dead token.
pub const REFRESH_MARGIN_SECONDS: i64 = 300;

/// Kept in ~/.config/withings/tokens.json (mode 600).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tokens {
    pub userid: String,
    pub access_token: String,
    pub refresh_token: String,
    /// Unix seconds.
    pub expires_at: i64,
}

/// `body` of a successful `requesttoken` response.
#[derive(Debug, Deserialize)]
pub struct TokenBody {
    pub userid: serde_json::Value,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
}

pub fn authorize_url(client_id: &str, redirect_uri: &str, state: &str) -> String {
    let mut url = Url::parse(AUTHORIZE_URL).expect("AUTHORIZE_URL is a valid URL");
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("scope", SCOPE)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("state", state);
    url.into()
}

/// The `code` from the URL Withings redirects to, after checking `state`.
/// Accepts the full URL, a path with a query (what the local listener sees),
/// or a partial copy of the query such as `code=...&state=...` or
/// `<code>&state=...`.
pub fn code_from_redirect(redirect: &str, expected_state: &str) -> Result<String> {
    let query = query_part(redirect.trim());
    let pairs: Vec<(String, String)> = url::form_urlencoded::parse(query.as_bytes()).into_owned().collect();
    let param = |name: &str| {
        pairs.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone()).or_else(|| {
            // A bare first item (`<code>&state=...`) is the code without its key.
            (name == "code").then(|| pairs.first().filter(|(_, v)| v.is_empty()).map(|(k, _)| k.clone())).flatten()
        })
    };
    if param("state").as_deref() != Some(expected_state) {
        bail!("the redirect's state does not match this login; start `withings auth` again and use the newest URL");
    }
    if let Some(error) = param("error") {
        bail!("Withings refused the authorization ({error}); allow access on the consent page and try again");
    }
    match param("code") {
        Some(code) if !code.is_empty() => Ok(code),
        _ => bail!("the redirect has no code; paste the full URL the browser was sent to"),
    }
}

fn query_part(redirect: &str) -> &str {
    match redirect.split_once('?') {
        Some((_, query)) => query,
        None => redirect,
    }
}

pub fn tokens_from(body: TokenBody, now: i64) -> Tokens {
    let userid = match body.userid {
        serde_json::Value::String(s) => s,
        other => other.to_string(),
    };
    Tokens { userid, access_token: body.access_token, refresh_token: body.refresh_token, expires_at: now + body.expires_in }
}

pub fn needs_refresh(tokens: &Tokens, now: i64) -> bool {
    now >= tokens.expires_at - REFRESH_MARGIN_SECONDS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorize_url_carries_client_scope_redirect_and_state() {
        let url = url::Url::parse(&authorize_url("cid", "http://localhost:8765/callback", "s1")).unwrap();
        let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert!(url.as_str().starts_with(AUTHORIZE_URL));
        assert_eq!(q["response_type"], "code");
        assert_eq!(q["client_id"], "cid");
        assert_eq!(q["scope"], SCOPE);
        assert_eq!(q["redirect_uri"], "http://localhost:8765/callback");
        assert_eq!(q["state"], "s1");
    }

    #[test]
    fn reads_the_code_from_the_redirect() {
        let code = code_from_redirect("http://localhost:8765/callback?code=abc&state=s1", "s1").unwrap();
        assert_eq!(code, "abc");
        // The local listener only sees the path and query.
        assert_eq!(code_from_redirect("/callback?state=s1&code=xyz", "s1").unwrap(), "xyz");
    }

    #[test]
    fn accepts_partial_copies_of_the_redirect() {
        // Copying from the address bar often loses the start of the URL.
        assert_eq!(code_from_redirect("?code=abc&state=s1", "s1").unwrap(), "abc");
        assert_eq!(code_from_redirect("code=abc&state=s1", "s1").unwrap(), "abc");
        assert_eq!(code_from_redirect("abc&state=s1\n", "s1").unwrap(), "abc");
    }

    #[test]
    fn rejects_a_redirect_with_another_state() {
        let err = code_from_redirect("/callback?code=abc&state=other", "s1").unwrap_err();
        assert!(err.to_string().contains("state"), "{err}");
    }

    #[test]
    fn reports_a_refused_consent() {
        let err = code_from_redirect("/callback?error=access_denied&state=s1", "s1").unwrap_err();
        assert!(err.to_string().contains("access_denied"), "{err}");
    }

    #[test]
    fn token_expiry_is_absolute_and_userid_a_string() {
        let body: TokenBody = serde_json::from_str(
            r#"{"userid":12345,"access_token":"a","refresh_token":"r","expires_in":10800,"scope":"user.metrics","token_type":"Bearer"}"#,
        )
        .unwrap();
        let t = tokens_from(body, 1_000);
        assert_eq!(t, Tokens { userid: "12345".into(), access_token: "a".into(), refresh_token: "r".into(), expires_at: 11_800 });
    }

    #[test]
    fn refreshes_shortly_before_expiry() {
        let t = Tokens { userid: "1".into(), access_token: "a".into(), refresh_token: "r".into(), expires_at: 10_000 };
        assert!(!needs_refresh(&t, 10_000 - REFRESH_MARGIN_SECONDS - 1));
        assert!(needs_refresh(&t, 10_000 - REFRESH_MARGIN_SECONDS));
        assert!(needs_refresh(&t, 20_000));
    }
}
