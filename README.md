# withings-cli

A small command-line client for the [Withings API](https://developer.withings.com/):
body measurements (weight, body fat, blood pressure, ...), daily activity,
sleep summaries, workouts, and any other call through `withings raw`.

Output is JSON by default (`-f jsonl` or `-f table` for the others), so it
composes with `jq`, cron jobs and agents.

## Setup

1. Create an application on the [Withings developer dashboard](https://developer.withings.com/dashboard/)
   (choose **Europe Cloud**: the Public API, no contract needed; US Cloud is
   contract-only). Withings checks that the callback URL is reachable from
   its servers, so `localhost` is rejected. Register any public HTTPS page you
   control, e.g. your GitHub Pages site:

   ```
   https://<you>.github.io/
   ```

   The page is never read by this tool; after consent the browser just lands
   there with `?code=...&state=...` in the address bar.

2. Put its credentials in `~/.config/withings/client.env` (`chmod 600` it):

   ```sh
   WITHINGS_CLIENT_ID=...
   WITHINGS_CLIENT_SECRET=...
   WITHINGS_REDIRECT_URI=https://<you>.github.io/   # the callback URL you registered
   ```

   Environment variables with the same names take precedence over the file.

3. Build and log in once:

   ```sh
   cargo install --path .     # installs ~/.cargo/bin/withings
   withings auth --manual     # prints a URL; open it, allow access, paste the URL you land on
   ```

   Paste within 30 seconds: that is how long the authorization code lives.
   Without `--manual`, `withings auth` listens on the redirect URI's port and
   catches the redirect itself, which only works for an `http://localhost`
   redirect (for example behind your own reachable proxy).

Tokens are stored in `~/.config/withings/tokens.json` (mode 600) and refreshed
automatically. Access tokens last 3 hours; the refresh token lasts a year and
is rotated on every refresh.

## Usage

```sh
withings measure                                   # weight, all history
withings measure --types weight,fat_ratio,muscle_mass --since 2026-10-01 -f table
withings measure --all --lastupdate 1794870000     # every type, changed since a Unix time
withings activity --since 2026-11-01 -f table      # steps, distance, calories per day
withings sleep --since 2026-11-01
withings workouts --since 2026-11-01
withings raw v2/sleep getsummary startdateymd=2026-11-01 data_fields=total_sleep_time
```

- `--since` / `--until` are local dates; `--until` is inclusive.
- `measure` returns one object per measure group with named values, e.g.
  `{"date":"2026-11-17T08:00:00+09:00","grpid":1,"category":"real","fat_ratio":18.5,"weight":68.45}`.
  Names follow the meastype table in the [OpenAPI spec](https://developer.withings.com/openapi.yaml)
  (weight, fat_ratio, fat_mass, muscle_mass, bone_mass, hydration,
  visceral_fat, bmr, metabolic_age, heart_pulse, ...). Unlisted types appear
  as `type_<id>`; `--types` also accepts ids.
- `activity`, `sleep` and `workouts` print the API's items as they are,
  following pages. These services need both dates, so a missing `--until`
  is today and a missing `--since` covers 7 days. `--fields` sets `data_fields`.
- `raw` prints the `body` of any call to `https://wbsapi.withings.net/<service>`.
- The API allows about one request per user every 10 minutes for polling;
  use `--lastupdate` for incremental fetches.

## Development

```sh
cargo test
cargo clippy --all-targets
```
