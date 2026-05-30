# This image is the cargo-dist build container for Linux musl release artifacts.
#
# cargo-dist starts the configured job container before checkout and before our
# github-build-setup hook. That means tools used by generated pre-checkout
# steps, actions/checkout, cargo-dist installation, rust-cache, and the native
# musl/OpenSSL build must already exist in the image. Installing them later from
# .github/dist-build-setup.yml is too late.

ARG RUST_VERSION
FROM rust:${RUST_VERSION}-alpine

ARG RUST_VERSION

RUN apk add --no-cache \
      bash \
      build-base \
      ca-certificates \
      curl \
      git \
      openssl-dev \
      openssl-libs-static \
      perl \
      pkgconf \
      sed \
      tar \
      xz

ENV OPENSSL_STATIC=1 \
    PKG_CONFIG_ALL_STATIC=1

RUN set -eux; \
    test "$(rustc -vV | sed -n 's/^release: //p')" = "${RUST_VERSION}"; \
    tar --version | grep -q 'GNU tar'; \
    xz --version >/dev/null; \
    git --version >/dev/null; \
    bash --version >/dev/null; \
    curl --version >/dev/null; \
    make --version >/dev/null; \
    cc --version >/dev/null; \
    pkg-config --libs --static openssl >/dev/null; \
    test -f /usr/lib/libssl.a; \
    test -f /usr/lib/libcrypto.a
