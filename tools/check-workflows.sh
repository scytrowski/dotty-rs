#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
actionlint_version="1.7.12"

if command -v actionlint >/dev/null 2>&1 \
    && [[ "$(actionlint -version 2>&1 | head -n 1)" == "$actionlint_version" ]]; then
    actionlint -color "$repo_root/.github/workflows"/*.yml
elif command -v docker >/dev/null 2>&1; then
    docker run --rm \
        --volume "$repo_root:/repo:ro" \
        --workdir /repo \
        "rhysd/actionlint:$actionlint_version" \
        -color
else
    cat >&2 <<EOF
actionlint $actionlint_version is required to validate GitHub Actions workflows.
An installed actionlint with a different version does not satisfy this check.
Install it with:
  go install github.com/rhysd/actionlint/cmd/actionlint@v$actionlint_version
or install Docker and rerun this script; it will use the pinned official image.
EOF
    exit 127
fi
