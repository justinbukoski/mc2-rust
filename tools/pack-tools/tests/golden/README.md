Golden outputs of the Python tools that `pack-tools` replaced, made once from the fixtures in `../fixtures`
before the scripts were deleted (the single permitted Python run; see `docs/porting-log.md`).

- `README-PLAYERS.md`, `Info.plist`: `render_product.py template|plist` with
  `PRODUCT_NAME='Second & "Product"' PRODUCT_SLUG=second-product BUNDLE_ID=org.example.second-product`, version `1.2.3`, revision `0123456789ab`.
- `notices.txt`: `gen_notices.py x86_64-unknown-linux-gnu` with a `cargo` that printed `fixtures/notices/metadata.json` (`@ROOT@` replaced).
- `archive.tar.gz`, `archive.zip`, `archive-early.zip`: `mkarchive.py tgz|zip` of `fixtures/archive/pkg` plus an empty `emptydir/`
  (mode 0755), `run.sh` 0755 and every other file 0644; epoch 1700000000, and 1000 (clamped to 1980) for `archive-early.zip`.
