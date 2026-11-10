#!/usr/bin/env bats

load test_helper

setup() {
    export GH_REPO
    GH_REPO="$(python3 - "${ROOT}/.github/workflows/edge-build.yml" "${BATS_TEST_TMPDIR}/gate.sh" <<'PY'
from pathlib import Path
import sys
import yaml

workflow = yaml.safe_load(Path(sys.argv[1]).read_text())
gate = workflow["jobs"]["changed"]["steps"][0]
Path(sys.argv[2]).write_text(gate["run"])
print(gate["env"].get("GH_REPO", "").replace("${{ github.repository }}", "ndyer/zwift-snap"))
PY
)"
    mkdir "${BATS_TEST_TMPDIR}/bin"
    cat > "${BATS_TEST_TMPDIR}/bin/gh" <<'SH'
#!/bin/bash
set -euo pipefail
[[ ${GH_REPO} == ndyer/zwift-snap ]]
[[ $* == 'run list --workflow edge-build.yml --branch main --status success --limit 1 --json headSha --jq .[0].headSha // ""' ]]
touch "${BATS_TEST_TMPDIR}/gh-called"
printf '%s\n' "${LAST_SHA:-}"
exit "${GH_STATUS:-0}"
SH
    chmod +x "${BATS_TEST_TMPDIR}/bin/gh"
    export PATH="${BATS_TEST_TMPDIR}/bin:${PATH}"
    export FORCE=false SHA=current
    export GITHUB_OUTPUT="${BATS_TEST_TMPDIR}/output"
    # The runner has no checkout in this job.
    cd "${BATS_TEST_TMPDIR}" || return
}

@test "edge gate builds without a checkout when no successful run exists" {
    run bash "${BATS_TEST_TMPDIR}/gate.sh"
    assert_success
    assert_equal "$(cat "${GITHUB_OUTPUT}")" "build=true"
    assert_file_exists "${BATS_TEST_TMPDIR}/gh-called"
}

@test "edge gate skips a commit already built successfully" {
    run env LAST_SHA="${SHA}" bash "${BATS_TEST_TMPDIR}/gate.sh"
    assert_success
    assert_equal "$(cat "${GITHUB_OUTPUT}")" "build=false"
}

@test "edge gate builds when main has moved" {
    run env LAST_SHA=previous bash "${BATS_TEST_TMPDIR}/gate.sh"
    assert_success
    assert_equal "$(cat "${GITHUB_OUTPUT}")" "build=true"
}

@test "edge gate force bypasses the API query" {
    run env FORCE=true GH_STATUS=42 bash "${BATS_TEST_TMPDIR}/gate.sh"
    assert_success
    assert_equal "$(cat "${GITHUB_OUTPUT}")" "build=true"
    assert_file_not_exists "${BATS_TEST_TMPDIR}/gh-called"
}

@test "edge gate propagates API failures rather than deciding to build" {
    run env GH_STATUS=42 bash "${BATS_TEST_TMPDIR}/gate.sh"
    assert_failure 42
    assert_file_not_exists "${GITHUB_OUTPUT}"
}
