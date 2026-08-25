<div align="center">

# 🎵 Koaai

### A single-process Discord music bot, written in Rust

*Formerly CupcakeRS — the native rewrite of the Cupcake Music Bot*

![Build](./actions/workflows/build.yml/badge.svg)
![Rust](https://img.shields.io/badge/Rust-stable-DEA584?logo=rust)
![Discord](https://img.shields.io/badge/Components%20V2-ready-5865F2?logo=discord)

</div>

---

## ✨ What is this?

Koaai collapses the classic `bot → Shoukaku → Lavalink → voice` four-hop
stack into **one binary that IS the audio engine**. No JVM, no second
process to babysit, no version drift between bot and node — just
`cargo run`.

```
┌──────────────────────────────────────────────┐
│                  koaai                        │
│                                              │
│  serenity ── gateway / REST / slash + prefix │
│    poise ──── command framework              │
│    songbird ─ DAVE/E2EE voice driver         │
│    yt-dlp ─── source extraction (subprocess) │
│    symphonia  decode · opus2  encode         │
│    rusqlite ── prefixes · 24/7 · reports     │
└───────────────────┬──────────────────────────┘
                    ▼
           Discord voice servers
```

## 🌟 Highlights

- 🧩 **Components V2 everywhere** — every message (now-playing card, queue,
  help, reports) renders as a modern CV2 container with buttons & selects
- 🔎 **Smart search chain** — name searches walk
  *YouTube Music (official catalog filter) → YouTube → Spotify → SoundCloud*
  with automatic fallback; explicit prefixes (`ytsearch:` `spsearch:`
  `scsearch:` `amsearch:`) jump straight to a platform
- 🔗 **Link support** — YouTube, Spotify (track/album/playlist via public
  embed), SoundCloud, Apple Music matches, direct file URLs & Discord CDN
- ⏯ **Full player** — seek/forward/rewind with transparent Opus-frame
  caching (first seek re-buffers once, then seeks are instant), loop modes,
  shuffle, queue management, live NP card with transport buttons
- 🛌 **24/7 mode** — lock the bot to a channel; it survives restarts via
  automatic rejoin and never auto-leaves
- 🩹 **Self-healing** — dead-player recovery with forced voice rejoin,
  error-streak circuit breaker, timestamped stop-flags that can never wedge
  playback, hourly WAL checkpoints so even a hard kill loses ≤1h of data
- 📨 **Report system** — category select → modal → saved to SQLite → owner
  gets a DM; owners browse/filter/resolve everything from a DM dashboard
- 🎛 **Per-guild prefixes**, admin-gated, applied instantly — plus graceful
  SIGTERM shutdown and zero-warning clippy

## 🚀 Getting started

### Requirements

| Need | Why |
|---|---|
| Rust stable (2024 edition) | build |
| **yt-dlp nightly ≥ 2026.08.20** | source extraction (`pip install --upgrade yt-dlp --pre`) |
| Node.js on PATH | passed as `--js-runtimes node` to every yt-dlp call |
| CMake + C compiler | builds the vendored Opus FFI |

### Run it

```bash
git clone https://github.com/Kurenaiiiii/Koaai.git && cd Koaai
cp .env.example .env        # then fill TOKEN in
cargo run --release
```

`.env` keys:

```ini
TOKEN=your-bot-token
# optional:
OWNER_ID=your-discord-id          # unlocks the report dashboard in DMs
DEV_GUILD_ID=123456789            # scope slash commands to one guild while developing
REGISTER_COMMANDS=1               # or register globally
SPOTIFY_CLIENT_ID=...             # enables spotify links & spsearch
SPOTIFY_CLIENT_SECRET=...
```

> 💡 During development keep `DEV_GUILD_ID` set — registration is instant and
> can't clobber any production bot's global commands.

## 📖 Commands

Default prefix `+`, fully renameable per server. Every command works as
slash **and** prefix.

| | Command | Aliases | Description |
|---|---|---|---|
| 🎶 | `/play` | `p` | Play a song or playlist from a name or link |
| | `/skip [n]` | `s` | Skip the current track (or several) |
| | `/stop` | | Stop, clear the queue, leave (unless 24/7) |
| | `/pause` · `/resume` | `unpause` | Playback control |
| | `/seek <time>` | | Jump to `90` or `1:30` or `1:02:30` |
| | `/forward` · `/rewind` | `ff` · `rw` | Hop ±N seconds (default 15) |
| | `/volume <0-200>` | `vol` | Set volume (100% = pure passthrough quality) |
| | `/loop` | `repeat` | Cycle off → track → queue |
| 📋 | `/queue` | `q` | Paginated queue view |
| | `/shuffle` | | Randomize the queue |
| | `/clear` | `clearqueue` | Drop every queued track |
| | `/remove <pos>` | `rm` | Remove one track |
| | `/move <from> <to>` | `mv` | Reorder the queue |
| ⚙ | `/join` | | Enable 24/7 mode in your VC |
| | `/leave` | `dc` `disconnect` | Disable 24/7 and leave |
| | `/setprefix <prefix>` | | Per-server prefix (admin only) |
| ℹ | `/ping` · `/uptime` · `/help` | | Status, runtime, browsable help |
| 🐛 | `/report` | `bug` `feedback` `reports` | Bug reports; owners get a DM dashboard |

## ⚙️ Configuration

`config.toml` is auto-generated on first boot — every key documented inline.
Highlights:

```toml
[audio]
bitrate_kbps = 256            # opus target (max 512); YouTube sources pass through untouched at 100% volume
auto_leave_secs = 300         # idle leave timer (24/7 channels are exempt)
np_delete_previous = true     # keep only the latest Now Playing card

[sources]
youtube_enabled = true        # each platform can be toggled independently
```

Secrets stay in `.env`, never in the repo.

## 🗂 Project structure

```
src/
├── main.rs        boot, self-checks, shutdown, interaction router
├── core.rs        shared state (http, db, registry, caches)
├── state.rs       Track / GuildState / registry (pure logic, unit-tested)
├── player.rs      ensure_voice, play_next, timers, recovery, seek-cache
├── sources.rs     ytmusic/yt/sc/spotify/am resolvers + playlists
├── ui.rs          Components V2 builders + button router
├── db.rs          rusqlite layer (same schema as the old Node bot)
├── settings.rs    config.toml
├── logger.rs      Nodelink-style console logging
└── commands/      music.rs · general.rs · report.rs
```

## 📚 Docs

- [`cupcake-rs-prd.md`](./cupcake-rs-prd.md) — original rewrite PRD
- [`contanerv2.md`](./contanerv2.md) — Discord Components V2 guide for serenity
- [`Changelog.md`](./Changelog.md) — full development history *(kept local, gitignored)*

---

<div align="center">
<sub>Built with serenity · poise · songbird · symphonia — single binary, zero excuses 🦀</sub>
</div>
