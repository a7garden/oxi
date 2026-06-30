//! LLM fact-extraction wrapper around any [`oxi_agent::tools::MemoryBackend`].
//!
//! When `settings.memory_llm_extract = true`, every `put(content, …)`
//! is sent through a structured extraction prompt and the model's
//! JSON response is fanned out into one `inner.put(fact_text, …)`
//! per atomic fact.
//!
//! Gap-2 design: the LLM call must run inside the async runtime.
//! The LLM extractor is implemented directly as an async function
//! that awaits the provider stream. The sync [`FactExtractor`]
//! trait is retained only for the **heuristic** path (pure CPU, no
//! I/O).
//!
//! Failure policy: when the extractor errors, returns no facts,
//! or returns malformed JSON, the wrapper falls back to storing
//! `content` verbatim so `memory_retain` never silently drops user
//! input.
#![allow(clippy::manual_pattern_char_comparison)]
#![allow(missing_docs)]
#![allow(dead_code)]

use std::pin::Pin;
use std::sync::Arc;

use futures::StreamExt;
use oxi_agent::tools::{MemoryBackend, MemoryItem, ToolError};
use oxi_ai::{Context, ProviderEvent};
use serde::Deserialize;

use super::settings::Settings;

/// One atomic fact returned by the LLM extractor.
#[derive(Debug, Clone, Deserialize)]
pub struct ExtractedFact {
    /// Atomic fact text. Must be non-empty after trim.
    pub content: String,
    /// Free-form importance in `[0, 1]`. `None` → backend default.
    #[serde(default)]
    pub importance: Option<f64>,
    /// Free-form kind (`"fact"` / `"preference"` / `"context"` /
    /// `"summary"`). `None` → kind passed into `put`.
    #[serde(default)]
    pub kind: Option<String>,
}

/// JSON shape returned by the extraction prompt.
#[derive(Debug, Default, Deserialize)]
struct ExtractionPayload {
    #[serde(default)]
    pub facts: Vec<ExtractedFact>,
}

/// Sync, no-IO extractor (heuristic). Safe to call anywhere.
pub trait FactExtractor: Send + Sync {
    /// Return `None` to defer to the inner backend with the raw payload.
    fn extract(&self, content: &str, kind: &str) -> Option<Vec<ExtractedFact>>;
}

/// Default heuristic — splits on sentence boundaries.
pub struct HeuristicFactExtractor;

impl FactExtractor for HeuristicFactExtractor {
    fn extract(&self, content: &str, _kind: &str) -> Option<Vec<ExtractedFact>> {
        let facts: Vec<ExtractedFact> = content
            .split(|c: char| c == '.' || c == '!' || c == '?' || c == '\n')
            .map(str::trim)
            .filter(|s| s.len() >= 10 && !s.ends_with('?'))
            .map(|s| ExtractedFact {
                content: s.to_string(),
                importance: None,
                kind: None,
            })
            .collect();
        if facts.is_empty() { None } else { Some(facts) }
    }
}

/// omp `stage_one_system.md`, truncated to the parts that drive
/// our extraction contract (we don't need `rollout_slug` here — we
/// only store a flat list of facts).
pub const STAGE_ONE_SYSTEM_PROMPT: &str = "\
You extract atomic durable facts from free-form text.\n\
Return STRICT JSON ONLY (no markdown, no commentary).\n\
Output contract: {\"facts\": [{\"content\": \"string\", \"importance\": number, \"kind\": \"string\"}]}.\n\
Rules:\n\
- Each fact is one self-contained claim (a constraint, decision, workflow, or resolved pitfall).\n\
- Skip transient chatter, greetings, and pure questions.\n\
- importance in [0, 1] (1 = critical, never forget).\n\
- kind is one of: \"fact\", \"preference\", \"context\", \"summary\".\n\
- When no durable signal exists, return {\"facts\": []}.";

/// omp `stage_one_input.md` user template (`{{content}}`).
pub const STAGE_ONE_USER_TEMPLATE: &str = "\
Text to extract from:\n\
{{content}}\n\n\
Return JSON now.";

/// Async LLM fact extractor. Owns a `Provider` and a `Model`.
pub struct LlmExtractor {
    provider: Arc<dyn oxi_ai::Provider>,
    model: oxi_ai::Model,
}

impl std::fmt::Debug for LlmExtractor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmExtractor")
            .field("model", &self.model.id)
            .finish_non_exhaustive()
    }
}

impl LlmExtractor {
    /// Build an extractor backed by any `oxi_ai::Provider`.
    pub fn new(provider: Arc<dyn oxi_ai::Provider>, model: oxi_ai::Model) -> Self {
        Self { provider, model }
    }
}

/// Async extraction helper shared by the wrap path.
async fn run_extraction(
    provider: &Arc<dyn oxi_ai::Provider>,
    model: &oxi_ai::Model,
    content: &str,
) -> Option<Vec<ExtractedFact>> {
    let user = STAGE_ONE_USER_TEMPLATE.replace("{{content}}", content);
    let mut ctx = Context::default();
    ctx.set_system_prompt(STAGE_ONE_SYSTEM_PROMPT.to_string());
    ctx.add_message(oxi_ai::Message::user(user));
    let mut stream = provider.stream(model, &ctx, None).await.ok()?;
    let mut buf = String::new();
    while let Some(ev) = stream.next().await {
        match ev {
            ProviderEvent::TextDelta { delta, .. } => buf.push_str(&delta),
            ProviderEvent::Done { .. } => break,
            ProviderEvent::Error { .. } => return None,
            _ => {}
        }
    }
    parse_facts(&buf)
}

/// Parse the strict-JSON `{"facts": […]}` payload that the model
/// returns. Strips ```json fences defensively.
fn parse_facts(buf: &str) -> Option<Vec<ExtractedFact>> {
    let trimmed = buf
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let payload: ExtractionPayload = serde_json::from_str(trimmed).ok()?;
    let facts: Vec<ExtractedFact> = payload
        .facts
        .into_iter()
        .filter(|f| !f.content.trim().is_empty())
        .collect();
    if facts.is_empty() { None } else { Some(facts) }
}

/// Wrapper around any [`MemoryBackend`].
pub struct ExtractingMemoryBackend {
    inner: Arc<dyn MemoryBackend>,
    heuristic: Arc<dyn FactExtractor>,
    llm: Option<LlmExtractor>,
}

impl std::fmt::Debug for ExtractingMemoryBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtractingMemoryBackend")
            .field("inner", &"<dyn MemoryBackend>")
            .field("heuristic", &"<dyn FactExtractor>")
            .field("llm", &self.llm.as_ref().map(|l| l.model.id.clone()))
            .finish()
    }
}

impl ExtractingMemoryBackend {
    /// Build a wrapper that runs extraction before delegating to the
    /// inner backend.
    pub fn new(
        inner: Arc<dyn MemoryBackend>,
        heuristic: Arc<dyn FactExtractor>,
        llm: Option<LlmExtractor>,
    ) -> Self {
        Self {
            inner,
            heuristic,
            llm,
        }
    }
}

impl MemoryBackend for ExtractingMemoryBackend {
    fn put<'a>(
        &'a self,
        content: &'a str,
        kind: &'a str,
        subject: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, ToolError>> + Send + 'a>> {
        let inner = Arc::clone(&self.inner);
        let heuristic = Arc::clone(&self.heuristic);
        let llm = self.llm.as_ref().map(|l| LlmExtractorHandle {
            provider: Arc::clone(&l.provider),
            model: l.model.clone(),
        });

        Box::pin(async move {
            const MAX_FACTS_PER_PUT: usize = 16;

            let facts = if let Some(handle) = llm {
                handle.extract(content).await
            } else {
                heuristic.extract(content, kind)
            };

            match facts {
                Some(fs) if !fs.is_empty() => {
                    let mut last: Result<String, ToolError> = Ok(content.to_string());
                    for fact in fs.into_iter().take(MAX_FACTS_PER_PUT) {
                        let fkind = fact.kind.as_deref().unwrap_or(kind);
                        match inner.put(&fact.content, fkind, subject).await {
                            Ok(id) => last = Ok(id),
                            Err(e) => {
                                last = Err(e);
                                break;
                            }
                        }
                    }
                    last
                }
                _ => inner.put(content, kind, subject).await,
            }
        })
    }

    fn search<'a>(
        &'a self,
        query: &'a str,
        k: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<MemoryItem>, ToolError>> + Send + 'a>> {
        self.inner.search(query, k)
    }

    fn list<'a>(
        &'a self,
        subject: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<MemoryItem>, ToolError>> + Send + 'a>> {
        self.inner.list(subject)
    }

    fn delete<'a>(
        &'a self,
        id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), ToolError>> + Send + 'a>> {
        self.inner.delete(id)
    }

    fn memory_info(&self) -> Option<String> {
        self.inner.memory_info()
    }
}

/// Borrow-friendly clone of an [`LlmExtractor`] so it can move into
/// the `async move { }` block on `put`.
struct LlmExtractorHandle {
    provider: Arc<dyn oxi_ai::Provider>,
    model: oxi_ai::Model,
}

impl LlmExtractorHandle {
    async fn extract(&self, content: &str) -> Option<Vec<ExtractedFact>> {
        run_extraction(&self.provider, &self.model, content).await
    }
}

/// Resolve the LLM extractor from the wired `Oxi` engine. Returns
/// `None` when the user hasn't configured
/// `memory_llm_extract_model` or the pattern doesn't resolve.
pub fn try_make_llm_extractor(oxi: &oxi_sdk::Oxi, settings: &Settings) -> Option<LlmExtractor> {
    let pat = settings.memory_llm_extract_model.trim();
    if pat.is_empty() {
        return None;
    }
    let model = oxi.resolve_model(pat).ok()?;
    let provider = oxi.providers().get(model.provider.as_str())?;
    Some(LlmExtractor::new(provider, model))
}

/// Wire the wrapper on top of an existing memory backend.
pub fn wrap_with_extractor(
    inner: Arc<dyn MemoryBackend>,
    settings: &Settings,
    oxi: Option<&oxi_sdk::Oxi>,
) -> Arc<dyn MemoryBackend> {
    if !settings.memory_llm_extract {
        return inner;
    }
    let heuristic: Arc<dyn FactExtractor> = Arc::new(HeuristicFactExtractor);
    let llm = oxi.and_then(|o| try_make_llm_extractor(o, settings));
    Arc::new(ExtractingMemoryBackend::new(inner, heuristic, llm))
}
