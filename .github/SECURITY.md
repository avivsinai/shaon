# Security Policy

## Reporting a Vulnerability

If you discover a security vulnerability in shaon, please report it responsibly.

**Do NOT open a public GitHub issue for security vulnerabilities.**

Instead, please email: **aviv.s@taboola.com**

Include:
- Description of the vulnerability
- Steps to reproduce
- Potential impact
- Suggested fix (if any)

You should receive a response within 48 hours. We will work with you to understand and address the issue before any public disclosure.

## Scope

This policy applies to:
- The shaon CLI binary and all source code in this repository
- Credential handling (keychain storage, config files)
- Network communication with Hilan servers
- MCP server mode

## Supported Versions

| Version | Supported |
|---------|-----------|
| 1.x     | Yes       |
| < 1.0   | No        |

## Security Measures

- Credentials stored in OS keychain (macOS Keychain, Linux Secret Service, Windows Credential Manager)
- No plaintext passwords in config files (migration path available)
- All HTTP traffic over TLS (rustls, no cert bypass)
- Binary ad-hoc codesigned on macOS for keychain access
- Gitleaks secret scanning in CI
- `cargo-deny` dependency auditing
