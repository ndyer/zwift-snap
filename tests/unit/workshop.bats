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

# Load the real install action into an isolated project; never run the real sudo.
prepare_install_action() {
    python3 - "${ROOT}/.workshop/zwift-snap.yaml" "${PROJECT}" <<'PY'
from pathlib import Path
import sys
import yaml

source, project = sys.argv[1:]
action = yaml.safe_load(Path(source).read_text())["actions"]["install"]
action = action.replace("cd /project\n", "", 1)
Path(project, ".workshop/zwift-snap.yaml").write_text(
    yaml.safe_dump({"actions": {"install": action}})
)
PY
    mkdir -p "${PROJECT}/bin"
    cat > "${PROJECT}/bin/sudo" <<'SH'
#!/bin/bash
printf '%s\n' "$@" > "${INSTALL_ARGS}"
exit "${INSTALL_STATUS:-0}"
SH
    chmod +x "${PROJECT}/bin/sudo"
    export PATH="${PROJECT}/bin:${PATH}"
    export INSTALL_ARGS="${PROJECT}/install-args"
}

@test "install selects only the newest snap by modification time and forwards flags" {
    prepare_install_action
    touch -t 202001010000 "${PROJECT}/zwift_9.0_amd64.snap"
    touch -t 202101010000 "${PROJECT}/zwift_0.13 dirty_amd64.snap"
    touch -t 202201010000 "${PROJECT}/zwift_99_arm64.snap"
    mkdir "${PROJECT}/zwift_directory_amd64.snap"
    run "${RUNNER}" run install --devmode "two words"
    assert_success
    assert_output "Installing ./zwift_0.13 dirty_amd64.snap"
    assert_equal "$(cat "${INSTALL_ARGS}")" "$(printf '%s\n' snap install --dangerous --devmode 'two words' './zwift_0.13 dirty_amd64.snap')"
}

@test "install handles a single snap and preserves installer failure status" {
    prepare_install_action
    touch "${PROJECT}/zwift_0.13_amd64.snap"
    export INSTALL_STATUS=42
    run "${RUNNER}" run install
    assert_failure 42
    assert_equal "$(cat "${INSTALL_ARGS}")" "$(printf '%s\n' snap install --dangerous ./zwift_0.13_amd64.snap)"
}

@test "install fails clearly without invoking sudo when no snap exists" {
    prepare_install_action
    run "${RUNNER}" run install
    assert_failure
    assert_output --partial "No Zwift snap found"
    assert_file_not_exists "${INSTALL_ARGS}"
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
        assert_output --partial "not the host Workshop CLI"
        assert_output --partial "can not be run from within the workshop"
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
