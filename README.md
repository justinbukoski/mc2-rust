# MC2 Rust

A port of the 2001 game MechCommander 2, written in Rust, for macOS, Linux (including Steam Deck) and Windows. You need your own copy
of the original game's data to play; game data is not included.

## Download and play

You do not need to compile anything. Ready-to-run builds are on the
[release page](https://github.com/justinbukoski/mc2-rust/releases/tag/v0.1.0):

| System | Download |
| --- | --- |
| macOS (Apple Silicon) | [mc2-rust-0.1.0-5dcc8c14c146-macos-arm64.zip](https://github.com/justinbukoski/mc2-rust/releases/download/v0.1.0/mc2-rust-0.1.0-5dcc8c14c146-macos-arm64.zip) |
| Linux x86_64 / Steam Deck | [mc2-rust-0.1.0-5dcc8c14c146-linux-x86_64.tar.gz](https://github.com/justinbukoski/mc2-rust/releases/download/v0.1.0/mc2-rust-0.1.0-5dcc8c14c146-linux-x86_64.tar.gz) |
| Windows x86_64 | [mc2-rust-0.1.0-5dcc8c14c146-windows-x86_64.zip](https://github.com/justinbukoski/mc2-rust/releases/download/v0.1.0/mc2-rust-0.1.0-5dcc8c14c146-windows-x86_64.zip) |

Checksums: [SHA256SUMS](https://github.com/justinbukoski/mc2-rust/releases/download/v0.1.0/SHA256SUMS).

Quick start:

1. Download and unpack the build for your system.
   - macOS: move `MC2 Rust.app` anywhere. It is not notarised, so the first time right-click it and choose Open.
   - Linux: `tar xzf mc2-rust-0.1.0-*-linux-x86_64.tar.gz`, then run `./mc2-rust.sh` in the unpacked folder.
   - Windows: unzip and run `mc2.exe`.
2. Point the game at your original game data folder (the one containing `system.cfg` and the `.fst` files). The simplest way is a
   file named `mc2.cfg` next to the program containing one line: `data = /path/to/your/game/data`. That folder must also contain the
   game's string library (`mc2res.dll` from a retail install).
3. Start the game.

Full instructions, including the other ways to set the data folder, how to check an install, and command-line options:
[INSTALL.md](docs/release/INSTALL.md). Release notes: [RELEASE-NOTES-0.1.0.md](docs/release/RELEASE-NOTES-0.1.0.md).
Known differences from the original: [KNOWN-DIFFERENCES.md](docs/release/KNOWN-DIFFERENCES.md). Controls:
[the player guide](packaging/README-PLAYERS.md).

The Windows build has been tested under Proton only; the retail `mc2res.dll` path has been tested against a synthetic fixture only.
Reports welcome in Issues.

## Building from source

Optional. With rustup installed (it fetches the pinned Rust 1.98.0 from `rust-toolchain.toml`): `cargo build --release -p mc2-app`, then run `target/release/mc2 --data "/path/to/data"`.

## Licence

The source and the binaries are under the Microsoft Shared Source Limited Permissive License (Ms-SSLPL); see
[LICENSE-MS-SSLPL.txt](LICENSE-MS-SSLPL.txt), [NOTICE](NOTICE) and the third-party crate licences in the packages
(`THIRD-PARTY-NOTICES.txt`).

## Credits

- Microsoft Corporation released the MechCommander 2 source under the Ms-SSLPL; this port is derived from that release.
- alariq's port of the same source release was used as a behaviour reference only.

Not affiliated with or endorsed by Microsoft, FASA, Catalyst Game Labs, Topps, or alariq. Requires your own copy of the original
game data.
