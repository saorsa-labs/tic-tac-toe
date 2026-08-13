# macOS release packaging

Run the distribution entrypoint from an Apple Silicon Mac:

```bash
scripts/dist.sh
```

This builds and signs `tic-tac-toe.app` once, notarizes it when credentials are
configured, and publishes both a portable tarball and a signed DMG under
`dist/`. For local two-machine acceptance only, use
`scripts/dist.sh --skip-notarization`; that output is intentionally not a
public release candidate.

`x0xd` is **never cargo-built for distribution**. `dist.sh` fetches the
official signed `v0.37.2` macos-arm64 asset, checks the tarball and extracted
binary sha256 pins in `scripts/sidecar-validation.sh`, and refuses campaign
protocol strings (`recipient_ack_semantics_unavailable` and siblings — ttt
#12). `just sidecar-campaign-denylist-test` and `just mixed-version-dm-smoke`
are the local gates.

The DMG is created by `scripts/package-macos-dmg.sh` from the already-signed app
bundle. Do not run `tauri build --bundles dmg` as a follow-up: it performs a
second bundling pass and can replace the accepted app with a stale or incomplete
bundle. The packager fails closed unless all of these receipts agree:

1. `Contents/Resources` and the five shipped executables exist.
2. The source app passes deep, strict code-signature verification.
3. Every regular file in the staged app has the same SHA-256 as the source app.
4. The app mounted from the compressed, signed DMG has that same manifest and
   still passes deep, strict signature verification.
5. The DMG passes `codesign --verify` and `hdiutil verify` before publication.

The packager prints SHA-256 receipts for `buzz-desktop` and every
`bundle.externalBin` basename from `desktop/src-tauri/tauri.conf.json`
(`x0xd`, `buzz-acp`, `buzz-agent`, `buzz-x0x-mcp`). Preserve those
values with the release receipt and compare them with the installed binaries
during two-machine acceptance. A name that is not on `externalBin` is a
packaging bug — `portable-package-contract.test.mjs` rejects that drift.

For non-interactive CI, `DMG_SKIP_FINDER_LAYOUT=1` skips only the Finder window
metadata step. It does not skip source/mounted manifest comparison, signature
verification, or disk-image verification.

Run the packaging contract tests with:

```bash
just package-macos-dmg-test
node --test scripts/portable-package-contract.test.mjs
```

## Keychain namespace (v0.5.1)

Release builds use `com.saorsalabs.tictactoe` (`desktop/src-tauri/src/app_state_keyring.rs`).
They do **not** read or copy secrets stored under the inherited `buzz-desktop`
service. v0.5.0 was team-only; migrating those items would restore the
Finder ACL prompt this change exists to eliminate. Debug builds still use
`buzz-desktop-dev`.

To validate an existing signed app without creating a DMG:

```bash
scripts/package-macos-dmg.sh \
  --app /path/to/tic-tac-toe.app \
  --validate-only
```
