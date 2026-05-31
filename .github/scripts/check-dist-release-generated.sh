#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "${script_dir}/../.." && pwd)"
cd "${repo_root}"

dist_config="dist-workspace.toml"
release_workflow=".github/workflows/release.yml"
dry_run_workflow=".github/workflows/dist-dry-run-release.yml"

# shellcheck source=.github/scripts/dist-release-lib.sh
source "${script_dir}/dist-release-lib.sh"

# Throwaway namespace so the dry-run regeneration writes to a filename that does
# not collide with any committed workflow. Under dispatch-releases the namespace
# never appears in the generated body, so the content is identical to the
# committed dry-run workflow regardless of this value.
check_tag_namespace="dist-drift-check"
check_workflow=".github/workflows/${check_tag_namespace}-release.yml"

ensure_dist

changed=0

for file in "${release_workflow}" "${dry_run_workflow}"; do
  if [ ! -f "${file}" ]; then
    echo "${file} is missing; run \`just regen-dist-release\`" >&2
    changed=1
  fi
done

# release.yml: cargo-dist can check this itself without writing anything.
if ! check_output="$(dist generate --mode=ci --check 2>&1)"; then
  echo "${release_workflow} is out of date; run \`just regen-dist-release\` and commit the result." >&2
  printf '%s\n' "${check_output}" >&2
  changed=1
fi

# dist-dry-run-release.yml: cargo-dist has no notion of this file (we post-process
# its output), so regenerate it into a temp location and compare. dist only reads
# its config from ${dist_config} and only writes into .github/workflows/, so we
# temporarily rewrite the config and redirect output via a throwaway namespace,
# restoring everything via the trap below. The committed workflows are never written.
tmpdir="$(mktemp -d)"
cleanup() {
  if [ -f "${tmpdir}/${dist_config}.bak" ]; then
    cp "${tmpdir}/${dist_config}.bak" "${dist_config}"
  fi
  rm -f "${check_workflow}"
  rm -rf "${tmpdir}"
}
trap cleanup EXIT

cp "${dist_config}" "${tmpdir}/${dist_config}.bak"
write_dry_run_dist_config "${tmpdir}/${dist_config}.bak" "${check_tag_namespace}"

if ! gen_output="$(dist generate --mode=ci 2>&1)"; then
  echo "Failed to regenerate ${dry_run_workflow} for the drift check:" >&2
  printf '%s\n' "${gen_output}" >&2
  exit 1
fi

cp "${tmpdir}/${dist_config}.bak" "${dist_config}"
mv "${check_workflow}" "${tmpdir}/expected.yml"
apply_dry_run_safety_overlay "${tmpdir}/expected.yml"

if ! cmp -s "${tmpdir}/expected.yml" "${dry_run_workflow}"; then
  echo "${dry_run_workflow} is out of date; run \`just regen-dist-release\` and commit the result." >&2
  diff -u "${dry_run_workflow}" "${tmpdir}/expected.yml" || true
  changed=1
fi

exit "${changed}"
