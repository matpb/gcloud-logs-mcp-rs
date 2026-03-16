# gcloud-logs-mcp-rs

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.94+-orange.svg)](https://www.rust-lang.org/)
[![MCP](https://img.shields.io/badge/MCP-2025--03--26-green.svg)](https://modelcontextprotocol.io/)
[![Build](https://img.shields.io/badge/build-passing-brightgreen.svg)]()
[![Docker](https://img.shields.io/badge/Docker-ready-2496ED.svg)](https://www.docker.com/)

A lightweight Rust [MCP](https://modelcontextprotocol.io/) server for **read-only** access to [Google Cloud Logging](https://cloud.google.com/logging) with multi-project support.

Connect your AI tools (Claude Code, Cursor, Windsurf, etc.) to Google Cloud Logging via the Model Context Protocol. Query logs, filter by severity, resource type, and time range — all from your AI-powered IDE.

## Features

- **Multi-project** — connect to multiple GCP projects simultaneously, each with its own credentials
- **Cloud Logging filter syntax** — full support for the [Cloud Logging query language](https://cloud.google.com/logging/docs/view/logging-query-language)
- **Flexible authentication** — per-project service account keys, Application Default Credentials, or GCE metadata server
- **Smart time ranges** — use relative durations (`1h`, `30m`, `7d`), ISO timestamps, or start/end pairs
- **Severity filtering** — filter by minimum severity level (DEBUG through EMERGENCY)
- **Resource type discovery** — automatically discover what resource types are generating logs
- **Payload truncation** — large log entries are intelligently truncated to keep MCP responses manageable
- **MCP tools** — `list_projects`, `list_logs`, `query_logs`, `get_log_entry`, `list_resource_types`
- **MCP resources** — each project is exposed as a `gcp-logs://<name>` resource
- **Streamable HTTP transport** — serves MCP over HTTP at `/mcp`
- **Minimal footprint** — single static binary, ~10MB Docker image (distroless)

## Quick Start

### 1. Configure projects

Copy the example environment file and fill in your GCP project details:

```bash
cp .env.example .env
```

Edit `.env` with your project configuration:

```env
RUST_LOG=gcloud_logs_mcp=info

GCP_PROJECTS='[
  {
    "name": "my-project",
    "project_id": "my-gcp-project-id",
    "credentials_file": "/path/to/service-account-key.json"
  }
]'
```

Each project entry supports:

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `name` | yes | — | Friendly name used in MCP tool calls |
| `project_id` | yes | — | GCP project ID |
| `credentials_file` | no | ADC | Path to service account JSON key file |

If `credentials_file` is omitted, the server falls back to [Application Default Credentials](https://cloud.google.com/docs/authentication/application-default-credentials) (ADC), which covers:
- `GOOGLE_APPLICATION_CREDENTIALS` environment variable
- `gcloud auth application-default login`
- GCE/Cloud Run metadata server

### 2. Run with Docker (recommended)

```bash
# Place your service account key(s) in a credentials directory
mkdir -p credentials
cp /path/to/sa-key.json credentials/

docker compose up -d
```

### 3. Or build from source

```bash
cargo build --release
./target/release/gcloud-logs-mcp
```

The server starts on `http://0.0.0.0:8432` by default.

### 4. Connect to your AI tool

Add the server to your MCP client configuration. For Claude Code (`~/.claude.json`):

```json
{
  "mcpServers": {
    "gcloud-logs": {
      "type": "streamable-http",
      "url": "http://localhost:8432/mcp"
    }
  }
}
```

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `GCP_PROJECTS` | *required* | JSON array of project configs |
| `MCP_HOST` | `0.0.0.0` | Server bind address |
| `MCP_PORT` | `8432` | Server port |
| `DEFAULT_LOG_LIMIT` | `100` | Default max entries per query |
| `MAX_LOG_LIMIT` | `1000` | Hard cap on entries per query |
| `RUST_LOG` | `gcloud_logs_mcp=info` | Log level filter |

## MCP Tools

### `list_projects`

Lists all configured GCP projects and their project IDs.

### `list_logs`

Lists all available log names in a project (e.g. `syslog`, `cloudaudit.googleapis.com/activity`, `run.googleapis.com/stderr`).

- **Parameters:** `project` — the project name from your config

### `query_logs`

Queries log entries from Google Cloud Logging. This is the main workhorse tool.

- **Parameters:**

| Parameter | Required | Description |
|-----------|----------|-------------|
| `project` | yes | Project name from your config |
| `filter` | no | [Cloud Logging filter expression](https://cloud.google.com/logging/docs/view/logging-query-language) |
| `resource_type` | no | Filter by resource type (e.g. `gce_instance`, `cloud_run_revision`, `cloud_function`) |
| `severity` | no | Minimum severity: `DEFAULT`, `DEBUG`, `INFO`, `NOTICE`, `WARNING`, `ERROR`, `CRITICAL`, `ALERT`, `EMERGENCY` |
| `time_range` | no | Relative (`1h`, `30m`, `7d`, `2w`), ISO timestamp, or range (`start/end`) |
| `limit` | no | Max entries to return (default: 100, max: 1000) |
| `order_by` | no | `timestamp asc` or `timestamp desc` (default: `timestamp desc`) |

**Examples:**

```
# Recent errors in the last hour
query_logs(project="my-project", severity="ERROR", time_range="1h")

# Cloud Run logs with a text search
query_logs(project="my-project", resource_type="cloud_run_revision", filter='textPayload:"connection refused"')

# Specific time window
query_logs(project="my-project", time_range="2024-03-15T00:00:00Z/2024-03-16T00:00:00Z")

# Cloud SQL errors
query_logs(project="my-project", resource_type="cloudsql_database", severity="ERROR", time_range="24h")

# Complex filter
query_logs(project="my-project", filter='resource.type="cloud_run_revision" AND jsonPayload.status>=500')
```

### `get_log_entry`

Retrieves a specific log entry by its `insertId`.

- **Parameters:** `project`, `insert_id`

### `list_resource_types`

Discovers which resource types have generated log entries in the last hour. Useful for exploring what's available before writing a query.

- **Parameters:** `project`

## Authentication

The server supports three authentication methods, in order of precedence:

1. **Per-project service account key** — set `credentials_file` in the project config to a path to a service account JSON key file. Best for multi-project setups where each project uses different credentials.

2. **Application Default Credentials (ADC)** — if no `credentials_file` is set, the server uses ADC:
   - `GOOGLE_APPLICATION_CREDENTIALS` environment variable pointing to a key file
   - Credentials from `gcloud auth application-default login`
   - GCE/Cloud Run metadata server (automatic when running on GCP)

3. **Workload Identity** — when running on GKE or Cloud Run, credentials are provided automatically via the metadata server.

### Required IAM Permissions

The service account needs the **Logs Viewer** role (`roles/logging.viewer`) on each project, or the following individual permissions:

- `logging.logEntries.list`
- `logging.logs.list`

### Creating Service Account Credentials with `gcloud`

This walks you through creating a dedicated service account with the minimum permissions needed, using the Google Cloud CLI.

#### Prerequisites

- [Google Cloud CLI](https://cloud.google.com/sdk/docs/install) installed and authenticated
- `Owner` or `IAM Admin` role on the target GCP project(s)

Verify your access:

```bash
gcloud auth list
gcloud projects list
```

#### Step 1: Create a service account

Create a dedicated service account for the MCP server in each project you want to query. Use a descriptive name so it's easy to identify later.

```bash
gcloud iam service-accounts create gcloud-logs-mcp \
  --project=YOUR_PROJECT_ID \
  --display-name="GCloud Logs MCP Server" \
  --description="Read-only access to Cloud Logging for the gcloud-logs-mcp MCP server"
```

Repeat for each project:

```bash
# Example: dev project
gcloud iam service-accounts create gcloud-logs-mcp \
  --project=my-project-dev \
  --display-name="GCloud Logs MCP Server" \
  --description="Read-only access to Cloud Logging for the gcloud-logs-mcp MCP server"

# Example: prod project
gcloud iam service-accounts create gcloud-logs-mcp \
  --project=my-project-prod \
  --display-name="GCloud Logs MCP Server" \
  --description="Read-only access to Cloud Logging for the gcloud-logs-mcp MCP server"
```

#### Step 2: Grant the Logs Viewer role

Bind the `roles/logging.viewer` role to the service account on each project. This is the **minimum permission** needed — it grants read-only access to log entries and log names, nothing else.

```bash
gcloud projects add-iam-policy-binding YOUR_PROJECT_ID \
  --member="serviceAccount:gcloud-logs-mcp@YOUR_PROJECT_ID.iam.gserviceaccount.com" \
  --role="roles/logging.viewer" \
  --condition=None \
  --quiet
```

Repeat for each project:

```bash
# Dev
gcloud projects add-iam-policy-binding my-project-dev \
  --member="serviceAccount:gcloud-logs-mcp@my-project-dev.iam.gserviceaccount.com" \
  --role="roles/logging.viewer" \
  --condition=None \
  --quiet

# Prod
gcloud projects add-iam-policy-binding my-project-prod \
  --member="serviceAccount:gcloud-logs-mcp@my-project-prod.iam.gserviceaccount.com" \
  --role="roles/logging.viewer" \
  --condition=None \
  --quiet
```

#### Step 3: Generate JSON key files

Create a JSON key file for each service account. These files are the credentials the MCP server uses to authenticate.

```bash
mkdir -p credentials

# Dev key
gcloud iam service-accounts keys create credentials/dev-sa.json \
  --iam-account=gcloud-logs-mcp@my-project-dev.iam.gserviceaccount.com \
  --project=my-project-dev

# Prod key
gcloud iam service-accounts keys create credentials/prod-sa.json \
  --iam-account=gcloud-logs-mcp@my-project-prod.iam.gserviceaccount.com \
  --project=my-project-prod
```

> **Security note:** These JSON key files contain private keys. Never commit them to version control. The `.gitignore` in this repo already excludes the `credentials/` directory and `*.json` files.

#### Step 4: Configure the server

Reference the key files in your `.env`:

```env
GCP_PROJECTS='[
  {
    "name": "dev",
    "project_id": "my-project-dev",
    "credentials_file": "credentials/dev-sa.json"
  },
  {
    "name": "prod",
    "project_id": "my-project-prod",
    "credentials_file": "credentials/prod-sa.json"
  }
]'
```

When running in Docker, the paths should reference the mounted volume (e.g. `/credentials/dev-sa.json`).

#### Step 5: Verify

Start the server and check that credentials are validated at startup:

```bash
cargo run
```

You should see:

```
INFO gcloud_logs_mcp: Connecting to 2 GCP project(s): ["dev", "prod"]
INFO gcloud_logs_mcp::auth: Loading credentials from file project=dev
INFO gcloud_logs_mcp::auth: Loading credentials from file project=prod
INFO gcloud_logs_mcp: All GCP credentials validated
INFO gcloud_logs_mcp: GCloud Logs MCP server listening on 0.0.0.0:8432
```

If credentials are invalid or the service account lacks permissions, the server will panic at startup with a clear error message.

#### Cleanup: Revoking access

To revoke a service account's access or delete it entirely:

```bash
# Remove the role binding
gcloud projects remove-iam-policy-binding YOUR_PROJECT_ID \
  --member="serviceAccount:gcloud-logs-mcp@YOUR_PROJECT_ID.iam.gserviceaccount.com" \
  --role="roles/logging.viewer" \
  --quiet

# Delete the service account (also invalidates all keys)
gcloud iam service-accounts delete \
  gcloud-logs-mcp@YOUR_PROJECT_ID.iam.gserviceaccount.com \
  --project=YOUR_PROJECT_ID \
  --quiet
```

To list and revoke individual keys without deleting the service account:

```bash
# List keys
gcloud iam service-accounts keys list \
  --iam-account=gcloud-logs-mcp@YOUR_PROJECT_ID.iam.gserviceaccount.com

# Delete a specific key
gcloud iam service-accounts keys delete KEY_ID \
  --iam-account=gcloud-logs-mcp@YOUR_PROJECT_ID.iam.gserviceaccount.com \
  --quiet
```

## Docker

### Build

```bash
docker build -t gcloud-logs-mcp .
```

The Dockerfile uses a two-stage build:
- **Builder:** `rust:1.94-alpine` — compiles the binary with musl for a static build
- **Runtime:** `gcr.io/distroless/cc-debian12` — minimal image with CA certificates for HTTPS

### Run

```bash
docker run -d \
  -p 8432:8432 \
  -v /path/to/credentials:/credentials:ro \
  -e GCP_PROJECTS='[{"name":"my-project","project_id":"my-gcp-id","credentials_file":"/credentials/sa-key.json"}]' \
  gcloud-logs-mcp
```

### Docker Compose

```bash
docker compose up -d
```

The `docker-compose.yml` mounts a `credentials/` directory as a read-only volume. Place your service account key files there.

## Multi-Project Setup

Configure multiple GCP projects with independent credentials:

```env
GCP_PROJECTS='[
  {
    "name": "dev",
    "project_id": "my-project-dev",
    "credentials_file": "/credentials/dev-sa.json"
  },
  {
    "name": "staging",
    "project_id": "my-project-staging",
    "credentials_file": "/credentials/staging-sa.json"
  },
  {
    "name": "prod",
    "project_id": "my-project-prod",
    "credentials_file": "/credentials/prod-sa.json"
  }
]'
```

Then query any project by its friendly name:

```
query_logs(project="prod", severity="ERROR", time_range="1h")
```

## Endpoints

| Path | Description |
|------|-------------|
| `GET /health` | Health check (returns `ok`) |
| `/mcp` | MCP streamable HTTP endpoint |

## Security

- **Read-only access** — the server only queries logs, it never writes or deletes anything
- **Credential isolation** — each project can use its own service account with least-privilege permissions
- **No built-in auth** — run behind a firewall, VPN, or API gateway in production
- **Credential safety** — service account file paths are redacted in debug output

**Recommendations for production use:**

- Use service accounts with **only** `roles/logging.viewer`
- Run the server behind a **firewall or API gateway** — there is no built-in authentication
- Mount credentials as **read-only volumes** in Docker
- Set `RUST_LOG=gcloud_logs_mcp=warn` to minimize log output

## Development

```bash
# Build
cargo build

# Run (with ADC)
GCP_PROJECTS='[{"name":"my-project","project_id":"my-gcp-id"}]' cargo run

# Run with debug logging
RUST_LOG=gcloud_logs_mcp=debug GCP_PROJECTS='[...]' cargo run

# Release build (optimized for size)
cargo build --release
```

## Architecture

```
src/
├── main.rs          # HTTP server, MCP service setup, graceful shutdown
├── config.rs        # Environment variable parsing, multi-project config
├── auth.rs          # GCP authentication (per-project token providers)
├── logging.rs       # Cloud Logging REST API client, filter builder, time parsing
└── mcp/
    ├── mod.rs
    └── tools.rs     # MCP tool definitions, parameter types, ServerHandler impl
```

The server uses:
- [axum](https://github.com/tokio-rs/axum) for HTTP
- [rmcp](https://github.com/anthropics/rmcp) for MCP protocol
- [gcp_auth](https://github.com/hrvolapeter/gcp_auth) for GCP authentication
- [reqwest](https://github.com/seanmonstar/reqwest) for Cloud Logging REST API calls

## License

[MIT](LICENSE)
