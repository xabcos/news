APP := news
IMAGE ?= proxyxai/news
VERSION ?= rust

.PHONY: check test fmt fmt-check clippy audit js-check compose-check release-check run docker-build docker-up-local docker-down

check:
	cargo check --locked --all-targets

test:
	cargo test --locked --all-targets

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

clippy:
	cargo clippy --locked --all-targets -- -D warnings

# Not part of release-check: CI runs `make release-check` on a runner without cargo-audit.
# Run this locally, or install cargo-audit in the workflow before wiring it in.
audit:
	# SQLx records its optional MySQL driver in Cargo.lock; rsa is not in this project's build tree.
	cargo audit --ignore RUSTSEC-2023-0071

# One file per invocation: node only syntax-checks the first path it is given.
js-check:
	node --check pages/app.js
	node --check pages/admin/app.js
	node --check pages/me/app.js

compose-check:
	docker compose config >/dev/null
	docker compose -f docker-compose.yml -f docker-compose.local.yml config >/dev/null

release-check: fmt-check test clippy js-check compose-check
	@echo "OK: $(APP) release checks passed."

run:
	cargo run

docker-build:
	docker build -t $(IMAGE):$(VERSION) .

docker-up-local:
	docker compose -f docker-compose.yml -f docker-compose.local.yml up -d --build

docker-down:
	docker compose down
