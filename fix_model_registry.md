# Fix ModelRegistry Naming Confusion

## Problem
Two structs named `ModelRegistry` existed in the codebase:
- `oxi_ai::ModelRegistry` — lightweight, static model DB + dynamic registration (SDK use)
- `oxi_store::ModelRegistry` — CLI-specific: models.json parsing, AuthStorage integration, API key resolution

SDK users needing API key resolution couldn't use `oxi-store`'s version without pulling in CLI-specific dependencies.

## Solution

### 1. `oxi-store/src/model_registry.rs`
- Renamed `pub struct ModelRegistry` to `pub struct CliModelRegistry`
- Added `pub type ModelRegistry = CliModelRegistry;` for backward compatibility within oxi-store
- Added comprehensive doc comments explaining the CLI-specific role
- Updated `impl ModelRegistry` to `impl CliModelRegistry`

### 2. `oxi-store/src/lib.rs`
- Exports both names:
  - `pub use model_registry::CliModelRegistry;` (primary name)
  - `pub use model_registry::ModelRegistry;` (backward-compatible alias)
- Added doc comments distinguishing each export

### 3. `oxi-ai/src/model_registry.rs`
- Added doc comment clarifying its role as a lightweight SDK/engine registry
- References `oxi_store::CliModelRegistry` for CLI users needing auth integration

### 4. `oxi-cli` (no changes needed)
- `use oxi_store::ModelRegistry` continues to work via the type alias

## Verification Results

### Build Check
```
cargo check --workspace --lib 2>&1 | grep '^error' | wc -l
```
**Result: 0 errors** ✓

### CLI Check
```
cargo check -p oxi-cli 2>&1 | grep '^error' | wc -l
```
**Result: 0 errors** ✓

### Test Suite
```
cargo test --workspace --lib 2>&1 | grep 'test result:'
```
**Results:**
- oxi-ai: ok. 232 passed; 0 failed
- oxi-agent: ok. 350 passed; 0 failed
- oxi-store: ok. 307 passed; 0 failed
- oxi-cli: ok. 0 passed (integration tests)
- oxi-sdk: ok. 241 passed; 0 failed
- Other: ok. 79 passed; 0 failed

**Total: 1209 tests passed, 0 failed** ✓

## Summary
The rename successfully disambiguates the two registries while maintaining full backward compatibility. SDK users can now clearly identify `oxi_ai::ModelRegistry` for metadata-only usage, while CLI users can use `oxi_store::CliModelRegistry` for full auth integration.
