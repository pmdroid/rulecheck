---
title: Install from release archives
description: Download a checksummed Rulecheck CLI archive for Linux or macOS, directly or with mise.
---

Each `rust-vX.Y.Z` release on GitHub carries native archives of the full CLI, so you can install Rulecheck without a Rust toolchain. The archive holds the same two executables that `cargo install --locked rulecheck` builds: `rulecheck` and its evaluator, `rulecheck-evaluator`. You still need Git and Node.js 20 or later with npm. On the first rule command, Rulecheck installs its pinned TypeScript compiler into `$XDG_CACHE_HOME/rulecheck` or `~/.cache/rulecheck`, as the [quickstart](../quickstart/) describes.

## Targets and file names

Archives are built and smoke-tested on the platforms Rulecheck CI runs on:

| Platform | Rust target | Archive | mise platform |
| --- | --- | --- | --- |
| Linux, x86-64 (glibc) | `x86_64-unknown-linux-gnu` | `rulecheck-X.Y.Z-x86_64-unknown-linux-gnu.tar.gz` | `linux-x64` |
| macOS, Apple silicon | `aarch64-apple-darwin` | `rulecheck-X.Y.Z-aarch64-apple-darwin.tar.gz` | `macos-arm64` |

The Linux build runs on GitHub's `ubuntu-latest` image, so it needs a glibc at least that new. Other platforms, including Intel macOS, Linux on Arm and musl, have no archive yet. Use `cargo install` there.

Each release has these assets:

- `rulecheck-X.Y.Z-TARGET.tar.gz`, which unpacks to `rulecheck-X.Y.Z-TARGET/` with `bin/rulecheck`, `bin/rulecheck-evaluator`, `LICENSE`, `README.md` and `PROVENANCE.json`.
- `rulecheck-X.Y.Z-TARGET.tar.gz.sha256`, the SHA-256 of that one archive.
- `SHA256SUMS`, the SHA-256 of every archive in the release.

Keep `rulecheck` and `rulecheck-evaluator` in the same directory. The frontend runs the evaluator that sits beside it.

## Provenance

The release workflow builds only when the release tag matches the `Cargo.toml` version and resolves to a commit on `main`. It refuses to upload if the tag has moved since the build, and it never replaces an asset that already exists. `PROVENANCE.json` in each archive records the tag, the full commit SHA, the target, the `rustc` version, the SHA-256 of `Cargo.lock` and the SHA-256 of every packaged file. Archives are reproducible: entries are sorted, owned by `0:0` and dated with the commit time, so rebuilding the same commit with the same toolchain gives the same bytes.

## Direct download

Pin the checksum you copied from the release's `SHA256SUMS`, not one you download in the same step. This example installs into `~/.local/bin` on Linux x86-64:

```sh
version=X.Y.Z
target=x86_64-unknown-linux-gnu
sha256=PASTE_THE_ARCHIVE_SHA256_FROM_SHA256SUMS
archive="rulecheck-$version-$target.tar.gz"

curl -fsSLO "https://github.com/pmdroid/rulecheck/releases/download/rust-v$version/$archive"
echo "$sha256  $archive" | sha256sum -c -      # on macOS: shasum -a 256 -c -
tar -xzf "$archive"
install -m 0755 "rulecheck-$version-$target/bin/rulecheck" \
  "rulecheck-$version-$target/bin/rulecheck-evaluator" ~/.local/bin/
rulecheck --version
```

A checksum mismatch makes the check exit non-zero before anything is unpacked.

## mise

mise's `http` backend downloads the archive, checks the pinned SHA-256 and puts `bin/` on `PATH`. Add this to the repository's `mise.toml`, with one entry for each platform you use:

```toml
[tools."http:rulecheck"]
version = "X.Y.Z"
bin_path = "bin"
strip_components = 1

[tools."http:rulecheck".platforms]
linux-x64 = { url = "https://github.com/pmdroid/rulecheck/releases/download/rust-vX.Y.Z/rulecheck-X.Y.Z-x86_64-unknown-linux-gnu.tar.gz", checksum = "sha256:LINUX_X64_SHA256" }
macos-arm64 = { url = "https://github.com/pmdroid/rulecheck/releases/download/rust-vX.Y.Z/rulecheck-X.Y.Z-aarch64-apple-darwin.tar.gz", checksum = "sha256:MACOS_ARM64_SHA256" }
```

Then run `mise install` and `mise exec -- rulecheck --version`. mise stops with a checksum error if the download doesn't match. When you upgrade, change the version, both URLs and both checksums together.

The archives are public release assets and need no token. Installing from a package registry is a separate channel: crates.io via `cargo install`, documented in the [quickstart](../quickstart/).
