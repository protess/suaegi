# macOS release

Suaegi ships separate notarized artifacts for Apple Silicon and Intel Macs.
Pushing a tag that exactly matches the workspace version (`v0.1.0`, for
example) builds both architectures, signs the app with Hardened Runtime,
notarizes and staples the app and DMG, and publishes DMG, ZIP, and SHA-256 files
to a GitHub Release.

## Required repository secrets

Configure these Actions secrets before creating a release tag:

- `APPLE_CERTIFICATE_P12_BASE64`: base64-encoded Developer ID Application P12.
- `APPLE_CERTIFICATE_PASSWORD`: password protecting that P12.
- `APPLE_SIGNING_IDENTITY`: full identity, such as
  `Developer ID Application: Example Company (TEAMID1234)`.
- `APPLE_API_KEY_ID`: App Store Connect team API key ID.
- `APPLE_API_ISSUER_ID`: App Store Connect team API issuer UUID.
- `APPLE_API_PRIVATE_KEY_BASE64`: base64-encoded `.p8` private key.

The workflow imports the certificate into an ephemeral Keychain and writes the
notary key only under `RUNNER_TEMP`. Neither credential is uploaded as an
artifact. Use a Team API key with the minimum access needed for notarization.

## Release procedure

1. Update `[workspace.package].version` and `Cargo.lock`, then merge that change
   to `main` with green CI.
2. Create and push the matching tag, for example `v0.1.0`.
3. Wait for the **macOS Release** workflow. It must succeed for both `arm64` and
   `x86_64` before the publish job creates the GitHub Release.
4. Download each DMG and verify its adjacent `.sha256` entry before announcing
   the release.

An existing tag can be retried with the workflow-dispatch `tag` input. The
workflow refuses mismatched version tags and refuses production packaging when
any signing or notarization credential is absent.

## Local package smoke test

On a Mac, `sh scripts/package-macos-release.sh` creates an ad-hoc-signed DMG,
ZIP, and checksum in `dist/`. This validates the bundle and disk-image layout,
but it is not distributable production output. Set the signing and notary
environment variables used by the script only in a protected CI environment;
production mode is enforced with `SUEGI_REQUIRE_NOTARIZATION=1`.
