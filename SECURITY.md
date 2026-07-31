# Security Policy

## Supported Versions

| Version | Supported |
|---------|-----------|
| 0.20.x  | ✅ |
| < 0.20  | ❌ |

## Reporting a Vulnerability

**Email:** [a7garden@icloud.com](mailto:a7garden@icloud.com)

**Do NOT file a public issue for security vulnerabilities.**

We aim to respond to security reports within 48 hours and provide a fix within
7 days for critical issues.

### What to Include

- Description of the vulnerability
- Steps to reproduce
- Potential impact
- Suggested fix (if any)

## Security Features

- API keys are wrapped in `Secret<T>` (masked in `Debug`/`Display` output)
- Extension loading requires manifest validation + checksum verification
- Path traversal prevention on all file access tools (`PathGuard`)
- Dynamic library sandboxing with panic isolation
- Settings validation at startup to prevent runtime panics
- All file writes use atomic temp+rename to prevent corruption
- SSE parser handles partial UTF-8 lines safely

## Dependency Auditing

We use `cargo audit` and `cargo deny` to continuously monitor for known
vulnerabilities. The CI pipeline runs security audits on every push.

Known suppressed advisories are documented in `deny.toml` with justification.
