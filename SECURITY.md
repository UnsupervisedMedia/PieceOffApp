# Security policy

## Reporting a vulnerability

Please report vulnerabilities **privately** through GitHub's
[private vulnerability reporting](https://github.com/UnsupervisedMedia/PieceOffApp/security/advisories/new).
Do not open a public issue for security problems.

Include the affected version or commit, what an attacker can do, and steps to
reproduce. You can expect an acknowledgement within 7 days. This is a
volunteer-maintained project, so fix timelines depend on severity and
complexity; we will credit reporters in the advisory unless asked not to.

## Scope

In scope: the room server (WebSocket protocol, room/invite/upload-token
handling, media transfer, persistence), the `.pieceoff` package parser, and the
desktop client's handling of untrusted network or file input.

Out of scope: vulnerabilities in FFmpeg or other third-party dependencies
themselves (report those upstream), and self-hosted deployments that ignore the
guidance in `docs/DEPLOYMENT.md`.

## Supported versions

Only the latest release and `main` receive security fixes.
