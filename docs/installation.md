# Installation

[README](../README.md) · [Commands](commands.md) · [JSON](json.md)

For v8 migration steps, see [Upgrading to v8](upgrading-to-v8.md). Breaking changes remain listed in [CHANGELOG](../CHANGELOG.md).

Install the latest stable release from GitHub Releases:

```sh
curl -fsSL https://github.com/yoiailabs/rabi/releases/latest/download/install.sh | sh
```

The installer always verifies SHA-256. Use `sh -s -- --verify-attestation` to also verify artifact provenance (requires `gh`). Without a version pin, the installer selects the newest stable release with a `rabi-` archive for the platform, skipping v6.

For agents and CI, pin the installer and binary to the same release tag:

```sh
curl -fsSL https://github.com/yoiailabs/rabi/releases/download/v8.0.0/install.sh | RABI_VERSION=v8.0.0 sh
```

To pin another release, replace `v8.0.0` in both places with a tag from the [Releases page](https://github.com/yoiailabs/rabi/releases); rabi releases start at v7.0.0-alpha.10. v7.0.0-alpha.9 and v7.0.0-alpha.8 must each be installed with that release's own `install.sh` at `https://github.com/yoiailabs/rabi/releases/download/<TAG>/install.sh` (with `<TAG>` set to that release), using its original options and environment variables; v7.0.0-alpha.7 and older have no installer, so install them manually as described below. Pinned installs allow prereleases and make no GitHub API request.

To opt into newer prereleases when installing without a version pin, pass `--prerelease`:

```sh
curl -fsSL https://github.com/yoiailabs/rabi/releases/latest/download/install.sh | sh -s -- --prerelease
```

Pass installer options with `sh -s -- [options]`; flags override environment variables.

| Option | Environment variable | Behavior |
|---|---|---|
| `--version <TAG>` | `RABI_VERSION` | Exact release tag, with or without the leading `v` |
| `--prerelease` | `RABI_PRERELEASE=1` | Allow the newest prerelease when unpinned |
| `--install-dir <DIR>` | `RABI_INSTALL_DIR` | Install directory (default `~/.local/bin`) |
| `--verify-attestation` | `RABI_VERIFY_ATTESTATION=1` | Also verify GitHub artifact provenance; requires `gh` and aborts if verification fails |

`GITHUB_TOKEN` is optional, only for GitHub API rate limits on unpinned installs; it is not sent to asset downloads. The installer needs `curl` and `tar`; on Alpine, run `apk add curl` first. It never prompts, uses no sudo, and does not edit shell profiles; it prints a PATH hint if needed. It always verifies SHA-256 and replaces the binary atomically. It refuses to replace a symlinked `rabi` managed by another tool; update with that tool or choose another install directory.

Attestations for v7.0.0-alpha.10 and earlier remain under the old `phantompane` organization after the repository transfer. The current installer verifies against `yoiailabs/rabi` without a fallback: v7.0.0-alpha.10 downloads and passes SHA-256 verification, but fails with `--verify-attestation` (or `RABI_VERIFY_ATTESTATION=1`). Its release-pinned installer verifies against `phantompane/phantom`, which also fails after the transfer. When installing v7.0.0-alpha.10, omit `--verify-attestation` and leave `RABI_VERIFY_ATTESTATION` unset. To verify these older artifacts manually, use `gh attestation verify <archive> --owner phantompane`.

Release binaries support Linux x64/arm64 (static musl builds for any distribution, including Alpine) and macOS x64/arm64. Windows is not supported; use WSL with the Linux binaries.

## Other installation methods

With mise, pin v8.0.0 or a later release (v7.0.0-alpha.10 and later prereleases also work). The v7 releases share the same repository as the older v6 TypeScript releases, so keep the version pin explicit.

```sh
mise use github:yoiailabs/rabi@<VERSION>
```

Or build from source with Rust 1.89+ (the `rust-version` in `Cargo.toml`):

```sh
cargo install --git https://github.com/yoiailabs/rabi --tag v8.0.0 --locked rabi
```

For a manual install, download `rabi-v<VERSION>-<os>-<arch>.tar.gz` from Releases (`<VERSION>` omits the leading `v`; `<os>` is `linux` or `darwin`, and `<arch>` is `x64` or `arm64`). Some older releases use archive names such as `mabo-v<VERSION>-<os>-<arch>.tar.gz`, `maboroshi-v<VERSION>-<os>-<arch>.tar.gz` or `ptm-v<VERSION>-<os>-<arch>.tar.gz`; for those, download the archive listed on that release. `install.sh` supports only the `rabi-` archive name and requires exactly one valid entry for it in `SHA256SUMS`. Each current archive contains only `rabi` (older archives contain the binary named at that release); extract it and place it in a directory on PATH. For releases after v7.0.0-alpha.7, download `SHA256SUMS` from the same release and verify before extracting. For v8.0.0 on Linux x64, verify provenance with `gh` and check SHA-256 in the download directory (adapt the archive name for other platforms or releases):

```sh
gh attestation verify rabi-v8.0.0-linux-x64.tar.gz --repo yoiailabs/rabi
grep " rabi-v8.0.0-linux-x64.tar.gz$" SHA256SUMS | sha256sum -c -
```

`SHA256SUMS` covers all four archives and `install.sh`; the `grep` selects the line for the archive you downloaded, which works with GNU and BusyBox `sha256sum`. On macOS, use `shasum -a 256 -c -` instead of `sha256sum -c -`. v7.0.0-alpha.7 and older releases have no `SHA256SUMS` or artifact attestations.

## Updating

Re-run the main installer command above to update to the newest stable release, or update the tag in both places in the pinned command. mise users update their version pin with mise; Cargo users rerun the source install command with the new tag; manual users replace the binary with the newly downloaded and verified one. `rabi` has no self-update command and does not check for updates or make network calls for update checks.
