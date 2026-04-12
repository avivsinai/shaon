# Security Policy

## Reporting Security Issues

If you discover a security vulnerability in shaon, please report it responsibly.

Please use [GitHub Security Advisories](https://github.com/avivsinai/shaon/security/advisories) to report vulnerabilities privately.

**Do not open public issues for security reports.**

## Scope

This policy applies to:
- The shaon CLI binary and all source code in this repository
- Credential handling (keychain storage, config files)
- Network communication with Hilan servers
- MCP server mode

## Supported Versions

| Version | Supported |
|---------|-----------|
| 0.8.x   | Yes       |
| < 0.8   | No        |

## Security Measures

- Credentials stored in OS keychain (macOS Keychain, Linux Secret Service, Windows Credential Manager)
- No plaintext passwords in config files
- All HTTP traffic over TLS (rustls, no cert bypass)
- Binary ad-hoc codesigned on macOS for keychain access
- Gitleaks secret scanning in CI
- `cargo-deny` dependency auditing
