# tbard

**Audiobook manager and listener for the terminal.**

tbard keeps your audiobook library in one place and plays it from the
terminal. It connects to your Audible account to list the books you own,
downloads them in the best available quality split into one file per chapter,
plays them with mpv, and keeps your listening position in sync with the
Audible app on your phone.

- Library with fuzzy search, sections and cover art (Kitty/Ghostty graphics
  or half blocks).
- Downloads in the best quality available, losslessly split by chapter
  (`.m4b` with title, track number and cover).
- Player with chapters, ±30 s, speed 0.5–3×, volume and a sleep timer.
- Two-way position sync (Whispersync). Offline playback with a queue that
  sends positions when the connection comes back.
- English and Spanish interface.
- Linux, macOS and Windows.

tbard is inspired by [Libation](https://github.com/rmcrackan/Libation) and
similar projects: it brings Libation's idea of a personal, DRM-free audiobook
library to the terminal, together with a player and position sync. See
[Inspiration and thanks](#inspiration-and-thanks).

> **Disclaimer.** tbard is an unofficial tool, not affiliated with Audible or
> Amazon. It talks to Audible's private API, which may change at any time.
> Use it only for books you have bought, for your personal listening. Removing
> DRM may be restricted where you live; you are responsible for how you use it.

## Install

tbard needs **mpv** (playback) and **ffmpeg** (decryption and splitting) on
your `PATH`. Package managers below install them for you; otherwise install
them yourself.

| Platform | Command |
|---|---|
| macOS / Linux (Homebrew) | `brew install abelland-lgtm/tap/tbard` |
| Arch Linux (AUR) | `yay -S tbard-bin` |
| Windows (Scoop) | `scoop bucket add extras; scoop bucket add tbard https://github.com/abelland-lgtm/scoop-bucket; scoop install tbard` |
| Any Linux / macOS | `curl --proto '=https' --tlsv1.2 -LsSf https://github.com/abelland-lgtm/tbard/releases/latest/download/tbard-installer.sh \| sh` |
| Windows (PowerShell) | `powershell -ExecutionPolicy Bypass -c "irm https://github.com/abelland-lgtm/tbard/releases/latest/download/tbard-installer.ps1 \| iex"` |
| Rust toolchain | `cargo install tbard --locked` |
| Nix | `nix run github:abelland-lgtm/tbard` |

Prebuilt binaries for every platform are on the
[releases page](https://github.com/abelland-lgtm/tbard/releases).

Installing mpv and ffmpeg by hand:

- Debian/Ubuntu: `sudo apt install mpv ffmpeg`
- Fedora: `sudo dnf install mpv ffmpeg` (ffmpeg from RPM Fusion)
- Arch: `sudo pacman -S mpv ffmpeg`
- macOS: `brew install mpv ffmpeg`
- Windows: `scoop bucket add extras; scoop install mpv ffmpeg`

Run `tbard doctor` to check that everything is in place.

## Quick start

```sh
tbard
```

The first time, tbard asks for your language and your Audible store, opens
Amazon's sign-in page in your browser and asks you to paste the address you
land on. After that it opens your library.

Everything is also available from the command line:

```sh
tbard login --locale us        # sign in (store: us, uk, ca, au, in, de, fr, it, es, jp, br)
tbard library                  # refresh and list the library
tbard play dune                # play (downloading first if needed), syncing the position
tbard download project hail    # download only
tbard sync                     # send positions queued while offline
tbard doctor                   # check mpv, ffmpeg, folders and session
tbard logout                   # remove this device from Amazon and delete the credentials
```

### Keys

| Library | | Player | |
|---|---|---|---|
| `/` | search | `space` | pause |
| `1`–`5` | section | `←` `→` | ±30 s |
| `⏎` | listen | `[` `]` | previous / next chapter |
| `d` | download | `+` `-` | speed |
| `r` | refresh | `9` `0` | volume |
| `c` | settings | `t` | sleep timer |
| `tab` | player | `s` | stop |
| `q` | quit | `tab` | back to the library |

## How syncing works

- On start, tbard uses the most recent of the local and the Audible position.
  If both changed since the last sync and they differ by more than two
  minutes, it asks which one to use.
- While playing it saves the position locally every 5 s and sends it to
  Audible every 2 minutes, on pause and when you stop.
- Without a connection, positions wait in a queue and are sent later.

## Files and privacy

tbard stores everything in your user's own folders. Nothing is shared with
other users of the computer, and nothing is sent anywhere except to Amazon and
Audible. There is no telemetry.

| What | Linux | macOS | Windows |
|---|---|---|---|
| Settings and theme (`config.toml`, `colors.toml`) | `~/.config/tbard` | `~/Library/Application Support/tbard` | `%APPDATA%\tbard\config` |
| Credentials and database | `~/.local/share/tbard` | `~/Library/Application Support/tbard` | `%APPDATA%\tbard\data` |
| Cover cache | `~/.cache/tbard` | `~/Library/Caches/tbard` | `%LOCALAPPDATA%\tbard\cache` |
| Log | `~/.local/state/tbard` | `~/Library/Application Support/tbard/logs` | `%LOCALAPPDATA%\tbard\data\logs` |
| Audiobooks | `~/Audiobooks` (configurable) | `~/Audiobooks` | `%USERPROFILE%\Audiobooks` |

**Credentials.** Signing in registers tbard as a device on your Amazon account
(like a phone with the Audible app). Its tokens and private key are saved in
`credentials.enc`, encrypted with AES-256-GCM. The encryption key is kept in
your system keychain: Keychain on macOS, Credential Manager on Windows, and
the Secret Service (GNOME Keyring, KWallet) on Linux. A copy of the file is
useless on another machine or user account. If no keychain is available (for
example a server without a desktop), tbard stores `credentials.json`
unencrypted, readable only by you, and warns about it.

**Permissions.** On Linux and macOS tbard's folders are created `0700` and its
private files `0600`. On Windows they live inside your user profile, which
only you can read.

**Logs** never contain tokens or the signed download links.

**What is stored:**
- `credentials.enc` (or `credentials.json`): your device registration.
- `tbard.db`: library cache, listening positions, per-book speed and volume.
- Cover images and the downloaded audiobooks.

**Removing everything:** run `tbard logout` (removes the device from your
Amazon account and deletes the credentials and the keychain entry), then
delete the folders above. You can also remove the device from Amazon under
*Account → Content and Devices → Devices*.

## Configuration

`config.toml` in the settings folder. The settings screen (`c`) writes it for
you.

```toml
library_dir = "~/Audiobooks"   # where new downloads go
language = "en"                # "en" or "es"
mpv_path = "/opt/mpv/bin/mpv"  # only if mpv is not on PATH
ffmpeg_path = "C:\\tools\\ffmpeg.exe"
```

Set `TBARD_LOG=debug` for a more detailed log.

### Colors

tbard uses your terminal's colors by default. To change them, create
`colors.toml` next to `config.toml`; tbard reloads it while running.

```toml
primary = "#bde0fe"
on_primary = "#0b344b"
secondary = "#bbc8d5"
tertiary = "#f2d0fb"
error = "#ffb4ab"
on_surface_variant = "#c2c7cd"
outline = "#8c9197"
```

With [Noctalia](https://github.com/noctalia-dev/noctalia-shell), the template
in [`noctalia/tbard.toml`](noctalia/tbard.toml) generates this file from your
wallpaper's palette.

## Nix and home-manager

```nix
# flake inputs
inputs.tbard.url = "github:abelland-lgtm/tbard";

# home-manager
imports = [ inputs.tbard.homeManagerModules.default ];
programs.tbard = {
  enable = true;
  settings.library_dir = "~/Audiobooks";  # optional; makes config.toml read-only
};

# Noctalia
theme.templates.user.tbard = {
  input_path = "${inputs.tbard}/noctalia/tbard.toml";
  output_path = "$XDG_CONFIG_HOME/tbard/colors.toml";
};
```

## Development

```sh
nix develop          # or install Rust, mpv and ffmpeg yourself
cargo test
cargo clippy --all-targets -- -D warnings
cargo deny check licenses
nix flake check
```

[`docs/audible-api.md`](docs/audible-api.md) documents the Audible API as
tbard uses it.

## Inspiration and thanks

tbard exists because of projects that showed what an open audiobook library
can be:

- [**Libation**](https://github.com/rmcrackan/Libation): the main inspiration.
  A desktop app that backs up your Audible library as DRM-free files; tbard
  follows the same idea in the terminal.
- [audible](https://github.com/mkb79/Audible) and
  [audible-cli](https://github.com/mkb79/audible-cli): Python tools whose work
  documenting Audible's API made projects like this possible.
- [Toutui](https://github.com/AlbanDAVID/Toutui): a terminal audiobook player
  that inspired tbard's player.
- [mpv](https://mpv.io) and [ffmpeg](https://ffmpeg.org), which do the
  playback and the audio work.

tbard is an independent implementation: it does not contain code from these
projects.

## License

[GPL-3.0-or-later](LICENSE). Copyright (C) 2026 Abelland.
