//! File locations and the Withings app credentials.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Serialize, de::DeserializeOwned};

/// Must match a callback URL registered for the app on developer.withings.com.
pub const DEFAULT_REDIRECT_URI: &str = "http://localhost:8765/callback";

#[derive(Debug, Clone, PartialEq)]
pub struct Credentials {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
}

/// KEY=VALUE lines; blank lines and `#` comments are ignored, and one pair of
/// surrounding quotes is stripped.
pub fn parse_env_file(text: &str) -> HashMap<String, String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| {
            let value = value.trim();
            let unquoted = ['"', '\'']
                .iter()
                .find_map(|q| value.strip_prefix(*q).and_then(|v| v.strip_suffix(*q)))
                .unwrap_or(value);
            (key.trim().to_string(), unquoted.to_string())
        })
        .collect()
}

/// Environment variables win over the file, so a systemd EnvironmentFile and
/// a manual run behave the same.
pub fn credentials(env_vars: &HashMap<String, String>, file: &HashMap<String, String>) -> Result<Credentials> {
    let get = |key: &str| env_vars.get(key).or_else(|| file.get(key)).filter(|v| !v.is_empty()).cloned();
    let required = |key: &str| {
        get(key).ok_or_else(|| {
            anyhow::anyhow!(
                "{key} is not set; put it in {} (KEY=VALUE, from your app on developer.withings.com) or the environment",
                client_env_path().display()
            )
        })
    };
    Ok(Credentials {
        client_id: required("WITHINGS_CLIENT_ID")?,
        client_secret: required("WITHINGS_CLIENT_SECRET")?,
        redirect_uri: get("WITHINGS_REDIRECT_URI").unwrap_or_else(|| DEFAULT_REDIRECT_URI.into()),
    })
}

fn home() -> PathBuf {
    env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

fn xdg(variable: &str, fallback: &str) -> PathBuf {
    env::var_os(variable).filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home().join(fallback))
}

pub fn client_env_path() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("withings/client.env")
}

pub fn tokens_path() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("withings/tokens.json")
}

pub fn load_credentials() -> Result<Credentials> {
    let path = client_env_path();
    let file = match fs::read_to_string(&path) {
        Ok(text) => parse_env_file(&text),
        Err(e) if e.kind() == io::ErrorKind::NotFound => HashMap::new(),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    credentials(&env::vars().collect(), &file)
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(serde_json::from_str(&text).with_context(|| format!("{} is not valid JSON", path.display()))?)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
    }
}

/// Written to a temp file and renamed. `private` makes it readable by the
/// owner only (tokens).
pub fn write_json<T: Serialize>(path: &Path, value: &T, private: bool) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    let mode = if private { 0o600 } else { 0o644 };
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(&tmp)
        .with_context(|| format!("cannot write {}", tmp.display()))?;
    file.write_all(serde_json::to_string_pretty(value)?.as_bytes())?;
    file.sync_all()?;
    fs::rename(&tmp, path).with_context(|| format!("cannot replace {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn parses_env_file_lines() {
        let parsed = parse_env_file("# app\nWITHINGS_CLIENT_ID=abc\n\nWITHINGS_CLIENT_SECRET=\"s e\"\nBROKEN\n");
        assert_eq!(parsed, map(&[("WITHINGS_CLIENT_ID", "abc"), ("WITHINGS_CLIENT_SECRET", "s e")]));
    }

    #[test]
    fn environment_overrides_the_file_and_redirect_has_a_default() {
        let file = map(&[("WITHINGS_CLIENT_ID", "file-id"), ("WITHINGS_CLIENT_SECRET", "file-secret")]);
        let creds = credentials(&map(&[("WITHINGS_CLIENT_ID", "env-id")]), &file).unwrap();
        assert_eq!(
            creds,
            Credentials { client_id: "env-id".into(), client_secret: "file-secret".into(), redirect_uri: DEFAULT_REDIRECT_URI.into() }
        );
    }

    #[test]
    fn missing_credentials_say_where_to_put_them() {
        let err = credentials(&HashMap::new(), &map(&[("WITHINGS_CLIENT_ID", "id")])).unwrap_err().to_string();
        assert!(err.contains("WITHINGS_CLIENT_SECRET") && err.contains("client.env"), "{err}");
    }

    #[test]
    fn private_json_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a/tokens.json");
        write_json(&path, &serde_json::json!({"a": 5}), true).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(read_json::<serde_json::Value>(&path).unwrap(), Some(serde_json::json!({"a": 5})));
        assert_eq!(read_json::<serde_json::Value>(&dir.path().join("none.json")).unwrap(), None);
    }
}
