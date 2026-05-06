# Security Policy

## Supported Versions

| Version | Supported |
|---------|-----------|
| 0.6.x   | ✅ |
| < 0.6   | ❌ |

## Reporting a Vulnerability

Email: security@a7garden.dev

Do NOT file a public issue for security vulnerabilities.

## Security Features

- API keys are wrapped in `Secret<T>` (masked in Debug/Display)
- Extension loading requires manifest validation + checksum
- Path traversal prevention on all file tools
- Dynamic library sandboxing with panic isolation
