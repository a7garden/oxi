# Progress

## Status
Completed

## Tasks
- Port pi-mono package-manager.ts to Rust (packages.rs)

## Files Changed
- `oxi-cli/src/packages.rs` — Major enhancement (~1800 lines). Ported all missing features from pi-mono's package-manager.ts
- `oxi-cli/Cargo.toml` — Added `semver` dependency

## What was ported

### 1. Package Sources (ParsedSource enum)
- `npm:<package>[@<version>]` — npm registry packages
- `git://`, `git+ssh://`, `git+https://`, `git@host:path` — git repositories  
- `github:org/repo[@ref]` — GitHub shorthand
- URL archives (`.tar.gz`, `.tgz`, `.zip`, `.tar.bz2`)
- Local path (fallback)

### 2. Package Installation
- `install_from_source(source, scope)` — universal install from any source type
- `install_npm_pack(spec, scope)` — npm pack + tar extraction approach
- `install_git_sync(source, repo, ref, scope)` — git clone with optional ref checkout + npm dep install
- `install_url(url, scope)` — async HTTP download + archive extraction
- `install_local(path)` — local directory copy (existing, enhanced)

### 3. Package Update
- `update(name)` — smart update based on lockfile source type
- `update_all()` — batch update all installed packages
- `check_for_updates()` — async check for available npm/git updates
- Git update: fetch → reset --hard → clean -fdx

### 4. Package Uninstall
- `uninstall(name)` — by package name (existing)
- `uninstall_from_source(source, scope)` — by source specifier, handles git/npm cleanup
- Prune empty parent directories after git removal

### 5. Package Listing
- `list()` — list all installed manifests
- `list_configured()` — list with ConfiguredPackage metadata (source, scope, installed_path)
- `get_installed_path_for_source(source, scope)` — resolve source → installed path

### 6. Lockfile (oxi-lock.json)
- `Lockfile` struct with version, packages map
- `LockEntry` with source, name, version, integrity hash, scope, source_type, dependencies
- SHA-256 integrity hash for installed directories
- Read/write JSON lockfile to packages dir

### 7. Dependency Resolution
- `resolve_dependencies()` — detect missing dependencies across installed packages
- PackageManifest now has `dependencies: BTreeMap<String, String>`

### 8. Package Types
- ResourceKind: Extension, Skill, Prompt, Theme
- SourceScope: User, Project  
- ResourceOrigin: Package, TopLevel
- ProgressEvent/ProgressAction for install/remove/update/clone/pull

### 9. Version Constraints (semver)
- `NpmPackageInfo::resolve_version(constraint)` — semver range matching
- `version_satisfies(name, requirement)` — check installed version against constraint
- Uses `semver` crate for version parsing and requirement matching

### 10. Git Operations
- `git_clone(repo, target, ref)` — clone with optional checkout
- `git_update(repo_dir, ref)` — fetch + reset + clean
- `git_has_update(repo_dir)` — check remote for new commits
- `git_command()` / `git_command_silent()` — low-level git helpers

### 11. NPM Registry
- `NpmPackageInfo::fetch(name)` — async registry API lookup
- `get_latest_npm_version(name)` — convenience function
- Version resolution with semver ranges

### 12. Validation
- `validate_package(dir)` — validate manifest, check paths, semver, resources

## Notes
- Pre-existing compilation errors exist in other oxi-cli files (session.rs, agent_session.rs, lib.rs, etc.) — NOT caused by this change
- `cargo check -p oxi-cli` shows 0 errors/warnings in packages.rs
- All existing tests preserved; new tests added for source parsing, lockfile, validation, dependencies, version matching, resolve, and progress callbacks
- Uses shell-out to `git` for git operations (no git2 crate dependency needed)
- Uses `reqwest` for HTTP, `serde_json` for manifests/lockfile, `toml` for package manifests
- Progress callback system matches the TypeScript ProgressCallback pattern
