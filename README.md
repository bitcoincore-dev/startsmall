# [startsmall](https://bitcoincore-dev.github.io/startsmall/)

[![Rust](https://github.com/bitcoincore-dev/startsmall/actions/workflows/rust.yml/badge.svg)](https://github.com/bitcoincore-dev/startsmall/actions/workflows/rust.yml)

A Rust tool that snapshots a Google Sheet into a git repository and optionally serves it as a web page. Each sync commit records the CSV content, a SHA-256 hash, and a nanosecond timestamp. Snapshots can also be announced on [Nostr](https://nostr.com/) (plain text note + NIP-34 repository announcement).

A live deployment syncs every minute via GitHub Actions and is published to GitHub Pages.

---

## Table of Contents

- [Quick start](#quick-start)
- [CLI interface](#cli-interface)
  - [Modes](#modes)
  - [Flags](#flags)
  - [Environment variables](#environment-variables)
- [HTTP / web interface](#http--web-interface)
  - [Endpoints](#endpoints)
  - [Authorization](#authorization)
- [Git snapshot format](#git-snapshot-format)
- [Nostr integration](#nostr-integration)
- [GitHub Actions workflows](#github-actions-workflows)
- [Building](#building)

---

## Quick start

```sh
# Serve the default sheet on a random local port
cargo run

# Sync (snapshot) the default sheet once, then exit
cargo run -- sync

# Serve a custom sheet on a fixed address
cargo run -- --sheet-url "https://..." --bind-addr 127.0.0.1:8080 serve

# Sync and publish a Nostr note
cargo run -- --privkey nsec1... sync
```

---

## CLI interface

```
startsmall [OPTIONS] [MODE]
```

### Modes

| Mode | Description |
|------|-------------|
| `serve` *(default)* | Start the HTTP server and keep it running. On every `GET /` request the sheet is fetched live and rendered as HTML. `POST /sync` takes a snapshot and commits it to git. |
| `sync` | Fetch the sheet once, commit a snapshot to git, optionally publish to Nostr, then exit. Suitable for cron jobs and CI. |

### Flags

| Flag | Value | Default | Description |
|------|-------|---------|-------------|
| `--sheet-url <URL>` | Google Sheets CSV export URL | `SHEET_CSV_URL` env var, or the bundled StartSmall sheet | The URL that returns the sheet as CSV. Use the **export as CSV** link from Google Sheets (`…/export?format=csv&gid=0`). |
| `--bind-addr <ADDR>` | `host:port` | `BIND_ADDR` env var, or `127.0.0.1:0` (random port) | Address the HTTP server listens on. Only used in `serve` mode. |
| `--privkey <NSEC_OR_HEX>` | Nostr private key | *(none – Nostr disabled)* | Private key used to sign and publish Nostr events on sync. Accepts `nsec1…` bech32 or 64-character hex. |
| `--version` | | | Print version and exit. |
| `--help` | | | Print help and exit. |

### Environment variables

All flags can alternatively be configured through environment variables. CLI flags take precedence.

| Variable | Equivalent flag | Description |
|----------|-----------------|-------------|
| `SHEET_CSV_URL` | `--sheet-url` | Google Sheets CSV export URL. |
| `BIND_ADDR` | `--bind-addr` | Listen address for the HTTP server. |
| `SYNC_TOKEN` | *(no flag equivalent)* | Secret token required to authorize `POST /sync` from non-loopback addresses. See [Authorization](#authorization). |
| `NOSTR_RELAYS` | *(no flag equivalent)* | Comma-separated list of Nostr relay WebSocket URLs. Defaults to `wss://relay.damus.io`, `wss://relay.nostr.band`, `wss://nostr.wine`. |

---

## HTTP / web interface

Start the server with `cargo run` (or `cargo run -- serve`) and open the printed URL in a browser.

```
Serving spreadsheet viewer at http://127.0.0.1:PORT
```

### Endpoints

#### `GET /`

Fetches the Google Sheet CSV live, parses it, and returns an HTML page.

**Response** `200 OK` – HTML table with:
- A **metadata section** for any leading rows that appear before the column-header row.
- A **header row** detected automatically (a row whose cells include keywords such as `date`, `amount`, `grantee`, `twitter`, `link`, etc.).
- **Data rows** rendered below the header. Each row is numbered.
- Cells that look like URLs, domains, or Twitter/X handles are rendered as clickable links.
- A **Sync snapshot to git** button that posts to `/sync`.

**Response** `200 OK` (error page) – If the sheet cannot be fetched, a plain error message is rendered instead of a 5xx, so the browser always receives a readable page.

#### `POST /sync`

Fetches the sheet, writes `sheet-snapshots/google-sheet.csv` and `sheet-snapshots/google-sheet.meta`, stages both files, and creates a git commit in the current repository. If a Nostr private key is configured, a text note and a NIP-34 repository announcement are published.

**Response** `200 OK` – HTML confirmation page showing the commit hash, SHA-256, row count, and optional Nostr event IDs.

**Response** `403 Forbidden` – The request was not authorized. See [Authorization](#authorization).

**Response** `503 Service Unavailable` – The server is already handling the maximum number of concurrent requests (16).

### Authorization

`POST /sync` is automatically authorized when the request originates from a **loopback address** (`127.0.0.1` / `::1`). This covers the browser button when the server is running locally.

For remote clients (e.g., a GitHub Actions webhook), set the `SYNC_TOKEN` environment variable to a secret string. The client must then supply the token in one of two ways:

```http
X-Sync-Token: <token>
```

```http
Authorization: Bearer <token>
```

Token comparison is performed with a constant-time algorithm (via the `subtle` crate) to avoid timing-based attacks.

If `SYNC_TOKEN` is not set, remote `POST /sync` requests are always rejected with `403 Forbidden`.

---

## Git snapshot format

Each sync writes two files and creates a commit:

### `sheet-snapshots/google-sheet.csv`

The raw CSV body exactly as returned by the Google Sheets export URL.

### `sheet-snapshots/google-sheet.meta`

A newline-delimited key=value file:

```
source_url=<csv-export-url>
sha256=<hex-encoded SHA-256 of the CSV body>
rows=<number of CSV rows>
synced_at_unix_ns=<Unix timestamp in nanoseconds>
```

### Commit message format

```
sync: snapshot Google Sheet document

sha256: <hex>
rows: <count>
source: <url>
file: sheet-snapshots/google-sheet.csv
meta: sheet-snapshots/google-sheet.meta
synced_at_unix_ns: <nanoseconds>
```

The commit author/committer is read from the local git config. If no user is configured, the fallback identity `Start Small Bot <start-small-bot@example.com>` is used.

---

## Nostr integration

Passing `--privkey` (or equivalent) enables two Nostr publications on every successful sync.

### Text note (kind 1)

A short human-readable note is broadcast:

```
StartSmall snapshot synced
commit: <git-commit-sha>
sha256: <csv-sha256>
rows: <count>
source: <csv-export-url>
```

### NIP-34 repository announcement (kind 30617)

A [NIP-34](https://github.com/nostr-protocol/nips/blob/master/34.md) `git_repository_announcement` event is published with:

- **id**: `startsmall`
- **name**: `StartSmall`
- **description**: `Google Sheet snapshot viewer and git sync`
- **web**: read from a `CNAME` file in the repository root (if present)
- **clone**: derived from the `origin` remote URL (SSH `git@github.com:` and HTTPS forms are both normalised)
- **relays**: the configured relay list

### Relay configuration

Default relays:
- `wss://relay.damus.io`
- `wss://relay.nostr.band`
- `wss://nostr.wine`

Override with `NOSTR_RELAYS=wss://my-relay.example.com,wss://other.example.com`.

---

## GitHub Actions workflows

### `rust.yml` – CI

Runs on every push and pull request to `main`, and on a cron schedule every minute.

| Step | Command |
|------|---------|
| Build | `cargo build --verbose` |
| Test | `cargo test --verbose` |

### `startsmall.yml` – Deploy StartSmall to Pages

Triggered on pushes to `main`, every minute via cron, and manually via `workflow_dispatch`.

**`sync` job** (runs on `schedule` / `workflow_dispatch` only)

1. Checks out the full history.
2. Configures git identity (`StartSmall Bot`).
3. Runs `cargo run -- sync` with `SHEET_CSV_URL` set.
4. Pushes the resulting snapshot commit back to the branch.

**`build` job** (runs on `push` to `main` only)

1. Generates a static `_site/` directory containing:
   - `index.html` – plain HTML table built from the latest `google-sheet.csv`.
   - `google-sheet.csv` – the raw CSV.
   - `CNAME` – the custom domain file (if present).
   - `.nojekyll` – disables Jekyll processing.
2. Uploads the artifact for Pages deployment.

**`deploy` job**

Deploys the uploaded artifact to GitHub Pages.

---

## Building

**Prerequisites**: Rust toolchain (stable). Install via [rustup](https://rustup.rs/).

```sh
# Debug build
cargo build

# Release build
cargo build --release

# Run tests
cargo test

# Run with release optimisations
cargo run --release -- serve
```

The compiled binary is `target/release/startsmall` (or `target/debug/startsmall` for debug builds).
