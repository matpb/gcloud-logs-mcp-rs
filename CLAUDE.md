# gcloud-logs-mcp-rs: read-only MCP server that lets AI coding tools query Google Cloud Logging

This codebase is yours: what it's for and how it's built are your call, and you'll keep building on it.

## Ground rules

1. **Stay read-only.** Tools only list and read Cloud Logging (`entries:list`, `logs.list`). Nothing may write, delete or modify logs or IAM. The IAM guidance assumes `roles/logging.viewer` alone.
2. **Keep secrets and real identifiers out of git.** The repo is public. `.env`, `credentials/` and all `*.json` files are gitignored; keep it that way. In tracked files use placeholders such as `my-project` and `/path/to/sa-key.json`, never a real project id, key, token or account name.
3. **Tool names, parameters and resource URIs are a client contract.** `list_projects`, `list_logs`, `query_logs`, `get_log_entry`, `list_resource_types` and `gcp-logs://<name>` are used by connected AI clients. Changing one breaks them; update the README tool section in the same change.
4. **Read the landmines before touching `src/main.rs`.** The transport and shutdown settings there were each changed for a reason.

## Commands

Run from the repo root. `--locked` makes cargo fail instead of rewriting `Cargo.lock`.

- Build (debug): `cargo build --locked`
- Build (release): `cargo build --release --locked` (LTO and one codegen unit, so it is slow)
- Typecheck: `cargo check --locked`
- Lint: `cargo clippy --locked`
- Full test suite: `cargo test --locked` (unit tests live in `#[cfg(test)]` modules; `src/logging.rs` has the first ones)
- Single test: `cargo test --locked truncates_multibyte_payload`
- Run: not listed. Startup needs real credentials and calls Google Cloud. Use the README Quick Start.

## Layout

- `src/main.rs`: axum routes, optional API key middleware on `/mcp` (`/health` stays open), stateless MCP service, graceful shutdown.
- `src/config.rs`: every env var the server reads, parsed in `Config::from_env`.
- `src/auth.rs`: one token provider per project. A project uses its `credentials_file` if set, otherwise Application Default Credentials.
- `src/logging.rs`: the Cloud Logging REST client, plus the filter builder, time-range parser and payload truncation. It is not server logging.
- `src/mcp/tools.rs`: tool parameter structs, tool bodies and the `ServerHandler` impl (tools and resources). `src/mcp/mod.rs` only re-exports it.
- In tool calls, `project` is the friendly `name` from `GCP_PROJECTS`, never the GCP project id.

## Conventions

- Bad config and failed credential checks panic at startup on purpose, so the server never runs half-configured. The README documents this.
- Runtime errors are `String`s mapped to `rmcp::ErrorData::internal_error`. Cloud Logging HTTP errors go through `map_api_error` in `logging.rs`.
- Tool results are JSON strings built with `serde_json::json!`, not typed structs. Field names are mixed (`executionTime` sits next to `has_more`), so copy the style of the neighbouring field.
- Logging uses `tracing`. The default filter is `gcloud_logs_mcp=info`; `RUST_LOG` overrides it.
- Rust 1.94 or later, edition 2024. Existing code is not rustfmt-clean, so do not reformat whole files; format only the lines you write.

## Landmines

- Don't move `/mcp` back to stateful sessions. Sessions were kept in memory forever because clients rarely send a DELETE, and the container needed a nightly restart to stay up. The fix is `NeverSessionManager` with stateful mode off and JSON responses on, as set in `main.rs`.
- Don't remove `disable_allowed_hosts()` from `main.rs`. Without it, rmcp answers 403 to requests that reach the server through a gateway carrying a public Host header. Network exposure and the API key are the protection instead.
- Keep the SIGTERM arm in the shutdown future. A container stop sends SIGTERM, and only that arm turns it into axum's graceful shutdown.
- An empty `API_KEY` means no authentication. Config treats `API_KEY=` as unset, `.env.example` ships it empty, and startup only logs a warning. The README Security section covers the gateway requirement.
- Every `*.json` is gitignored, so a JSON test fixture needs a forced add or a non-`.json` name.

## Deeper docs

- `README.md`: setup (service accounts, IAM role, `GCP_PROJECTS` format), env var table, tool parameters and examples, endpoints, security notes.
- `.env.example`: every env var the server reads, with comments. Copy the shape, not its values.
- `Makefile`: the build, run, dev and docker targets.
- `Dockerfile` and `docker-compose.yml`: the image copies only `Cargo.toml`, `Cargo.lock` and `src/`; credentials are mounted read-only at `/credentials`.
