# syntax=docker/dockerfile:1

# ---- builder ----
FROM rust:1.95-bookworm AS builder
WORKDIR /src
# buildpack-deps base already has gcc/make/pkg-config for vendored-libgit2 + rustls.
COPY . .
# caliband: normal defaults. caliban: drop the `clipboard`/arboard X11 link dep.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo build --release -p caliban-supervisor --bin caliband \
 && cargo build --release -p caliban --bin caliban --no-default-features

# ---- runtime ----
FROM debian:bookworm-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends bubblewrap git ca-certificates \
 && rm -rf /var/lib/apt/lists/*
# non-root user with a writable HOME for XDG config/data/runtime dirs
RUN useradd --uid 10001 --create-home --home-dir /home/app --shell /usr/sbin/nologin app
COPY --from=builder /src/target/release/caliban  /usr/local/bin/caliban
COPY --from=builder /src/target/release/caliband /usr/local/bin/caliband
ENV HOME=/home/app \
    XDG_CONFIG_HOME=/home/app/.config \
    XDG_DATA_HOME=/home/app/.local/share \
    XDG_RUNTIME_DIR=/home/app/.run \
    CALIBAN_DAEMON_RUNTIME_DIR=/home/app/.run/caliban
RUN mkdir -p /home/app/.config/caliban /home/app/.local/share/caliban /home/app/.run/caliban \
 && chown -R app:app /home/app
USER app
WORKDIR /home/app
ENTRYPOINT ["caliband"]
CMD ["--help"]
