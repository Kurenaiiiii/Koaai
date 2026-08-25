# Deploying Koaai on Pterodactyl

Two ways to get the binary onto the server — pick one.

## 0. Get a binary URL

The GitHub Action builds `koaai` (linux x64, glibc 2.35-compatible) on every
push. Two options:

**A. Release asset (recommended, stable URL):**
```bash
gh release create v1.0.0 --title v1.0.0
gh run download            # grab the artifact from the latest Actions run...
gh release upload v1.0.0 koaai
```
Direct URL: `https://github.com/<you>/Koaai/releases/download/v1.0.0/koaai`

**B. Actions artifact:** download from the run page, then re-upload to the
server via SFTP/panel file manager (artifact URLs need auth, so the egg's
curl won't work with them directly).

> Private repo? The egg needs a `GITHUB_TOKEN` (PAT with `repo` scope) to
> `curl` release assets.

## 1. Import the egg

Panel → **Nests** → choose/create a nest → **Import** → upload
`pterodactyl/egg-koaai.json`.

## 2. Create the server

New server → nest **Koaai** → image **Debian Bookworm** → fill variables:

| Variable | Notes |
|---|---|
| Binary download URL | from step 0 |
| Bot token | required |
| GitHub token | only for private repos |
| Owner ID / DEV_GUILD_ID / Spotify keys | optional, same as `.env` |

The install script runs once: installs **yt-dlp nightly**, downloads
**Node 22** into the server dir, fetches your binary, chmod +x.

## 3. Start it

Press **Start**. Console shows the banner → self-checks (`yt-dlp --version`,
token identity) → `[STARTED] >: gateway >: ready as ...`.

- **Stop** sends Ctrl-C → the bot leaves voice, checkpoints `bot.db`, exits.
- `bot.db`, `config.toml`, and logs live in the server dir — back those up /
  edit config.toml in the panel file manager, then restart.

## Updating

Push a commit → Actions builds → create/upload a new release asset → set the
variable `KOAAI_DOWNLOAD_URL` to the new URL → **Reinstall** (or just SFTP
the new binary over `koaai`) → restart. 24/7 channels are rejoined
automatically on boot.

## Gotchas

- Don't upload locally-built binaries unless your machine matches the egg's
  glibc — the CI build on ubuntu-22.04 is the safe one.
- yt-dlp breaks regularly; if playback dies with errors, **Reinstall** the
  server (re-runs pip install --pre) or update yt-dlp manually.
- RAM: ~30–80 MB idle; seek-cache adds ~2 MB/min of cached track while
  seeking is used.
