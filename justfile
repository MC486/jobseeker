# Common developer commands. `just` is optional; every recipe is a thin wrapper.

set dotenv-load := false

default:
    @just --list

fmt:
    cargo fmt --all

lint:
    cargo clippy --workspace --all-targets

test:
    cargo test --workspace

# Fast check used by CI before the full suite.
check:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets
    cargo test --workspace

web-install:
    cd web && npm install

web-dev:
    cd web && npm run dev

web-build:
    cd web && npm install && npm run build

serve:
    cargo run -p jobseeker-cli -- serve

add url:
    cargo run -p jobseeker-cli -- add {{url}}

reconcile:
    cargo run -p jobseeker-cli -- reconcile --check

# Dump OpenAPI (no server) and regenerate web/src/api/generated.ts.
# The generated file is committed (ADR-0009); CI typechecks it via `npm run build`.
gen-client:
    cargo run -q -p jobseeker-cli -- openapi --out web/openapi.json
    cd web && npx --yes openapi-typescript openapi.json -o src/api/generated.ts
