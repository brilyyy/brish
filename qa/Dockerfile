# syntax=docker/dockerfile:1
# briSH QA image: reference shells + a release brish binary + a harness.
#
# Two stages so the runtime image carries no toolchain: the builder
# compiles the workspace, the runtime copies only the brish binary plus
# the oracles. Debian, not Alpine — busybox `ash` is a useful lean
# oracle but its coreutils differ enough that diffing against it
# produces more noise than signal for POSIX checks (we install it as a
# *shell* to benchmark, not as a coreutils replacement).
FROM rust:1.96-bookworm AS builder

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
RUN cargo build --release -p brish 2>&1 | tail -5 \
 && ./target/release/brish --version

FROM debian:bookworm-slim AS runtime

# dash is the POSIX oracle the whole conformance suite is built on.
# bash is the second opinion for constructs dash rejects. mksh and
# busybox ash are *lean* POSIX implementations: comparing against them
# is what separates "brish is slow" from "Rust is fast", because they
# are not C shells carrying a decade of bash-isms.
#
# python3   -> pty driver for the REPL (main.rs:390 gates the interactive
#              path on stdin AND stdout being TTYs, so there is no way to
#              test completion/highlighting/menus without one).
# time      -> GNU time -v for peak RSS in the benchmark layer.
# util-linux-> taskset for CPU pinning (benchmark noise).
RUN apt-get update && apt-get install -y --no-install-recommends \
      dash bash mksh busybox-static \
      coreutils findutils grep sed gawk diffutils \
      python3 time util-linux \
 && rm -rf /var/lib/apt/lists/*

COPY --from=builder /src/target/release/brish /usr/local/bin/brish

# Example plugins (starter / sentinel / words) so the store layer has real
# plugins to install rather than fixtures it invented.
COPY examples /usr/local/share/brish-examples

# Unprivileged by default: `-r`/`-w`/`-x` truth tables differ under uid 0,
# and running the suite as root would hide the very bug class we fixed.
RUN useradd --create-home --shell /bin/bash qa
USER qa
WORKDIR /home/qa

# Fail the build, not the run. A silently broken oracle makes every
# comparison read as "brish: command not found (127)" — i.e. it
# manufactures failures that look like real bugs. qa/diff-snippets.sh
# documented this failure mode once already; assert it up front instead.
RUN dash -c 'exit 0' \
 && bash -c 'exit 0' \
 && mksh -c 'exit 0' \
 && busybox ash -c 'exit 0' \
 && brish -c 'echo brish-ok' \
 && python3 -c 'import pty' \
 && echo "oracles ready: dash bash mksh busybox-ash brish"