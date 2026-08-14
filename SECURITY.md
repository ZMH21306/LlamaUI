# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| v0.6.x  | ✅                 |
| v0.5.x  | ⚠️ Security fixes only |
| < 0.5.0 | ❌                 |

## Reporting a Vulnerability

**Please do not file a public GitHub issue.**

Report security vulnerabilities via:

- **GitHub Private Vulnerability Reporting**: https://github.com/ZMH21306/LlamaUI/security/advisories/new
- Or email the maintainer directly.

Please include:

1. Description of the vulnerability
2. Steps to reproduce
3. Impact assessment (if known)
4. Suggested fix (optional)

We aim to respond within **72 hours** and publish a fix in the next patch release.

## Current Security Measures

- **RCE Protection**: Command whitelist validates the executable name and file path
- **Job Object Isolation**: Child processes are killed when the parent exits
- **URL Scheme Whitelist**: Only `http://` and `https://` allowed for external links
- **PATH Injection Defense**: Rejects executables from world-writable directories (`tmp/`, `temp/`, `downloads/`)
- **Memory Safety**: Log buffer capped at 5000 lines / 16KB per line; bounded channel with backpressure

## Dependencies

We monitor dependencies via [cargo audit](https://github.com/rustsec/rustsec). Known advisories:

- **RUSTSEC-2026-0221** (`event-listener`): Patched in v5.4.2 (locked in this release).
- **GTK/glib advisories**: Warning-level only (unmaintained upstream crates); no known exploitable impact on LlamaUI.

Run `cargo audit` locally to check for any new advisories.
