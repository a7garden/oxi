# Oxi Foundation v1 Contract — oxicode Host

> Spec for the `oxicode` side of the Oxi Foundation v1 contract.
> The neutral contract is shared with oxibrain and oxios; this document
> describes what oxicode MUST accept, validate, and reject.

## Scope

oxicode is an **Oxi Foundation host**, not the contract owner. It reads a
versioned filesystem layout under `~/.oxi/foundation/v1/`, maps package-
declared abstract capabilities to its existing workspace/access/tool
policy, and uses oxibrain as its only durable-memory authority.

The contract is the **only** interface across host boundaries. oxicode
does not import from `oxibrain`, `oxios`, or any other host. The shared
contract is enough.

## Layout

```
~/.oxi/foundation/v1/
├── foundation.json          # schema version + host compatibility
├── profiles.json            # non-secret provider/model profiles
├── packages.lock            # immutable resolved package records
└── packages/<sha256>/       # verified immutable package content
```

Overrides:

- `OXI_FOUNDATION_HOME` — when set, replaces `~/.oxi/` as the foundation
  root. oxicode follows it; secrets are never read from this path.
- `OXICODE_HOME` — unchanged. Continues to hold oxicode-owned sessions,
  caches, local overlays, and non-secret state. Must not be confused with
  the foundation root.

Any other path or filename is invalid.

## `foundation.json`

```json
{
  "schema_version": 1,
  "host_compatibility": {
    "oxicode": ">=0.75.0",
    "oxibrain": ">=0.2.0",
    "oxios": ">=0.1.0"
  }
}
```

Validation rules:

- `schema_version` MUST be a positive integer. Any value other than `1`
  is rejected with `Err(FoundationError::UnsupportedSchema)`.
- `host_compatibility.oxicode` MUST be a semver range. oxicode rejects
  the foundation if its `CARGO_PKG_VERSION` does not satisfy the range.
- Unknown host keys are silently ignored.
- Missing `schema_version` is rejected.
- Empty / malformed JSON is rejected with `Err(FoundationError::Parse)`.

## `profiles.json`

```json
{
  "schema_version": 1,
  "profiles": [
    {
      "id": "personal-coding",
      "provider": "anthropic",
      "model": "claude-sonnet",
      "roles": ["coding.primary", "assistant.general"],
      "credential": {
        "service": "dev.oxi.foundation",
        "account": "personal-coding"
      }
    }
  ]
}
```

Validation rules:

- `schema_version` MUST equal `1`.
- `profiles` MUST be a non-empty array. Duplicate profile IDs are
  rejected.
- Each profile's `id` is a non-empty string. Each `provider` and `model`
  is a non-empty string. `roles` is a non-empty array of non-empty
  strings.
- `credential.service` and `credential.account` MUST be non-empty. They
  are locators, not secrets.
- Profiles that contain any of `api_key`, `bearer_token`, `oauth_*`,
  `password`, `secret_value`, `private_key`, etc. are rejected with
  `Err(FoundationError::SecretNotAllowed)`. The validator scrubs known
  shapes but does not rely on a denylist — unknown fields with values
  that look like secret formats are flagged as warnings.
- `provider` MUST be one of the providers registered in
  `oxicode-ai::providers::register_builtins`. Unknown providers fail
  resolution, not parsing.
- `model` is validated against the catalog's known models for the
  provider at first use; the foundation file itself does not need to
  declare it.

## `packages.lock`

```json
{
  "schema_version": 1,
  "packages": [
    {
      "name": "@oxi/code-review",
      "version": "1.4.0",
      "digest": "sha256-9b2c…",
      "source": "foundation",
      "trust": "verified",
      "targets": ["oxicode"],
      "requirements": ["workspace.read", "workspace.patch", "brain.query"]
    }
  ]
}
```

Validation rules:

- `schema_version` MUST equal `1`.
- Each package has a unique `name`. `digest` matches `sha256-<hex>`.
- `targets` MUST include `oxicode`. Any package that lists `oxicode`
  without `oxicode` in its targets is rejected at install.
- `trust` is one of `verified`, `unverified`. Anything else is rejected.
- `requirements` is a list of dotted abstract capability strings. The
  validator accepts the documented set:

  ```
  workspace.read
  workspace.patch
  shell.execute
  browser.navigate
  brain.query
  schedule.manage
  ```

  Unknown requirements are rejected as `Err(FoundationError::UnsupportedRequirement)`.
- The on-disk content at `~/.oxi/foundation/v1/packages/<sha256>/`
  MUST hash to the recorded digest. Mismatches are rejected before
  any resource is loaded.

## Role resolution

The pure decision function `resolve_profile` accepts:

```
resolve_profile(
    explicit_profile: Option<&str>,           // --profile / OXICODE_PROFILE
    explicit_environment_override: Option<&str>, // OXICODE_PROVIDER + OXICODE_MODEL
    requested_role: Option<&str>,             // coding.primary | assistant.general
    foundation_profiles: &[Profile],
    compatibility_import: Option<&CompatibilityImport>,
) -> Result<ResolvedProfile, ResolutionError>
```

Precedence:

1. `explicit_environment_override` — non-persistent automation override.
   Always logs `source = environment`. Provider/model come from
   `OXICODE_PROVIDER` and `OXICODE_MODEL`. No profile is selected.
2. `explicit_profile` — selected profile id. Reads the profile, validates
   the credential locator, and returns the resolved record.
3. Role-compatible foundation profile — when `requested_role` is set,
   oxicode picks the first profile whose `roles` contains the requested
   role. Multiple matches fail with `ResolutionError::AmbiguousRole`.
4. `compatibility_import` (one-time legacy import) — only when enabled
   via `OXICODE_FOUNDATION_MIGRATION=1`. Records a structured migration
   marker after a successful credential export. Has no effect once
   archived.

Absent credentials, an unknown profile, an unknown role, or an
ambiguous role all surface as `ResolutionError` with a typed reason.
oxicode never silently selects a different remote provider.

## Capability mapping

Package requirements are host-local abstract capabilities. oxicode
maps them to its existing policy surface:

| Requirement | oxicode policy |
|---|---|
| `workspace.read` | `AccessGate::allow_workspace_read` + workspace approval |
| `workspace.patch` | `AccessGate::allow_workspace_write` + run approval |
| `shell.execute` | `AccessGate::allow_shell` + `ToolPolicy::bash` |
| `browser.navigate` | `ToolPolicy::web_search` + native-browser feature |
| `brain.query` | Bound typed `BrainClient` already installed at the composition root |
| `schedule.manage` | `CronScheduler` port per active scope |

A package that asks for an unsupported requirement is rejected before
its content is loaded. A verified package is **not** automatically
authorized — every requirement still has to pass oxicode's policy.

## Error states

| Error | When | Surface |
|---|---|---|
| `FoundationError::Parse` | malformed JSON / missing required fields | setup wizard, `/config` |
| `FoundationError::UnsupportedSchema` | `schema_version != 1` | setup wizard |
| `FoundationError::IncompatibleHost` | oxicode version not in `host_compatibility.oxicode` | setup wizard |
| `FoundationError::SecretNotAllowed` | profile contains a secret-shaped field | setup wizard |
| `FoundationError::UnknownProfile` | `--profile` not found | engine startup |
| `FoundationError::UnknownRole` | requested role has no matching profile | engine startup |
| `FoundationError::AmbiguousRole` | multiple profiles match the role | engine startup |
| `FoundationError::UnsupportedRequirement` | package requires unknown capability | package install |
| `FoundationError::DigestMismatch` | on-disk content hash differs | package install |
| `FoundationError::KeychainUnavailable` | OS keychain denied access | engine startup |
| `FoundationError::KeychainLocked` | user cancelled keychain prompt | engine startup |
| `FoundationError::KeychainNotFound` | credential locator has no entry | engine startup |
| `FoundationError::BrainUnavailable` | daemon socket missing / unreachable | engine startup (degraded) |

All errors log `provider`, `profile`, `role`, `source_class` (one of
`environment`, `keychain`, `unavailable`, `migrated`). They never log
the underlying credential value, account name, or bearer token.

## Cross-host fixture location

The shared neutral fixture set lives at:

```
tests/fixtures/oxi-foundation/v1/
├── profiles/
│   ├── valid_personal_coding.json
│   ├── unknown_schema.json
│   ├── duplicate_profile_id.json
│   ├── malformed_credential_locator.json
│   └── role_ambiguous.json
├── packages/
│   ├── valid_lock.json
│   ├── bad_digest.json
│   ├── missing_target.json
│   └── denied_requirement.json
└── foundation.json
```

oxicode's CI consumes these fixtures via the path-resolved
`crate::foundation::fixtures::load` helper. oxibrain and oxios read
the same byte-identical files until a shared crate is justified by
duplicated nontrivial parsing.

## Compatibility matrix

| Host | Owns | Hosts |
|---|---|---|
| **oxibrain** | durable memory, retrieval, projection, consolidation | memory endpoints |
| **oxicode** | code execution, workspace policy, tool invocation, package compilation | `~/.oxicode/` |
| **oxios** | orchestration, experience, persona composition | oxicode SDK embedding |

oxios embeds `oxicode-sdk` directly. oxios MUST NOT spawn the oxicode
CLI as a child process for normal operation; when oxicode is needed
as a CLI, oxios provides its own binary to the user.

## Cross-references

- The canonical oxibrain integration document lives at
  `oxibrain/doc/RFC-047-brain-as-durable-memory.md` (not vendored here).
- The Oxios RFC for multi-host orchestration lives at
  `oxios/docs/rfcs/oxi-foundation.md` (not vendored here).

oxicode does not depend on either repository. The cross-references
exist for human readers, not for cargo.
