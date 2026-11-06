# Installing MC2 Rust 0.1.0

You need your own copy of the original game data. The packages do not include it. "The data folder" below means the folder that
contains `system.cfg` and the `.fst` archives.

Package file names look like `mc2-rust-0.1.0-<commit>-<platform>.<ext>` (for example `...-linux-x86_64.tar.gz`). Each unpacks into a
folder with the same name, containing the program, `README-PLAYERS.md`, `mc2.cfg.example`, `VERSION`, `THIRD-PARTY-NOTICES.txt` and
`LICENSE-MS-SSLPL.txt`.

## Unpack

### Linux x86_64

```sh
tar xzf mc2-rust-0.1.0-<commit>-linux-x86_64.tar.gz
cd mc2-rust-0.1.0-<commit>-linux-x86_64
./mc2-rust.sh --version
```

`mc2-rust.sh` starts the `mc2` program in the same folder and passes all arguments through; running `./mc2` directly is the same.
You need a Vulkan driver and ALSA (`libasound.so.2`). The package was built on Fedora, so it needs a glibc as new as that one or newer.

### Windows x86_64

Unzip the archive and run `mc2.exe` from the unpacked folder (for example from a command prompt: `mc2.exe --version`). You need a graphics
driver with Vulkan or DirectX 12 support. If the archive contains any `.dll` files, keep them next to `mc2.exe`.

### macOS arm64

Unzip the archive and move `MC2 Rust.app` anywhere. The app is signed with an ad-hoc signature only. It is not notarised, so Gatekeeper
will refuse a double-click the first time. Do either of these once:

- right-click the app and choose Open, then confirm; or
- in Terminal: `xattr -dr com.apple.quarantine "MC2 Rust.app"`.

To use the command-line options below, run the bundled program from Terminal:
`"MC2 Rust.app/Contents/MacOS/mc2" --version`.

## Point it at your game data

The game uses the first of these that is set:

1. the command line: `mc2 --data "/path/to/data"`
2. the environment variable `MC2_DATA`
3. a line `data = /path/to/data` in a file named `mc2.cfg`. The game looks for it in this order: next to the program (for the macOS app, next
   to `MC2 Rust.app`), then in the user config folder:
   - macOS: `~/Library/Application Support/mc2/mc2.cfg`
   - Linux: `$XDG_CONFIG_HOME/mc2/mc2.cfg`, or `~/.config/mc2/mc2.cfg` when `XDG_CONFIG_HOME` is not set
   - Windows: `%APPDATA%\mc2\mc2.cfg`

`mc2.cfg.example` in the package shows the format. A relative path in `mc2.cfg` is relative to the folder holding that file. A `#` starts a
comment, and the file accepts only the keys `data`, `hires`, `voices` and `art`; any other key is an error.

If no data folder is configured, the program prints `MC2 Rust game data not found.` followed by these three ways and the list of `mc2.cfg`
places it looked in, and exits without opening a window. On macOS, double-clicking the app in that state shows the same text in a dialog.
If the configured folder does not exist, it exits with an error naming the folder.

### Interface text

The game reads its interface text from your own data: the string resource library, which in a retail install is `mc2res.dll`, in your
data folder. Without it `mc2` stops with "game data incomplete" before showing any screen. The retail `mc2res.dll` path is fixture-tested
only: the tests use a synthetic library, not a retail file.

If you would rather use a different Linux build, alariq's Linux port of the source is at <https://github.com/alariq/mc2>. It is a separate project; see
its own page for what it needs.

## Check that the install works

All of these run without opening a window.

```sh
mc2 --version
mc2 --data "/path/to/data" --list-missions
mc2 --data "/path/to/data" --selftest-play
```

(On Linux use `./mc2-rust.sh` or `./mc2`; on macOS use the program inside the app as shown above; on Windows `mc2.exe`.)

- `--version` prints `mc2 0.1.0`, with the build's commit in parentheses for the packaged builds.
- `--list-missions` prints the missions found in your data, one per line. `mc2_01` should be among them.
- `--selftest-play` loads `mc2_01`, runs the simulation for 9,000 fixed frames without a window or GPU, and then, as a separate harness-driven run after that 9,000-frame smoke run, plays the mission to its win
  by its objectives. It prints the data and texture-pack paths it resolved, then `objective check: pass (...)`, and exits with status 0. If the data
  cannot be loaded, a frame fails, or the objective check fails, it prints `objective check: fail ...` (or an error) and exits non-zero.
  The release packages were checked this way when they were built, except that the Windows package was run separately under Proton on Linux.

## Start the game

Run the program with no arguments (Linux: `./mc2-rust.sh`; Windows: `mc2.exe`; macOS: open the app). Useful options:

- `--mission mc2_01` starts straight into a mission.
- `--fullscreen` borderless fullscreen; `--size 1920x1080` window size.
- `--help` prints all options.

Esc and P pause the game. Controls match the original and are listed in `README-PLAYERS.md` in the package.

### Upscaled textures

The first time the game window starts it builds a 2x-upscaled texture pack from your data into a cache folder (macOS `~/Library/Caches/mc2/hires`,
Linux `~/.cache/mc2/hires`, or `$XDG_CACHE_HOME/mc2/hires`, Windows `%LOCALAPPDATA%\mc2\hires`) and prints progress to the terminal. It spends at most 90 seconds per
start on this (`MC2_HIRES_BUDGET_SECS=N` changes the limit; `0` means no limit). Textures not yet done keep their original look, and the
next start continues. `--hires-dir off` (or `hires = off` in `mc2.cfg`, or `MC2_HIRES_DIR=off`) keeps the original art. `--selftest-play`, `--list-missions` and `--headless` never build the pack.

Saved options (`options.cfg`) live in the same user config folder as `mc2.cfg`.
