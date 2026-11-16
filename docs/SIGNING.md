# Windows code signing

Unsigned Windows executables trigger SmartScreen ("Windows protected your PC")
no matter how polished the package is. The release ships an icon, version
metadata, a GUI-subsystem app (no console window), a `PieceOff.exe` launcher
and an installer; signing is the remaining step.

## Plan: SignPath Foundation (free for open source)

PieceOff is public and Apache-2.0, which is what the
[SignPath Foundation](https://signpath.org/) program requires.

1. Apply at <https://signpath.org/apply> with the repository URL, the
   `LICENSE`, and a link to `CONTRIBUTING.md` / `SECURITY.md`. Approval takes
   a few weeks.
2. Once approved, create a project and a `release-signing` policy in SignPath,
   connect the GitHub repository (trusted build system), and add the
   repository secret `SIGNPATH_API_TOKEN` and variables
   `SIGNPATH_ORGANIZATION_ID`, `SIGNPATH_PROJECT_SLUG`.
3. Wire `release.yml` (not done yet, because the artifact configuration must
   match what SignPath creates): upload the unsigned `PieceOff.exe`,
   `pieceoff-app.exe` and `pieceoff-server.exe`, submit them with
   `signpath/github-action-submit-signing-request`, build the installer from
   the signed files, then submit the installer for signing too. Pin the action
   by commit SHA like the other workflow steps.

SignPath certificates are issued to the SignPath Foundation, so the publisher
shown by Windows is "SignPath Foundation", and SmartScreen reputation builds
over the first downloads.

## Until then

`START-HERE.txt` tells testers what the SmartScreen prompt means and how to
verify the download against `SHA256SUMS.txt`. Releases also carry GitHub build
provenance attestations (`gh attestation verify`).
