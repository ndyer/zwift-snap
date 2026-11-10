#!/usr/bin/env bats

load test_helper

# The knowledge base is governed by .kb/agents.md. These tests enforce the laws
# that can be checked mechanically: the AGENTS.md index (§4), the filenames (§1)
# and the headers (§2). .kb/agents.md itself must NOT be edited unless explicitly
# requested, so it is never rewritten here, only checked.

@test "AGENTS.md starts with the knowledge-base index of every .kb file" {
    run python3 - "${ROOT}" <<'PY'
import os
import re
import sys

root = sys.argv[1]
# Build outputs, the fetched skill set and the separate Wine clone are not part
# of this repository's knowledge base.
skipped = {"parts", "prime", "stage", "overlay", "wine-src", "target", "node_modules", ".git"}


def knowledge_base_files(base):
    found = []
    for entry in sorted(os.listdir(base)):
        path = os.path.join(base, entry)
        if entry in skipped or not os.path.isdir(path):
            continue
        if entry == ".kb":
            for name in sorted(os.listdir(path)):
                if name.endswith(".md"):
                    found.append(os.path.relpath(os.path.join(path, name), root))
        else:
            found.extend(knowledge_base_files(path))
    return found


files = knowledge_base_files(root)
assert files, "no .kb/*.md files found"
assert ".kb/agents.md" in files

with open(os.path.join(root, "AGENTS.md")) as source:
    lines = source.read().splitlines()

assert lines[0] == "# Knowledge Base", lines[0]
assert lines[1] == "", lines[1]
assert lines[2] == "IMPORTANT: Read the `.kb/agents.md` file.", lines[2]
assert lines[3] == "", lines[3]

items = [line for line in lines[3:] if line]
assert items, "AGENTS.md lists no knowledge-base files"
assert items[0] == ("- [.kb/agents.md](.kb/agents.md) - Structure and rules of the"
                    " knowledge base. You MUST read this file."), items[0]

listed = []
for line in items:
    match = re.fullmatch(r"- \[([^\]]+)\]\(([^)]+)\) - (.+)", line)
    assert match, "malformed index line: " + line
    assert match.group(1) == match.group(2), line
    listed.append(match.group(1))

assert listed[0] == ".kb/agents.md", listed[0]
assert sorted(listed) == sorted(files), (
    "missing from AGENTS.md: " + repr(sorted(set(files) - set(listed))) +
    ", not in the repository: " + repr(sorted(set(listed) - set(files))))
assert len(listed) == len(set(listed)), "duplicate index entries"
PY
    assert_success
}

@test "knowledge-base filenames are dashed lowercase and globally unique" {
    run python3 - "${ROOT}" <<'PY'
import os
import re
import sys

root = sys.argv[1]
skipped = {"parts", "prime", "stage", "overlay", "wine-src", "target", "node_modules", ".git"}


def knowledge_base_files(base):
    found = []
    for entry in sorted(os.listdir(base)):
        path = os.path.join(base, entry)
        if entry in skipped or not os.path.isdir(path):
            continue
        if entry == ".kb":
            found.extend(os.path.relpath(os.path.join(path, name), root)
                         for name in sorted(os.listdir(path)) if name.endswith(".md"))
        else:
            found.extend(knowledge_base_files(path))
    return found


seen = {}
for path in knowledge_base_files(root):
    name = os.path.basename(path)
    assert re.fullmatch(r"[a-z0-9]+(-[a-z0-9]+)*\.md", name), "not dashed lowercase: " + path
    assert name not in seen, "duplicate filename: " + path + " and " + seen.get(name, "")
    seen[name] = path
PY
    assert_success
}

@test "knowledge-base files obey the Preface and header laws" {
    run python3 - "${ROOT}" <<'PY'
import os
import re
import sys

root = sys.argv[1]
skipped = {"parts", "prime", "stage", "overlay", "wine-src", "target", "node_modules", ".git"}
allowed = ["# Preface", "# Laws", "# Concepts"]
required_last_line = "IMPORTANT: Read the `.kb/agents.md` file."


def knowledge_base_files(base):
    found = []
    for entry in sorted(os.listdir(base)):
        path = os.path.join(base, entry)
        if entry in skipped or not os.path.isdir(path):
            continue
        if entry == ".kb":
            found.extend(os.path.relpath(os.path.join(path, name), root)
                         for name in sorted(os.listdir(path)) if name.endswith(".md"))
        else:
            found.extend(knowledge_base_files(path))
    return found


for path in knowledge_base_files(root):
    with open(os.path.join(root, path)) as source:
        lines = source.read().splitlines()

    # Headers inside fenced code blocks (AGENTS.md's §4 shows one) are examples,
    # not headers.
    top, sub = [], []
    fenced = False
    for index, line in enumerate(lines):
        if line.startswith("```"):
            fenced = not fenced
            continue
        if fenced:
            continue
        if line.startswith("# "):
            top.append((index, line))
        elif line.startswith("## "):
            sub.append((index, line))

    assert top, path + " has no headers"
    headers = [line for _, line in top]
    assert headers[0] == "# Preface", (path, headers[0])
    for header in headers:
        assert header in allowed, (path, header)
    order = [allowed.index(header) for header in headers]
    assert order == sorted(order), (path, headers)

    for index, header in top[1:]:
        assert index >= 3 and lines[index - 1] == "" and lines[index - 2] == "" \
            and lines[index - 3] != "", (path, "top header without two empty lines before it", header)
    for index, header in sub:
        assert lines[index - 1] == "" and lines[index - 2] != "", \
            (path, "subheader without one empty line before it", header)

    end = top[1][0] if len(top) > 1 else len(lines)
    preface = [line for line in lines[top[0][0] + 1:end] if line.strip()]
    assert preface, path + " has an empty preface"
    assert preface[-1] == required_last_line, (path, preface[-1])
PY
    assert_success
}

@test "the AGENTS.md heading is the first line and no content follows the index" {
    run python3 - "${ROOT}" <<'PY'
import sys

with open(sys.argv[1] + "/AGENTS.md") as source:
    lines = source.read().splitlines()

assert lines[0] == "# Knowledge Base"
assert lines[-1].startswith("- ["), lines[-1]
body = [line for line in lines if line and not line.startswith("- [")]
assert body == ["# Knowledge Base", "IMPORTANT: Read the `.kb/agents.md` file."], body
PY
    assert_success
}
