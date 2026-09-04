# Koaai Deploy Guide

Pick one path. Both use the same image (`ghcr.io/kurenaiiiii/koaai`), which
already contains everything: the bot, yt-dlp nightly, and Node 22. You never
install dependencies by hand.

> **Secrets rule:** the image has NO token inside. Secrets always come from
> outside — panel variables, or a `.env` file. Never commit `.env`.

---

## Path A — Pterodactyl panel (easiest)

1. Panel → **Nests** → **Import** → upload `pterodactyl/egg-koaai.json`.
2. Create a server from the **Koaai** egg. Image is already set, nothing to upload.
3. Fill variables: **Bot Token** (required), Owner ID / Spotify keys (optional).
4. Press **Start**. Done.
5. `bot.db` and `config.toml` live in the server dir — edit `config.toml` in the
   file manager, then restart.

**Update:** reinstall or just restart the server — it pulls the newest image.
If playback breaks (YouTube changed something), a restart pulls a fresh image
with fresh yt-dlp.

---

## Path B — VPS / self-host with Docker (one command)

Needs: Docker installed. That's the only requirement.

```bash
git clone https://github.com/Kurenaiiiii/Koaai.git && cd Koaai

# 1. make your secrets file from the template
cp .env.example .env

# 2. open .env and fill it in (TOKEN is required, rest optional)
nano .env

# 3. start it
docker compose up -d

# 4. watch the logs — you want "ready as ..."
docker compose logs -f
```

Your `.env` looks like this:

```ini
TOKEN=your-bot-token
OWNER_ID=your-discord-id
SPOTIFY_CLIENT_ID=
SPOTIFY_CLIENT_SECRET=
#DEV_GUILD_ID=
```

Data (`bot.db`, `config.toml`) is stored in `./data/` next to the compose file,
so it survives restarts and updates. Edit `./data/config.toml`, then
`docker compose restart`.

**No compose?** Plain docker works too:

```bash
docker run -d --name koaai --restart unless-stopped \
  --env-file .env -v ./data:/home/container \
  ghcr.io/kurenaiiiii/koaai:latest
```

**Update:**

```bash
docker compose pull && docker compose up -d
```

---

## Path C — bundle zip, no Docker

Every build produces `koaai-bundle-linux-x64.zip` (Actions artifacts, and
attached to each Release). Inside, perfectly organized: `koaai` + `bin/yt-dlp`
+ `bin/node`. Nothing to install.

1. Download the zip from the latest
   [Release](https://github.com/Kurenaiiiii/Koaai/releases) (or any Actions run).
2. Unzip it anywhere, `cd` into the folder.
3. Create `.env` next to `koaai` (same format as Path B — TOKEN required).
4. Make sure the bot finds its tools (they're in `bin/`, same folder layout
   the bot expects on PATH):
   ```bash
   export PATH="$PWD/bin:$PATH"
   ./koaai
   ```
   (Or move `bin/` anywhere on your PATH once.)

---

## Troubleshooting

| Symptom | Fix |
|---|---|
| `/play` says "Interaction failed" once, works second try | Old image — update. (Cold voice joins used to outlive the 3s window.) |
| VC status never shows | Bot needs the **Set Voice Channel Status** permission *and* must be sitting in that VC. |
| Playback errors on every track | YouTube moved again — update the image (fresh yt-dlp), restart. |
| Bot won't start / token rejected | `.env` missing or wrong TOKEN. Panel: check the Bot Token variable. |
| `Read-only file system (os error 30)` on panel | Old image started the bot in `/app/data` (image rootfs is read-only). Fixed in current images (workdir is `/home/container`); just pull + restart. |
| `docker compose` can't find `.env` | Run the command from the same folder as the compose file. |
