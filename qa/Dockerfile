# syntax=docker/dockerfile:1
# briSH QA image: reference shells + a release brish binary.
#
# Two stages so the runtime image carries no toolchain: the builder
# compiles the workspace, the runtime copies only the brish binary plus
# dash/bash for the oracles. Debian, not Alpine — busybox `ash` is a
# useful third oracle but its coreutils differ enough that diffing
# against it produces more noise than signal for POSIX checks.
FROM rust:1.96-bookworm AS builder

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
RUN cargo build --release -p brish 2>&1 | tail -5 \
 && ./target/release/brish --version

FROM debian:bookworm-slim AS runtime

# dash is the POSIX oracle the whole conformance suite is built on;
# bash is the second opinion for constructs dash rejects. coreutils +
# friends supply the real-world /usr/bin corpus we sweep.
RUN apt-get update && apt-get install -y --no-install-recommends \
      dash bash coreutils findutils grep sed gawk diffutils \
 && rm -rf /var/lib/apt/lists/*

COPY --from=builder /src/target/release/brish /usr/local/bin/brish

# Unprivileged by default: `-r`/`-w`/`-x` truth tables differ under uid 0,
# and running the sweep as root would hide the very bug class we fixed.
RUN useradd --create-home --shell /bin/bash qa
USER qa
WORKDIR /home/qa

RUN dash -c 'exit 0' && bash -c 'exit 0' && brish -c 'echo brish-ok' \
 && echo "oracles ready"