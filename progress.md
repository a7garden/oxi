# Progress

## Status
In Progress

## Tasks
- [x] Fix 13: Google API key security fix (query param → header)
- [x] Fix 12: Consolidate duplicate AgentConfig structs

## Files Changed
- oxi-ai/src/providers/google.rs
- oxi-agent/src/types.rs

## Notes
- Moved API key from URL query parameter to x-goog-api-key header
- Google Generative AI API uses x-goog-api-key header (different from Vertex AI which uses Authorization: Bearer)
- Removed duplicate AgentConfig from types.rs; kept the fuller version in config.rs (has compaction support, builder methods). types::AgentConfig had zero external usages — all code already used config::AgentConfig.
