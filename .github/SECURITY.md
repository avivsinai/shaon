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

## Security Posture

`shaon` is provided "AS IS" under the MIT License with no warranty of security or fitness for any particular purpose. The notes below describe the current implementation's intent, not guarantees.

- For interactive macOS / Linux use, the default credential path stores the Hilan password and a per-install master key in the OS keychain (`shaon-cli` service); `SHAON_PASSWORD` and `SHAON_MASTER_KEY` are headless escape hatches that bypass the keychain.
- The shipped configuration parser does not require a plaintext password in `config.toml`. Operators may still place secrets in environment variables, shell history, or other files outside `shaon`'s control.
- HTTPS requests use `reqwest` with `rustls-tls` and standard certificate validation. Network operators, custom root stores, and MITM tooling on the host are out of scope.
- On macOS, `scripts/run.sh` and release builds codesign the binary with the stable identifier `io.github.avivsinai.shaon` and an explicit identifier-based designated requirement to keep Keychain ACLs stable across rebuilds and upgrades; this does not provide tamper resistance.
- CI runs gitleaks secret scanning and `cargo-deny` dependency auditing on a best-effort basis.

These measures reduce common accidental-exposure risks; they do not make the tool resistant to a determined local attacker, malicious agents, or compromised hosts.
