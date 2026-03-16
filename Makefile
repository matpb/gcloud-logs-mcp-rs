ENV_FILE ?= .env
MCP_PORT ?= 8432

.PHONY: run dev build release docker docker-up docker-down clean health

# Run in release mode
run: build
	set -a && . ./$(ENV_FILE) && set +a && ./target/release/gcloud-logs-mcp

# Run in dev mode with debug logging
dev:
	set -a && . ./$(ENV_FILE) && set +a && RUST_LOG=gcloud_logs_mcp=debug cargo run

# Build debug binary
build:
	cargo build --release

# Build optimized release binary
release:
	cargo build --release

# Build Docker image
docker:
	docker build -t gcloud-logs-mcp .

# Start with Docker Compose
docker-up:
	docker compose up -d

# Stop Docker Compose
docker-down:
	docker compose down

# Health check
health:
	@curl -sf http://localhost:$(MCP_PORT)/health && echo " ok" || echo " FAILED"

# Remove build artifacts
clean:
	cargo clean
