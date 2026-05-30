#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "${script_dir}/../.." && pwd)"
cd "${repo_root}"

files=(
  .github/workflows/release.yml
  .github/workflows/dist-dry-run-release.yml
)

tmpdir="$(mktemp -d)"
cleanup() {
  rm -rf "${tmpdir}"
}
trap cleanup EXIT

for file in "${files[@]}"; do
  if [ ! -f "${file}" ]; then
    echo "${file} is missing; run \`just regen-dist-release\`" >&2
    exit 1
  fi

  mkdir -p "${tmpdir}/$(dirname "${file}")"
  cp "${file}" "${tmpdir}/${file}"
done

.github/scripts/regen-dist-release.sh

changed=0
for file in "${files[@]}"; do
  if ! cmp -s "${tmpdir}/${file}" "${file}"; then
    echo "${file} changed after regeneration; run \`just regen-dist-release\` and commit the result." >&2
    diff -u "${tmpdir}/${file}" "${file}" || true
    changed=1
  fi
done

exit "${changed}"
