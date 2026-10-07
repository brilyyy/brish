# syntax=docker/dockerfile:1
# briSH QA image: reference shells + a release-ish brish binary.
#
# Two stages so the runtime image carries no toolchain: the builder
# compiles the workspace, the runtime copies only `target/release/brish`
# plus dash/bash for the oracles. Debian, not Alpine — busybox `ash` is
# a useful *third* oracle but its coreutils differ enough that a diff
# against it produces more noise than signal for POSIX checks.
FROM rust:1.90-bookworm AS builder

WORKDIR /src
# Dependency layer first: cached until Cargo.lock changes.
COPY Cargo.toml Cargo.lock ./
COPY crates/brish/Cargo.toml crates/brish/
COPY crates/brish-core/Cargo.toml crates/brish-core/
COPY crates/brish-builtin/Cargo.toml crates/brish-builtin/
COPY crates/brish-words/Cargo.toml crates/brish-words/
COPY crates/brish-platform/Cargo.toml crates/brish-platform/
COPY crates/brish-plugin/Cargo.toml crates/brish-plugin/
COPY crates/brish-theme/Cargo.toml crates/brish-theme/
RUN mkdir -p crates && for d in brish brish-core brish-builtin brish-words \
      brish-platform brish-plugin brish-theme; do \
      echo "pub fn placeholder() {}" > "crates/$d/src/lib.rs"; \
    done \
 && echo 'fn main() {}' > crates/brish/src/main.rs \
 && mkdir -p crates/brish/src \
 && cargo build --release -p brish 2>&1 | tail -3

COPY crates crates
# Touch so the placeholder artifacts don't win the fingerprint.
RUN touch crates/*/src/lib.rs crates/brish/src/main.rs \
 && cargo build --release -p brish 2>&1 | tail -5 \
 && ./target/release/brish --version

FROM debian:bookworm-slim AS runtime

# dash is the POSIX oracle the whole conformance suite is built on;
# bash is the second opinion for constructs dash rejects. `coreutils`
# supplies the real-world /usr/bin corpus we sweep.
RUN apt-get update && apt-get install -y --no-install-recommends \
      dash bash coreutils findutils grep sed gawk diffutils \
      ca-certificates \
 && rm -rf /var/lib/apt/lists/*

COPY --from=builder /src/target/release/brish /usr/local/bin/brish

# Unprivileged by default: `-r`/`-w`/`-x` truth tables differ under uid 0,
# and running the sweep as root would hide the very bug class we fixed.
RUN useradd --create-home --shell /bin/bash qa
USER qa
WORKDIR /home/qa

RUN dash -c 'exit 0' && bash -c 'exit 0' && brish -c 'echo brish-ok' \
 && echo "oracles ready"