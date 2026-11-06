# Shared helpers for the packaging and test-tool unit tests.
# shellcheck disable=SC1091
load '/usr/lib/bats/bats-support/load'
# shellcheck disable=SC1091
load '/usr/lib/bats/bats-assert/load'
# shellcheck disable=SC1091
load '/usr/lib/bats/bats-file/load'

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
export ROOT
