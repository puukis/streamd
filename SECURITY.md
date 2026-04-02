# Security Policy

## Supported Versions

Security fixes are applied on a best-effort basis to the active development line and the latest prerelease tag.

| Version | Status |
|---|---|
| `main` | supported |
| latest prerelease tag | supported |
| older tags | unsupported |

## Reporting A Vulnerability

Do **not** open a public issue for a security-sensitive report.

Preferred path:

1. Use GitHub's private vulnerability reporting flow from the repository's Security tab.
2. If that option is unavailable in the UI, open a GitHub Discussion titled `security-contact-request` without technical details so a private channel can be arranged.

Include:

* affected host and client versions or commit SHAs
* impact and attack prerequisites
* reproduction steps or proof of concept
* any suggested mitigation if you already have one

## Current Security Posture

streamd is still alpha software. The current trust model is intentionally minimal and assumes you control the network boundary. Until stronger authentication lands, prefer LAN use or tightly restricted WAN exposure.
