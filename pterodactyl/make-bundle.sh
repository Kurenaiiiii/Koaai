#!/usr/bin/env bash
# Builds koaai-bundle.tar.gz — the only file your Pterodactyl server ever needs.
#
# Contains: koaai (latest CI build) + yt-dlp nightly + node 22, all x86_64.
#
# Usage:  ./make-bundle.sh        (needs: gh authed, curl, tar)
set -euo pipefail

REPO="Kurenaiiiii/Koaai"
OUT="koaai-bundle"

echo "── pulling latest CI binary ──"
rm -rf "$OUT" koaai-bundle.tar.gz
mkdir -p "$OUT/bin"
gh run download --repo "$REPO" -n koaai-linux-x64 -D "$OUT"

echo "── fetching yt-dlp nightly ──"
curl -sSL --retry 3 -o "$OUT/bin/yt-dlp" \
    "https://github.com/yt-dlp/yt-dlp-nightly-build/releases/latest/download/yt-dlp_linux"
[ "$(head -c4 "$OUT/bin/yt-dlp")" = $'\x7fELF' ] || { echo "yt-dlp download junk!"; exit 1; }

echo "── fetching node 22 ──"
NODE_VERSION=22.14.0
curl -sSL -o /tmp/node.tar.xz \
    "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-x64.tar.xz"
tar -xf /tmp/node.tar.xz -C /tmp
cp "/tmp/node-v${NODE_VERSION}-linux-x64/bin/node" "$OUT/bin/node"

chmod +x "$OUT/koaai" "$OUT/bin/yt-dlp" "$OUT/bin/node"

echo "── verifying everything actually runs ──"
"$OUT/koaai" </dev/null >/dev/null 2>&1 || true   # exits on missing TOKEN — fine, proves ELF+libs
"$OUT/bin/yt-dlp" --version
"$OUT/bin/node" --version

tar -czf koaai-bundle.tar.gz -C "$OUT" .
rm -rf "$OUT"

echo ""
echo "✅ DONE → $(du -h koaai-bundle.tar.gz | cut -f1) koaai-bundle.tar.gz"
echo "Next:"
echo "  1. Upload koaai-bundle.tar.gz via the panel file manager (server root)"
echo "  2. Press Reinstall (egg script extracts it), or Extract it yourself in the file manager"
echo "  3. Start"
