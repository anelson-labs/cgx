#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "${script_dir}/../.." && pwd)"
cd "${repo_root}"

dist_config="dist-workspace.toml"
dry_run_workflow=".github/workflows/dist-dry-run-release.yml"

# shellcheck source=.github/scripts/dist-release-lib.sh
source "${script_dir}/dist-release-lib.sh"

ensure_dist

echo "Regenerating .github/workflows/release.yml..."
dist generate --mode=ci

tmpdir="$(mktemp -d)"
cleanup() {
  local status="$?"

  if [ -f "${tmpdir}/${dist_config}" ]; then
    cp "${tmpdir}/${dist_config}" "${dist_config}"
  fi
  rm -rf "${tmpdir}"
  exit "${status}"
}
trap cleanup EXIT

cp "${dist_config}" "${tmpdir}/${dist_config}"
write_dry_run_dist_config "${tmpdir}/${dist_config}"

echo "Regenerating ${dry_run_workflow} with temporary dry-run settings..."
dist generate --mode=ci

cp "${tmpdir}/${dist_config}" "${dist_config}"
trap - EXIT
rm -rf "${tmpdir}"

apply_dry_run_safety_overlay "${dry_run_workflow}"
