# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```sh
cargo build                          # debug build (binary: target/debug/withings)
cargo test                           # all unit tests (they live in #[cfg(test)] modules in each src file)
cargo test measure::tests::          # one module's tests
cargo test scaled                    # tests whose name contains "scaled"
cargo clippy --all-targets           # keep at zero warnings
cargo install --path .               # install ~/.cargo/bin/withings
cargo run -- measure --since 2026-11-01 -f table
```

Tests are pure. They never touch the network or `~/.config/withings`, so a test can't call `api::Client`. To make code testable, put the logic in a pure function (it takes `&str`/`Value`/`Range`, and a `today` or `now` argument instead of reading the clock) and keep the I/O in `main.rs` or `Client`.

## Architecture

A single binary crate (`withings`, edition 2024) built on blocking `reqwest` with rustls, `clap` derive and `anyhow`. Each module has one job:

- `main.rs`: the CLI definition and the I/O around it: the OAuth flow (`auth`, plus the localhost listener in `catch_redirect`) and `fresh_tokens`, which refreshes the tokens and writes them back. Every data subcommand reduces to `Vec<Value>`, and `output::render` prints it.
- `api.rs`: HTTP. Every Withings response is an envelope `{"status": N, "body": ...}`. A non-zero `status` is an error even when the HTTP status is 200 (`body_of`). `call_all` follows pages through `more`/`offset` and stops after `MAX_PAGES`.
- `params.rs`: turns `--since/--until/--lastupdate` into request parameters. There are **two date conventions**:
  - The v1 `measure` service takes Unix `startdate`/`enddate`, built in local time. `--until` is inclusive (the end of that day).
  - The v2 services (`v2/measure`, `v2/sleep`) take `startdateymd`/`enddateymd` and fail (501/503) unless both are present. `ymd_params` therefore fills a missing end with today and a missing start with `DEFAULT_SPAN_DAYS`.
- `measure.rs`: `getmeas` groups become flat records like `{date, grpid, category, <type_name>: value}`. Values arrive as `value × 10^unit` and are rescaled with rounding (`scaled`). `TYPES` maps meastype ids to names, taken from the official OpenAPI spec (`developer.withings.com/openapi.yaml`). The `llms.md` table is incomplete, so don't use it. Unknown ids become `type_<id>`.
- `oauth.rs`: the authorize URL, parsing the code out of a pasted redirect, and the token lifetime math. Pasting only part of the redirect URL is accepted on purpose.
- `config.rs`: reads credentials from env vars, falling back to `~/.config/withings/client.env` (XDG-aware). Writes `tokens.json` with mode 600.
- `output.rs`: `json` / `jsonl` / `table`. The table flattens nested objects into dotted columns.

`sleep-detail` and `intraday` don't page. They call once per day (`params::day_windows`, then `per_day` in `main.rs`) because long `v2/sleep get` ranges come back truncated. `sleep_detail::segments` drops segments that come back twice, and `intraday::rows` turns the `series` object keyed by Unix time into rows.

Adding a paged v2 endpoint usually means one more `Command` variant that calls `series(service, action, list_key, args)`. `list_key` is the array in `body` to collect: `activities` for getactivity, `series` for the others.

## Withings API behavior that shapes the code

- Register apps on **Europe Cloud**; US Cloud requires a contract. Withings rejects a `localhost` callback because its servers fetch the callback URL to check it. In practice this means `withings auth --manual` with a public HTTPS redirect. Authorization codes expire after 30 seconds, so the code is exchanged right away.
- Access tokens last 3 hours. The refresh token is **rotated on every refresh**, so the new token must be saved (`fresh_tokens` does this).
- Rate limit is about one request per user every 10 minutes for polling (status 601). Use `--lastupdate` for incremental fetches.
- Data synced in from Apple Health (HealthKit) does **not** show up in the daily aggregate endpoints (`v2/sleep getsummary`, `v2/measure getactivity`). These return empty for it. The data is available from `v2/sleep get` (per-segment sleep) and `v2/measure getintradayactivity`. Request long `v2/sleep get` ranges one day at a time, because long ranges come back truncated.
