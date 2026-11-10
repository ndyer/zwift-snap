#!/usr/bin/env bats

# Tests for .workshop/zwift-snap-sdk/fetch-wine.sh, the script the setup-project hook runs
# to clone the patched Wine source the snap's wine part builds from. Fully offline: a local
# git fixture stands in for github.com/ndyer/wine.

load test_helper

FETCH="${ROOT}/.workshop/zwift-snap-sdk/fetch-wine.sh"

setup() {
    UPSTREAM="${BATS_TEST_TMPDIR}/upstream"
    TARGET="${BATS_TEST_TMPDIR}/wine-src"

    git init -q -b zwift-radios-rebased "${UPSTREAM}"
    git -C "${UPSTREAM}" config user.email test@example.com
    git -C "${UPSTREAM}" config user.name test
    # ./configure is what fetch-wine.sh uses to recognise a Wine checkout.
    printf '#!/bin/sh\n' >"${UPSTREAM}/configure"
    printf 'wine-11.21\n' >"${UPSTREAM}/VERSION"
    git -C "${UPSTREAM}" add -A
    git -C "${UPSTREAM}" commit -qm 'initial'

    # file:// forces git's real transport, so --depth is honoured and no hardlinks leak.
    export ZWIFT_WINE_REPO="file://${UPSTREAM}"
    export ZWIFT_WINE_REF=zwift-radios-rebased
    export ZWIFT_WINE_SRC="${TARGET}"
}

@test "clones the requested ref into the target" {
    run "${FETCH}"
    assert_success

    assert_file_exists "${TARGET}/configure"
    assert_file_exists "${TARGET}/VERSION"
    assert_dir_exists "${TARGET}/.git"
    assert_output --partial "cloned file://${UPSTREAM} at zwift-radios-rebased"
}

@test "leaves an existing target untouched and needs no reachable upstream" {
    mkdir -p "${TARGET}"
    printf 'local patch\n' >"${TARGET}/local.patch"
    rm -rf "${UPSTREAM}" # prove the second run does not touch the network

    run "${FETCH}"
    assert_success
    assert_output --partial "already exists"

    assert_file_exists "${TARGET}/local.patch"
    assert_dir_not_exists "${TARGET}/.git"
}

@test "fails when the repository cannot be cloned" {
    export ZWIFT_WINE_REPO="file://${BATS_TEST_TMPDIR}/does-not-exist"

    run "${FETCH}"
    assert_failure
    assert_output --partial "could not clone"
    assert_dir_not_exists "${TARGET}"
}

@test "fails when the checkout does not look like Wine and leaves nothing behind" {
    rm "${UPSTREAM}/configure"
    git -C "${UPSTREAM}" add -A
    git -C "${UPSTREAM}" commit -qm 'drop configure'

    run "${FETCH}"
    assert_failure
    assert_output --partial "does not look like a Wine checkout"
    assert_dir_not_exists "${TARGET}"
    # The staging directory must not survive a failed clone either.
    run bash -c 'shopt -s nullglob; left=("$1".tmp.*); echo "${#left[@]}"' _ "${TARGET}"
    assert_output "0"
}

@test "clones the full single-branch history when ZWIFT_WINE_DEPTH is 0" {
    printf 'more\n' >"${UPSTREAM}/more"
    git -C "${UPSTREAM}" add -A
    git -C "${UPSTREAM}" commit -qm 'second'

    run "${FETCH}"
    assert_success
    run git -C "${TARGET}" rev-list --count HEAD
    assert_output "1"

    rm -rf "${TARGET}"
    export ZWIFT_WINE_DEPTH=0

    run "${FETCH}"
    assert_success
    run git -C "${TARGET}" rev-list --count HEAD
    assert_output "2"
}
