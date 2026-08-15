#!/usr/bin/env bash
# Brings up the docker-compose services this repo needs for testing, waits
# for them to actually be ready (docker-compose.yml defines no healthchecks,
# so this script owns that), runs migrations, then the full Rust + frontend
# test suites. Tears services down on exit regardless of pass/fail.
#
# Usage:
#   scripts/run-tests.sh                 # postgres + redis only (default)
#   scripts/run-tests.sh --with-validator  # also runs the one Solana
#                                           # e2e suite against a local
#                                           # solana-test-validator
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

WITH_VALIDATOR=false
if [[ "${1:-}" == "--with-validator" ]]; then
  WITH_VALIDATOR=true
fi

export TEST_DATABASE_URL="${TEST_DATABASE_URL:-postgres://postgres:password@localhost:5432/perp_exchange}"
export DATABASE_URL="$TEST_DATABASE_URL"

COMPOSE_SERVICES="postgres redis"
if $WITH_VALIDATOR; then
  COMPOSE_SERVICES="$COMPOSE_SERVICES solana-test-validator"
fi

echo "==> starting: $COMPOSE_SERVICES"
docker compose up -d $COMPOSE_SERVICES

cleanup() {
  echo "==> tearing down docker compose services"
  docker compose down
}
trap cleanup EXIT

wait_for() {
  local desc="$1"
  shift
  echo -n "==> waiting for $desc"
  for _ in $(seq 1 60); do
    if "$@" >/dev/null 2>&1; then
      echo " ok"
      return 0
    fi
    echo -n "."
    sleep 2
  done
  echo " TIMEOUT"
  echo "error: $desc never became ready" >&2
  exit 1
}

wait_for "postgres" docker compose exec -T postgres pg_isready -U postgres
wait_for "redis" docker compose exec -T redis redis-cli ping

if $WITH_VALIDATOR; then
  wait_for "solana-test-validator" curl -sf -X POST -H 'content-type: application/json' \
    -d '{"jsonrpc":"2.0","id":1,"method":"getHealth"}' http://127.0.0.1:8899
fi

echo "==> running migrations"
if ! command -v sqlx >/dev/null 2>&1; then
  cargo install sqlx-cli --version "^0.8" --no-default-features --features postgres,rustls
fi
sqlx migrate run --source migrations

echo "==> cargo test --workspace (colocated unit tests)"
cargo test --workspace --exclude perp-integration-tests

echo "==> cargo test -p perp-integration-tests (integration tests, validator suite excluded by default)"
cargo test -p perp-integration-tests

if $WITH_VALIDATOR; then
  echo "==> cargo test -p perp-integration-tests -- --ignored (validator e2e suite)"
  cargo test -p perp-integration-tests -- --ignored worker_deposit_withdrawal_e2e
fi

echo "==> frontend tests"
(cd frontend && npm run test)

echo "==> all test suites passed"
