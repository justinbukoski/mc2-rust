# MC2 Rust

MC2 Rust is an all-Rust port of the 2001 game MechCommander 2. It is based on the shared-source release of that game that Microsoft
published in 2006, and it plays the original game's single-player campaign and tutorials. Multiplayer is not ported (see
[KNOWN-DIFFERENCES.md](KNOWN-DIFFERENCES.md), D-22).

**Status: 0.1.0.** The release notes are in [RELEASE-NOTES-0.1.0.md](RELEASE-NOTES-0.1.0.md).

## Platforms

| Platform | Package |
|---|---|
| Linux x86_64 | `.tar.gz` |
| Windows x86_64 | `.zip` |
| macOS arm64 (Apple silicon, macOS 12 or later) | `.zip` containing `MC2 Rust.app` |

The macOS and Linux packages were smoke-tested on those systems. The Windows package was run only under Proton (GE-Proton11-9) on Linux, not
on Windows itself.

## You need your own game data

The packages do not contain any game data. You need your own copy of the original 2001 MechCommander 2 data (the folder that holds
`system.cfg` and the `.fst` archives). See [INSTALL.md](INSTALL.md) for how to point the game at it.

On the first start the game builds a 2x-upscaled texture pack from your data into your cache folder. This can be switched off
(`--hires-dir off`). See [INSTALL.md](INSTALL.md).

## Install and run

[INSTALL.md](INSTALL.md) covers unpacking on each platform, telling the game where your data is, and checking that the install works.

## Build from source

Install Rust (the toolchain named in `rust-toolchain.toml`), then build from the repository root:
`cargo build --release --locked -p mc2-app --bin mc2`. The program is `target/release/mc2`. `cargo test --workspace --locked` runs the
tests; the graphics tests need a GPU, and the tests that read game data need `MC2_DATA` set to your data folder.

## Differences from the original

[KNOWN-DIFFERENCES.md](KNOWN-DIFFERENCES.md) lists the differences a player can notice.

## Credits

- Microsoft, for the 2006 shared-source release of MechCommander 2, on which this port is based.
- alariq, whose Linux port of that source is the reference port. It is used as a behaviour reference only.

Not affiliated with or endorsed by Microsoft, FASA, Catalyst Game Labs, Topps, or alariq. Requires your own copy of the original game data.

## Licence

MC2 Rust, source and binaries, is distributed under the Microsoft Shared Source Limited Permissive License (Ms-SSLPL). The full text is in
[LICENSE-MS-SSLPL.txt](../../LICENSE-MS-SSLPL.txt) (also included in every package).
