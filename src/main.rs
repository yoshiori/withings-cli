//! withings: command-line client for the Withings API. See README.md.

mod api;
mod config;
mod measure;
mod oauth;
mod output;
mod params;

use std::io::{self, BufRead, Write};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use chrono::{Local, NaiveDate, Utc};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde_json::Value;

use crate::config::{Credentials, load_credentials, read_json, tokens_path, write_json};
use crate::oauth::{Tokens, authorize_url, code_from_redirect, needs_refresh, tokens_from};
use crate::output::{Format, render};
use crate::params::{Range, parse_pairs, timestamp_params, ymd_params};

/// The consent page must be finished within this time.
const AUTH_WAIT: Duration = Duration::from_secs(300);
const LISTENER_POLL: Duration = Duration::from_millis(500);

#[derive(Parser)]
#[command(version, about = "Command-line client for the Withings API")]
struct Cli {
    /// Output format
    #[arg(short, long, global = true, value_enum, default_value_t = Format::Json)]
    format: Format,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Log in to Withings once and store the tokens (~/.config/withings/tokens.json)
    Auth {
        /// Paste the redirected URL instead of catching it on localhost
        #[arg(long)]
        manual: bool,
    },
    /// Body measurements: weight, body fat, blood pressure, ... (measure/getmeas)
    Measure {
        /// Comma-separated type names or ids, e.g. weight,fat_ratio
        #[arg(long, default_value = "weight", conflicts_with = "all")]
        types: String,
        /// Every measure type
        #[arg(long)]
        all: bool,
        /// Real measurements, goals typed in the app, or both
        #[arg(long, value_enum, default_value_t = Category::Real)]
        category: Category,
        #[command(flatten)]
        range: RangeArgs,
    },
    /// Daily activity: steps, distance, calories, ... (v2/measure/getactivity)
    Activity(SeriesArgs),
    /// Sleep summaries (v2/sleep/getsummary)
    Sleep(SeriesArgs),
    /// Workouts (v2/measure/getworkouts)
    Workouts(SeriesArgs),
    /// Any API call: withings raw v2/sleep getsummary startdateymd=2026-11-01
    Raw {
        /// Service path after https://wbsapi.withings.net/, e.g. measure or v2/sleep
        service: String,
        /// The `action` parameter, e.g. getmeas
        action: String,
        /// Extra parameters as key=value
        params: Vec<String>,
    },
}

#[derive(Clone, Copy, PartialEq, ValueEnum)]
enum Category {
    Real,
    Goal,
    All,
}

#[derive(Args)]
struct RangeArgs {
    /// First day (YYYY-MM-DD, local time)
    #[arg(long)]
    since: Option<NaiveDate>,
    /// Last day, inclusive (YYYY-MM-DD, local time)
    #[arg(long)]
    until: Option<NaiveDate>,
    /// Only items created or edited after this Unix time
    #[arg(long)]
    lastupdate: Option<i64>,
}

impl RangeArgs {
    fn range(&self) -> Range {
        Range { since: self.since, until: self.until, lastupdate: self.lastupdate }
    }
}

#[derive(Args)]
struct SeriesArgs {
    #[command(flatten)]
    range: RangeArgs,
    /// Comma-separated `data_fields` to request (see the Withings API reference)
    #[arg(long)]
    fields: Option<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let items = match cli.command {
        Command::Auth { manual } => return auth(manual),
        Command::Measure { types, all, category, range } => measure(&types, all, category, &range.range())?,
        Command::Activity(args) => series("v2/measure", "getactivity", "activities", &args)?,
        Command::Sleep(args) => series("v2/sleep", "getsummary", "series", &args)?,
        Command::Workouts(args) => series("v2/measure", "getworkouts", "series", &args)?,
        Command::Raw { service, action, params } => {
            let client = api::Client::new()?;
            let tokens = fresh_tokens(&client)?;
            let body = client.call(&tokens.access_token, &service, &action, &parse_pairs(&params)?)?;
            vec![body]
        }
    };
    print!("{}", render(&items, cli.format));
    Ok(())
}

fn measure(types: &str, all: bool, category: Category, range: &Range) -> Result<Vec<Value>> {
    let mut params = timestamp_params(range, &Local);
    if !all {
        let ids: Vec<String> = measure::parse_types(types)?.iter().map(i64::to_string).collect();
        params.push(("meastypes".into(), ids.join(",")));
    }
    match category {
        Category::Real => params.push(("category".into(), "1".into())),
        Category::Goal => params.push(("category".into(), "2".into())),
        Category::All => {}
    }
    let client = api::Client::new()?;
    let tokens = fresh_tokens(&client)?;
    let groups: Vec<measure::MeasureGroup> = client
        .call_all(&tokens.access_token, "measure", "getmeas", &params, "measuregrps")?
        .into_iter()
        .map(serde_json::from_value)
        .collect::<Result<_, _>>()
        .context("Withings sent a measure group in an unexpected shape")?;
    measure::to_records(&groups, &Local).iter().map(|r| Ok(serde_json::to_value(r)?)).collect()
}

fn series(service: &str, action: &str, list_key: &str, args: &SeriesArgs) -> Result<Vec<Value>> {
    let mut params = ymd_params(&args.range.range());
    if let Some(fields) = &args.fields {
        params.push(("data_fields".into(), fields.clone()));
    }
    let client = api::Client::new()?;
    let tokens = fresh_tokens(&client)?;
    client.call_all(&tokens.access_token, service, action, &params, list_key)
}

fn auth(manual: bool) -> Result<()> {
    let creds = load_credentials()?;
    let state = uuid::Uuid::new_v4().to_string();
    eprintln!("Open this URL in a browser and allow access:\n\n{}\n", authorize_url(&creds.client_id, &creds.redirect_uri, &state));
    let redirect = if manual { read_pasted_redirect()? } else { catch_redirect(&creds)? };
    let code = code_from_redirect(&redirect, &state)?;
    // The code is valid for only 30 seconds: exchange it right away.
    let tokens = tokens_from(api::Client::new()?.exchange_code(&creds, &code)?, Utc::now().timestamp());
    write_json(&tokens_path(), &tokens, true)?;
    eprintln!("Logged in as Withings user {}. Tokens saved to {}.", tokens.userid, tokens_path().display());
    Ok(())
}

fn read_pasted_redirect() -> Result<String> {
    eprint!("After allowing, paste the full URL the browser was sent to: ");
    io::stderr().flush()?;
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    Ok(line)
}

/// Serves the redirect URI's port on localhost until the browser comes back.
fn catch_redirect(creds: &Credentials) -> Result<String> {
    let url = url::Url::parse(&creds.redirect_uri).with_context(|| format!("redirect URI {} is not a URL", creds.redirect_uri))?;
    let host = url.host_str().unwrap_or_default();
    if url.scheme() != "http" || !matches!(host, "localhost" | "127.0.0.1") {
        bail!("cannot listen on {}; run `withings auth --manual` and paste the redirected URL", creds.redirect_uri);
    }
    let port = url.port_or_known_default().unwrap_or(80);
    let server = tiny_http::Server::http(("127.0.0.1", port))
        .map_err(|e| anyhow::anyhow!("cannot listen on port {port} ({e}); free the port or use --manual"))?;
    eprintln!("Waiting for the browser on {} ...", creds.redirect_uri);
    let deadline = Instant::now() + AUTH_WAIT;
    while Instant::now() < deadline {
        let Some(request) = server.recv_timeout(LISTENER_POLL)? else { continue };
        if !request.url().starts_with(url.path()) {
            request.respond(tiny_http::Response::empty(404))?;
            continue;
        }
        let redirect = request.url().to_string();
        request.respond(tiny_http::Response::from_string("withings: done. You can close this tab."))?;
        return Ok(redirect);
    }
    bail!("no answer from the browser within {} minutes; run `withings auth` again", AUTH_WAIT.as_secs() / 60)
}

fn fresh_tokens(client: &api::Client) -> Result<Tokens> {
    let Some(tokens) = read_json::<Tokens>(&tokens_path())? else {
        bail!("not logged in: {} is missing; run `withings auth` first", tokens_path().display());
    };
    let now = Utc::now().timestamp();
    if !needs_refresh(&tokens, now) {
        return Ok(tokens);
    }
    let refreshed = tokens_from(client.refresh(&load_credentials()?, &tokens.refresh_token)?, now);
    // Withings rotates the refresh token; the old one stops working 8 hours later.
    write_json(&tokens_path(), &refreshed, true)?;
    Ok(refreshed)
}
