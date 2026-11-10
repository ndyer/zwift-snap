#!/usr/bin/env bats

# Tests for .workshop/zwift-snap-sdk/fetch-skills.sh, the script the setup-project hook runs
# to fetch the upstream Workshop skills. Fully offline: a local git fixture stands in for
# github.com/canonical/use-workshop-skill.

load test_helper

FETCH="${ROOT}/.workshop/zwift-snap-sdk/fetch-skills.sh"
SKILLS=(use-workshop onboard-workshop design-sdk)

setup() {
    UPSTREAM="${BATS_TEST_TMPDIR}/upstream"
    CACHE="${BATS_TEST_TMPDIR}/cache"
    TARGET="${BATS_TEST_TMPDIR}/skills"

    git init -q -b main "${UPSTREAM}"
    git -C "${UPSTREAM}" config user.email test@example.com
    git -C "${UPSTREAM}" config user.name test
    for skill in "${SKILLS[@]}"; do
        mkdir -p "${UPSTREAM}/.github/skills/${skill}/references"
        printf 'upstream %s\n' "${skill}" >"${UPSTREAM}/.github/skills/${skill}/SKILL.md"
        printf 'ref\n' >"${UPSTREAM}/.github/skills/${skill}/references/ref.md"
    done
    # An eval suite that must not be copied into the project.
    mkdir -p "${UPSTREAM}/.github/skills/use-workshop/tests"
    printf 'suite\n' >"${UPSTREAM}/.github/skills/use-workshop/tests/README.md"
    git -C "${UPSTREAM}" add -A
    git -C "${UPSTREAM}" commit -qm 'initial'
    git -C "${UPSTREAM}" tag v0.9.9

    # file:// forces git's real transport, so --depth is honoured and no hardlinks leak.
    export USE_WORKSHOP_SKILL_REPO="file://${UPSTREAM}"
    export USE_WORKSHOP_SKILL_REF=v0.9.9
    export USE_WORKSHOP_SKILL_CACHE="${CACHE}"
    export USE_WORKSHOP_SKILLS_DIR="${TARGET}"
}

@test "installs all three skills and omits the upstream test suites" {
    run "${FETCH}"
    assert_success

    for skill in "${SKILLS[@]}"; do
        assert_file_exists "${TARGET}/${skill}/SKILL.md"
        assert_file_exists "${TARGET}/${skill}/references/ref.md"
    done
    assert_dir_not_exists "${TARGET}/use-workshop/tests"
    assert_file_exists "${TARGET}/.fetched-revision"
    assert_file_contains "${TARGET}/.fetched-revision" "ref=v0.9.9"
    assert_file_contains "${TARGET}/.fetched-revision" "revision="
}

@test "is a no-op on a second run and needs no reachable upstream" {
    run "${FETCH}"
    assert_success
    rm -rf "${UPSTREAM}" # prove the second run does not touch the network

    run "${FETCH}"
    assert_success
    assert_output --partial "already at v0.9.9"
    assert_file_exists "${TARGET}/use-workshop/SKILL.md"
}

@test "reinstalls when the marker does not match the requested ref" {
    run "${FETCH}"
    assert_success
    printf 'upstream edit\n' >"${TARGET}/use-workshop/SKILL.md"
    printf 'ref=stale\n' >"${TARGET}/.fetched-revision"

    run "${FETCH}"
    assert_success
    run cat "${TARGET}/use-workshop/SKILL.md"
    assert_output "upstream use-workshop"
    assert_file_contains "${TARGET}/.fetched-revision" "ref=v0.9.9"
}

@test "warns and continues when the repository cannot be cloned" {
    export USE_WORKSHOP_SKILL_REPO="file://${BATS_TEST_TMPDIR}/does-not-exist"

    run "${FETCH}"
    assert_success
    assert_output --partial "could not clone"
    assert_file_not_exists "${TARGET}/use-workshop/SKILL.md"
}

@test "warns and continues when upstream lacks a skill" {
    rm -rf "${UPSTREAM}/.github/skills/design-sdk"
    git -C "${UPSTREAM}" commit -qam 'drop design-sdk'
    git -C "${UPSTREAM}" tag -f v0.9.9 >/dev/null

    run "${FETCH}"
    assert_success
    assert_output --partial "no .github/skills/design-sdk/SKILL.md"
    assert_file_not_exists "${TARGET}/use-workshop/SKILL.md"
}
