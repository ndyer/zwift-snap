# Preface

How to recover snap objects after an interrupted or failed launch, when the pkgproxy
snap cache is the only survivor of a rollback. Read this before re-downloading
everything — the objects are usually still in a stopped snapshot container.

IMPORTANT: Read the `.kb/agents.md` file.


# Concepts

## Recovering the pkgproxy snap cache

The `project-pkgproxy` SDK seeds its snap cache from `.workshop/pkgproxy/cache/`
before the project is mounted. Each cached snap is a file named by its 96-character
SHA3-384 digest, paired with `<digest>.meta.json`. The live cache is under
`/var/snap/pkgproxy/common/cache/snap/objects/sha3-384` inside the workshop.

To recover snap objects after an interrupted or failed launch:

1. Check `workshop list`, `workshop changes` and `workshop tasks <change-id>` first.
   A failed launch normally rolls back and deletes its container; downloads after
   the last SDK snapshot are then lost. If the change is still running, do not
   interfere with it. For future troubleshooting, `workshop launch --wait-on-error`
   preserves the failed instance for investigation until continued or aborted.
2. Locate the surviving instance or SDK snapshots with `lxc list --all-projects`.
   The workshop is normally in `workshop.$USER`; SDK snapshots are stopped
   containers in `workshop-snapshots.$USER`. Check creation times and task order:
   a `pkgproxy-*` snapshot taken before the next SDK's setup cannot contain its
   later downloads. Older `zwift-snap-sdk-*` or `rust-*` snapshots may contain
   useful objects, but are not evidence of downloads from the latest launch.
3. List the cache directory without starting the instance. Substitute the actual
   project and instance names (the project can use a numeric UID instead of the
   username):

   ```sh
   lxc query '/1.0/instances/<instance>/files?project=<project>&path=/var/snap/pkgproxy/common/cache/snap/objects/sha3-384'
   ```

   Compare the names with `.workshop/pkgproxy/cache/`; a missing directory means
   that snapshot has no cache at that path. Do not copy incomplete objects or
   overwrite existing local files.
4. For each missing pair, pull both files to a temporary directory, including
   from stopped snapshot containers:

   ```sh
   lxc file pull --project <project> <instance>/var/snap/pkgproxy/common/cache/snap/objects/sha3-384/<digest> <temporary-directory>/
   lxc file pull --project <project> <instance>/var/snap/pkgproxy/common/cache/snap/objects/sha3-384/<digest>.meta.json <temporary-directory>/
   ```

   Verify the blob's SHA3-384 against its filename, and its size and content hash
   against the JSON sidecar before copying the pair into `.workshop/pkgproxy/cache/`.
   Sidecars may encode `ContentHash` as `sha3-384:<digest>` or
   `sha3-384/<digest>`. Do not replace existing entries. `unsquashfs -cat <blob>
   meta/snap.yaml` identifies a snap if needed. Remove only the temporary files
   created for recovery.
