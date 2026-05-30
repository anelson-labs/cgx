#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/../.." && pwd)"
cd "${REPO_ROOT}"

OWNER="${OWNER:-anelson-labs}"
IMAGE_NAME="${IMAGE_NAME:-cgx-musl-build}"
IMAGE="${IMAGE:-ghcr.io/${OWNER}/${IMAGE_NAME}}"
CONTEXT="${CONTEXT:-build-images/linux-musl}"
DOCKERFILE="${DOCKERFILE:-${CONTEXT}/Dockerfile}"
VISIBILITY_URL="https://github.com/orgs/${OWNER}/packages/container/${IMAGE_NAME}/settings"

rust_version="$(sed -n 's/^channel[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' rust-toolchain.toml)"
if [[ -z "${rust_version}" ]]; then
  echo "Could not read Rust version from rust-toolchain.toml" >&2
  exit 1
fi

revision="$(git rev-parse HEAD)"
created="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
source_url="https://github.com/${OWNER}/cgx"
description="cargo-dist Alpine musl build image for cgx"

case "$(uname -m)" in
  x86_64 | amd64) host_platform="linux/amd64" ;;
  aarch64 | arm64) host_platform="linux/arm64" ;;
  *)
    host_platform="unknown"
    ;;
esac

echo "Publishing ${IMAGE}:${rust_version}"
echo "Host platform: ${host_platform}"

if ! command -v docker >/dev/null 2>&1; then
  echo "docker is required" >&2
  exit 1
fi

if ! docker info >/dev/null 2>&1; then
  echo "docker daemon is not reachable" >&2
  exit 1
fi

if ! command -v gh >/dev/null 2>&1; then
  echo "gh is required so the script can authenticate to GHCR and check package visibility" >&2
  exit 1
fi

gh auth status >/dev/null
gh_user="$(gh api user --jq .login)"
gh auth token | docker login ghcr.io -u "${gh_user}" --password-stdin >/dev/null

if ! docker buildx inspect cgx-build-images >/dev/null 2>&1; then
  docker buildx create --name cgx-build-images --use >/dev/null
else
  docker buildx use cgx-build-images >/dev/null
fi

docker buildx inspect --bootstrap >/dev/null

if [[ "${host_platform}" == "unknown" ]]; then
  echo "Ensuring QEMU/binfmt support is available for cross-platform builds..."
  docker run --privileged --rm tonistiigi/binfmt --install amd64,arm64 >/dev/null
elif [[ "${host_platform}" == "linux/amd64" ]]; then
  echo "Ensuring QEMU/binfmt support is available for linux/arm64..."
  docker run --privileged --rm tonistiigi/binfmt --install arm64 >/dev/null
else
  echo "Ensuring QEMU/binfmt support is available for linux/amd64..."
  docker run --privileged --rm tonistiigi/binfmt --install amd64 >/dev/null
fi

common_args=(
  --file "${DOCKERFILE}"
  --build-arg "RUST_VERSION=${rust_version}"
  --label "org.opencontainers.image.created=${created}"
  --label "org.opencontainers.image.description=${description}"
  --label "org.opencontainers.image.revision=${revision}"
  --label "org.opencontainers.image.source=${source_url}"
  --label "org.opencontainers.image.title=${IMAGE_NAME}"
  --label "org.opencontainers.image.version=${rust_version}"
  --annotation "org.opencontainers.image.created=${created}"
  --annotation "org.opencontainers.image.description=${description}"
  --annotation "org.opencontainers.image.revision=${revision}"
  --annotation "org.opencontainers.image.source=${source_url}"
  --annotation "org.opencontainers.image.title=${IMAGE_NAME}"
  --annotation "org.opencontainers.image.version=${rust_version}"
  --provenance=false
  --sbom=false
  --push
)

docker buildx build \
  --platform linux/amd64 \
  --tag "${IMAGE}:${rust_version}-amd64" \
  "${common_args[@]}" \
  "${CONTEXT}"

docker buildx build \
  --platform linux/arm64 \
  --tag "${IMAGE}:${rust_version}-arm64" \
  "${common_args[@]}" \
  "${CONTEXT}"

docker buildx imagetools create \
  --tag "${IMAGE}:${rust_version}" \
  "${IMAGE}:${rust_version}-amd64" \
  "${IMAGE}:${rust_version}-arm64"

echo "Published:"
echo "  ${IMAGE}:${rust_version}-amd64"
echo "  ${IMAGE}:${rust_version}-arm64"
echo "  ${IMAGE}:${rust_version}"

visibility="$(
  gh api "/orgs/${OWNER}/packages/container/${IMAGE_NAME}" --jq '.visibility' 2>/dev/null || true
)"

case "${visibility}" in
  public)
    echo "GHCR package is public."
    ;;
  "")
    echo "::warning::Could not check GHCR package visibility with gh api."
    echo "Check package visibility here: ${VISIBILITY_URL}"
    ;;
  *)
    echo "::warning::Published successfully, but GHCR reports package visibility '${visibility}'."
    echo "::warning::cargo-dist release jobs cannot pull this job container anonymously until the package is public."
    echo "Make the package public here: ${VISIBILITY_URL}"
    ;;
esac
