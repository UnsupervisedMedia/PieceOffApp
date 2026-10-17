# Contributing

Thanks for helping improve PieceOff. Start with the product direction and
acceptance gates in [the build plan](docs/PLAN.md) and the current status in
[the feature matrix](docs/FEATURE_MATRIX.md). For local development and
multiplayer checks, see [the README](README.md) and [testing guide](docs/TESTING.md).

## Workflow

`main` is protected: all changes land through pull requests from a fork or
topic branch, with required CI passing and review conversations resolved.
Pull requests are squash-merged. Please follow the
[code of conduct](CODE_OF_CONDUCT.md), and report security issues privately as
described in [SECURITY.md](SECURITY.md).

## Changes

- Keep changes focused and explain the user-visible behavior they improve.
- For protocol, package, or persistence changes, describe compatibility and
  migration behavior.
- Keep media decoding behind `MediaDecoderBackend`; puzzle rules belong in
  `pieceoff-core` and authoritative room validation belongs on the server.
- Do not add media, fonts, artwork, or other third-party material unless its
  source and redistribution license are documented.
- Before submitting, run `scripts/check.sh` and include the result in the
  pull request description. This local gate includes formatting, Clippy,
  tests, and a Windows GNU cross-check.

## Licensing contributions

Contributions are welcome under the Apache License 2.0, the same license as
this project. Please make sure you have the right to share your contribution
under that license; if your employer or another party may own it, check with
them first. You keep the copyright to your work. There is no CLA, copyright
assignment, or required DCO sign-off. A DCO sign-off is welcome if you want to
include one. Contributions do not include a separate commercial license
grant, royalty, revenue share, or promise of compensation.

PieceOff may operate an optional paid hosted service at `pieceoff.app` under
separate service terms. That does not change the Apache-2.0 license for the
software, and others remain free to self-host under that license. The project
does not promise that the hosted service is available or that contributors
will receive a share of its revenue.

Please do not submit confidential or unlicensed code.
