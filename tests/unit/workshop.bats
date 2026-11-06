#!/usr/bin/env bats

load test_helper

setup() {
    PROJECT="${BATS_TEST_TMPDIR}/project"
    mkdir -p "${PROJECT}/.workshop/zwift-snap-sdk"
    cp "${ROOT}/.workshop/zwift-snap-sdk/workshop" "${PROJECT}/.workshop/zwift-snap-sdk/workshop"
    RUNNER="${PROJECT}/.workshop/zwift-snap-sdk/workshop"
    cat > "${PROJECT}/.workshop/zwift-snap.yaml" <<'YAML'
name: zwift-snap
actions:
  inspect: |
    printf '%s\n' "$PWD" "$#" "$@"
  fail: |
    exit 42
  errexit: |
    false
    echo should-not-run
  pipefail: |
    false | true
    echo should-not-run
YAML
}

@test "workshop runs from the project and forwards arguments literally" {
    run "${RUNNER}" run inspect "two words" "" "\$(false)" --flag
    assert_success
    assert_output "$(printf '%s\n' "${PROJECT}" 4 "two words" "" "\$(false)" --flag)"
}

@test "workshop accepts an explicit workshop name" {
    run "${RUNNER}" run zwift-snap inspect value
    assert_success
    assert_output "$(printf '%s\n' "${PROJECT}" 1 value)"
}

@test "workshop preserves the action exit status" {
    run "${RUNNER}" run fail
    assert_failure 42
}

@test "workshop enables errexit and pipefail" {
    run "${RUNNER}" run errexit
    assert_failure
    refute_output --partial should-not-run
    run "${RUNNER}" run pipefail
    assert_failure
    refute_output --partial should-not-run
}

@test "workshop lists actions and rejects invalid commands" {
    run "${RUNNER}" --help
    assert_success
    assert_output --partial "inspect, fail, errexit, pipefail"
    run "${RUNNER}" run missing
    assert_failure
    assert_output --partial "unknown action"
    run "${RUNNER}" run
    assert_failure
    assert_output --partial "missing action"
    run "${RUNNER}" refresh
    assert_failure
    assert_output --partial "only 'run' is supported"
}

@test "unsupported workshop commands explain the environment even without a definition" {
    rm "${PROJECT}/.workshop/zwift-snap.yaml"
    local command
    for command in refresh info changes tasks exec shell --version; do
        run "${RUNNER}" "${command}"
        assert_failure
        assert_output --partial "already inside the zwift-snap workshop"
        assert_output --partial "workshop run <action>"
        assert_output --partial "workshop --help"
        assert_output --partial "Run shell commands directly in /project"
        assert_output --partial "can only be run from the host by the user"
        assert_output --partial "not from within the workshop"
    done
}

@test "workshop reads definition changes without reinstalling" {
    printf '\n  added: echo new-action\n' >> "${PROJECT}/.workshop/zwift-snap.yaml"
    run "${RUNNER}" run added
    assert_success
    assert_output new-action
}

@test "workshop reports missing or invalid definitions" {
    printf 'actions: [not-a-mapping]\n' > "${PROJECT}/.workshop/zwift-snap.yaml"
    run "${RUNNER}" run inspect
    assert_failure
    assert_output --partial "actions must map names to shell scripts"
    printf 'actions: [\n' > "${PROJECT}/.workshop/zwift-snap.yaml"
    run "${RUNNER}" run inspect
    assert_failure
    assert_output --partial "workshop:"
    rm "${PROJECT}/.workshop/zwift-snap.yaml"
    run "${RUNNER}" run inspect
    assert_failure
    assert_output --partial "No such file"
}
