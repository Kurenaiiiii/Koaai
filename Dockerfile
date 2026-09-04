# Koaai — everything-baked runtime image.
#
# Contains: koaai + yt-dlp nightly + Node 22. No secrets are baked in —
# TOKEN and friends are injected at RUN time (panel variables or .env).
#
# Build (CI does this for you on every release — see .github/workflows/docker.yml):
#   cargo build --release && cp target/release/koaai ./koaai && docker build -t koaai .
# The `koaai` binary must exist next to this file (it is gitignored on purpose).

FROM debian:bookworm-slim

# yt-dlp (python bundle) needs system CA certs at runtime. curl/xz are only
# needed to fetch node during the build and are removed in the same layer.
RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends ca-certificates curl xz-utils; \
    \
    echo "── Node 22 (latest v22.x) ──"; \
    NODE_FILE="$(curl -fsSL https://nodejs.org/dist/latest-v22.x/ | grep -oE 'node-v22\.[0-9]+\.[0-9]+-linux-x64\.tar\.xz' | head -1)"; \
    [ -n "$NODE_FILE" ] || { echo "could not resolve latest Node v22"; exit 1; }; \
    curl -fsSL -o /tmp/node.tar.xz "https://nodejs.org/dist/latest-v22.x/${NODE_FILE}"; \
    tar -xf /tmp/node.tar.xz -C /tmp; \
    mv /tmp/node-v*-linux-x64/bin/node /usr/local/bin/node; \
    rm -rf /tmp/node.tar.xz "/tmp/node-v22"*; \
    \
    echo "── yt-dlp nightly ──"; \
    curl -fsSL --retry 3 -o /usr/local/bin/yt-dlp \
        "https://github.com/yt-dlp/yt-dlp-nightly-builds/releases/latest/download/yt-dlp_linux"; \
    chmod +x /usr/local/bin/yt-dlp /usr/local/bin/node; \
    \
    echo "── verify ──"; \
    node --version; \
    yt-dlp --version; \
    \
    echo "── drop build-only tools ──"; \
    apt-get purge -y curl xz-utils; \
    apt-get autoremove -y; \
    rm -rf /var/lib/apt/lists/*

# The bot binary comes from the build context (CI: target/release/koaai).
# It lives on the (read-only under Pterodactyl) image layer — executing from
# there is fine, only WRITES need the mounted dir below.
COPY koaai /app/koaai
RUN chmod +x /app/koaai

# CWD MUST be the writable mount, not /app/*:
# Pterodactyl runs containers with a read-only rootfs and only
# /home/container is a writable volume. bot.db, config.toml (auto-created)
# and .env live here. (An earlier revision used /app/data and broke panels
# with "Read-only file system" — never again.)
WORKDIR /home/container

CMD ["/app/koaai"]
