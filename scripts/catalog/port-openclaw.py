#!/usr/bin/env python3
"""Port openclaw static model catalog to oxi TOML catalog."""
import json, os, re, sys, tomllib
from pathlib import Path

OPENCLAW_EXT = Path("/tmp/openclaw-upstream/extensions")
OXI_MODELS_DIR = Path("/Volumes/MERCURY/PROJECTS/oxi/oxi-ai/data/catalog/models")
OXI_OPENCLAW_DIR = Path("/Volumes/MERCURY/PROJECTS/oxi/oxi-ai/data/catalog/openclaw")
OXI_MODEL_RS = Path("/Volumes/MERCURY/PROJECTS/oxi/oxi-ai/src/catalog/model.rs")

OXI_EXISTING_FILE = {
    "anthropic", "azure-openai-responses", "cerebras", "cloudflare-ai-gateway",
    "cloudflare-workers-ai", "deepseek", "fireworks", "github-copilot",
    "google", "google-vertex", "groq", "huggingface", "kimi-coding",
    "minimax", "minimax-cn", "mistral", "moonshotai", "moonshotai-cn",
    "openai", "openai-codex", "opencode", "opencode-go", "openrouter",
    "together", "vercel-ai-gateway", "xai", "xiaomi", "zai", "amazon-bedrock",
}

DIR_TO_FILE = {
    "alibaba": "alibaba", "anthropic": "anthropic", "anthropic-vertex": "anthropic-vertex",
    "arcee": "arcee", "byteplus": "byteplus", "cerebras": "cerebras",
    "chutes": "chutes", "cloudflare-ai-gateway": "cloudflare-ai-gateway",
    "codex": "codex", "copilot": "copilot", "copilot-proxy": "copilot-proxy",
    "deepinfra": "deepinfra", "deepseek": "deepseek", "fireworks": "fireworks",
    "github-copilot": "github-copilot", "gmi": "gmi", "google": "google",
    "groq": "groq", "huggingface": "huggingface", "kilocode": "kilocode",
    "kimi-coding": "kimi-coding", "litellm": "litellm", "lmstudio": "lmstudio",
    "lobster": "lobster", "microsoft": "microsoft", "microsoft-foundry": "microsoft-foundry",
    "minimax": "minimax", "mistral": "mistral", "moonshot": "moonshot",
    "novita": "novita", "nvidia": "nvidia", "ollama": "ollama", "openai": "openai",
    "opencode": "opencode", "opencode-go": "opencode-go", "openrouter": "openrouter",
    "openshell": "openshell", "perplexity": "perplexity", "qianfan": "qianfan",
    "qwen": "qwen", "sglang": "sglang", "stepfun": "stepfun",
    "synthetic": "synthetic", "together": "together", "tokenjuice": "tokenjuice",
    "venice": "venice", "vercel-ai-gateway": "vercel-ai-gateway", "vllm": "vllm",
    "xai": "xai", "xiaomi": "xiaomi", "zai": "zai",
}

PROVIDER_API = {
    "claude-cli": "anthropic-messages", "anthropic": "anthropic-messages",
    "anthropic-vertex": "anthropic-messages", "openai": "openai-responses",
    "openai-codex": "openai-responses", "openai-completions": "openai-completions",
    "openai-chatgpt-responses": "openai-responses",
    "github-copilot": "openai-completions",
    "mistral": "mistral-conversations", "azure-openai-responses": "azure-openai-responses",
    "google": "google-generative-ai", "google-vertex": "google-vertex",
    "google-gemini-cli": "google-generative-ai", "xai": "openai-completions",
    "groq": "openai-completions", "cerebras": "openai-completions",
    "deepseek": "openai-completions", "fireworks": "openai-completions",
    "together": "openai-completions", "deepinfra": "openai-completions",
    "openrouter": "openai-completions", "vercel-ai-gateway": "openai-completions",
    "chutes": "openai-completions", "novita": "openai-completions",
    "nvidia": "openai-completions", "arcee": "openai-completions",
    "ollama": "openai-completions", "ollama-cloud": "openai-completions",
    "lmstudio": "openai-completions", "vllm": "openai-completions",
    "sglang": "openai-completions", "opencode": "openai-completions",
    "opencode-go": "openai-completions", "moonshot": "openai-completions",
    "moonshotai": "openai-completions", "moonshotai-cn": "openai-completions",
    "qwen": "openai-completions", "qwen-oauth": "openai-completions",
    "qwen-portal": "openai-completions", "qwencloud": "openai-completions",
    "dashscope": "openai-completions", "modelstudio": "openai-completions",
    "alibaba": "openai-completions", "byteplus": "openai-completions",
    "byteplus-plan": "openai-completions", "gmi": "openai-completions",
    "gmi-cloud": "openai-completions", "gmicloud": "openai-completions",
    "stepfun": "openai-completions", "stepfun-plan": "openai-completions",
    "qianfan": "openai-completions", "kilocode": "openai-completions",
    "kimi": "openai-completions", "kimi-coding": "openai-completions",
    "venice": "openai-completions", "zai": "openai-completions",
    "minimax": "anthropic-messages", "minimax-cn": "anthropic-messages",
    "minimax-portal": "anthropic-messages", "synthetic": "anthropic-messages",
    "codex": "openai-responses", "copilot": "openai-completions",
    "copilot-proxy": "openai-completions", "perplexity": "openai-completions",
    "litellm": "openai-completions", "tokenjuice": "openai-completions",
    "microsoft": "azure-openai-responses", "microsoft-foundry": "azure-openai-responses",
    "lobster": "openai-completions", "openshell": "openai-completions",
    "huggingface": "openai-completions", "cloudflare-ai-gateway": "openai-completions",
    "cloudflare-workers-ai": "openai-completions",
    "amazon-bedrock": "bedrock-converse-stream",
    "amazon-bedrock-mantle": "openai-completions",
    "vertex": "google-vertex", "azure": "azure-openai-responses",
    "bedrock": "bedrock-converse-stream", "baseten": "openai-completions",
    "kilo": "openai-completions", "zenmux": "openai-completions",
    "llmgateway": "openai-completions", "nvidia-nim": "openai-completions",
    "sap-ai-core": "openai-completions", "gitlab": "openai-completions",
    "zai-cn": "openai-completions", "zai-global": "openai-completions",
    "zai-coding-cn": "openai-completions", "zai-coding-global": "openai-completions",
}

def tesc(s):
    if s is None: return '""'
    if re.match(r'^[A-Za-z0-9_\-\.\+\/\:]+$', s):
        return f'"{s}"'
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n").replace("\t", "\\t").replace("\r", "\\r") + '"'

def render_model(m):
    lines = ["[[model]]"]
    lines.append(f"id = {tesc(m['id'])}")
    lines.append(f"name = {tesc(m['name'])}")
    lines.append(f"api = {tesc(m['api'])}")
    lines.append(f"provider = {tesc(m['provider'])}")
    lines.append(f"reasoning = {'true' if m['reasoning'] else 'false'}")
    inp = m["input"]
    if isinstance(inp, list) and inp:
        lines.append("input = [" + ", ".join(tesc(x) for x in inp) + "]")
    else:
        lines.append("input = [\"text\"]")
    lines.append(f"cost_input = {m['cost_input']}")
    lines.append(f"cost_output = {m['cost_output']}")
    lines.append(f"cost_cache_read = {m['cost_cache_read']}")
    lines.append(f"cost_cache_write = {m['cost_cache_write']}")
    lines.append(f"context_window = {m['context_window']}")
    lines.append(f"max_tokens = {m['max_tokens']}")
    return "\n".join(lines) + "\n"

def parse_model(m, provider_id):
    if "id" not in m: return None
    cost = m.get("cost") or {}
    api = PROVIDER_API.get(provider_id, "openai-completions")
    inp_raw = m.get("input", [])
    inp = []
    for x in inp_raw:
        if x == "text": inp.append("text")
        elif x == "image": inp.append("image")
    if not inp: inp = ["text"]
    return {
        "id": m["id"], "name": m.get("name", m["id"]), "api": api,
        "provider": provider_id, "reasoning": bool(m.get("reasoning", False)),
        "input": inp, "cost_input": float(cost.get("input", 0.0)),
        "cost_output": float(cost.get("output", 0.0)),
        "cost_cache_read": float(cost.get("cacheRead", 0.0)),
        "cost_cache_write": float(cost.get("cacheWrite", 0.0)),
        "context_window": int(m.get("contextWindow", 128000)),
        "max_tokens": int(m.get("maxTokens", 4096)),
    }

def load_existing_in_file(path):
    """Returns dict[provider_id, dict[id, model_dict]]"""
    out = {}
    try:
        with open(path, "rb") as f: data = tomllib.load(f)
    except Exception as e:
        print(f"  WARN: cannot parse {path}: {e}", file=sys.stderr)
        return out
    for m in data.get("model", []):
        pid = m.get("provider", "")
        out.setdefault(pid, {})[m["id"]] = m
    return out

def write_fresh_file(path, primary_pid, models, pids):
    path.parent.mkdir(parents=True, exist_ok=True)
    body = []
    if len(pids) > 1:
        body.append(f"# {', '.join(sorted(pids))} models — ported from openclaw (MIT)")
    else:
        body.append(f"# {next(iter(pids))} models — ported from openclaw (MIT)")
    body.append("# Source: openclaw MIT — github.com/openclaw/openclaw")
    body.append(f'provider = "{primary_pid}"')
    body.append("")
    for m in sorted(models, key=lambda x: (x["provider"], x["id"])):
        body.append(render_model(m))
        body.append("")
    path.write_text("\n".join(body))

def update_models_index_rs(new_files):
    if not new_files: return
    text = OXI_MODEL_RS.read_text()
    start = text.index("pub fn models_index()")
        # find the line with just "    ];"
    i = text.index("&[", start)
    depth = 0
    for j in range(i, len(text)):
        if text[j] == "[": depth += 1
        elif text[j] == "]":
            depth -= 1
            if depth == 0:
                end = j + 1
                break
    body = text[start:end]
    existing = set(re.findall(r'\("([^"]+)",\s*include_str!\("([^"]+)"\)\)', body))
    additions = []
    for provider_id, file_path in new_files:
        rel = os.path.relpath(file_path, OXI_MODEL_RS.parent.parent.parent)
        rel = rel.replace("oxi-ai/", "").replace(os.sep, "/")
        if (provider_id, rel) in existing: continue
        additions.append(f'        ("{provider_id}", include_str!("{rel}")),')
    if not additions: return
    new_body = body[:-2] + "\n" + "\n".join(additions) + "\n    ];"
    text = text[:start] + new_body + text[end:]
    OXI_MODEL_RS.write_text(text)
    print(f"  Updated {OXI_MODEL_RS.name}: added {len(additions)} include_str! entries")

def main():
    dry_run = "--dry-run" in sys.argv
    print("=" * 70)
    print("openclaw -> oxi TOML port")
    print("=" * 70)
    file_buckets = {}
    file_primary = {}
    file_pids = {}
    for directory, file_base in DIR_TO_FILE.items():
        manifest = OPENCLAW_EXT / directory / "openclaw.plugin.json"
        if not manifest.exists(): continue
        try: data = json.loads(manifest.read_text())
        except Exception as e:
            print(f"  WARN: cannot parse {manifest}: {e}", file=sys.stderr); continue
        catalog = data.get("modelCatalog", {}).get("providers", {})
        extracted = []
        for pid, pval in catalog.items():
            if not isinstance(pval, dict): continue
            for m in pval.get("models", []):
                parsed = parse_model(m, pid)
                if parsed: extracted.append((pid, parsed))
        if not extracted: continue
        is_existing = file_base in OXI_EXISTING_FILE
        out_path = OXI_MODELS_DIR / f"{file_base}.toml" if is_existing else OXI_OPENCLAW_DIR / f"{file_base}.toml"
        file_buckets.setdefault(str(out_path), []).extend(extracted)
        file_primary[str(out_path)] = extracted[0][0]
        file_pids.setdefault(str(out_path), set()).update(pid for pid, _ in extracted)

    added_total = 0
    conflicts = []
    new_files = []
    per_file_summary = []
    for out_path_str, items in sorted(file_buckets.items()):
        out_path = Path(out_path_str)
        is_existing = out_path.parent == OXI_MODELS_DIR
        primary_pid = file_primary[out_path_str]
        pids = file_pids[out_path_str]
        if is_existing:
            existing_in_file = load_existing_in_file(out_path)
            to_add = []
            for pid, m in items:
                same_pid_existing = existing_in_file.get(pid, {})
                if m["id"] in same_pid_existing:
                    ex = same_pid_existing[m["id"]]
                    diffs = []
                    for k in ("name", "api", "reasoning", "context_window", "max_tokens",
                              "cost_input", "cost_output", "cost_cache_read", "cost_cache_write", "input"):
                        if ex.get(k) != m.get(k):
                            diffs.append(f"{k}: oxi={ex.get(k)!r} openclaw={m.get(k)!r}")
                    if diffs:
                        conflicts.append(f"  {out_path.name}::{m['id']} (provider={pid}): {', '.join(diffs)}")
                    continue
                # Don't clobber a same-id model belonging to a *different* provider in this file.
                if any(m["id"] in ms for opid, ms in existing_in_file.items() if opid != pid):
                    continue
                to_add.append(m)
            if to_add and not dry_run:
                text = out_path.read_text().rstrip() + "\n"
                for m in to_add:
                    text += "\n" + render_model(m) + "\n"
                out_path.write_text(text)
            added_total += len(to_add)
            per_file_summary.append((out_path.name, len(items), len(to_add), "merged"))
        else:
            seen = set()
            deduped = []
            for pid, m in items:
                key = (m["provider"], m["id"])
                if key in seen: continue
                seen.add(key)
                deduped.append(m)
            if not dry_run:
                write_fresh_file(out_path, primary_pid, deduped, pids)
            added_total += len(deduped)
            per_file_summary.append((out_path.name, len(deduped), len(deduped), "new"))
            new_files.append((primary_pid, str(out_path)))

    if new_files and not dry_run:
        update_models_index_rs(new_files)

    print()
    print("=" * 70)
    print("Summary")
    print("=" * 70)
    print(f"Total new models added: {added_total}")
    print(f"New files created: {len(new_files)}")
    if new_files:
        print(f"  in: {OXI_OPENCLAW_DIR}")
    print()
    print("Per-file breakdown:")
    print(f"  {'file':<40s} {'openclaw':>10s} {'added':>8s}  {'mode'}")
    for name, total, added, mode in per_file_summary:
        print(f"  {name:<40s} {total:>10d} {added:>8d}  {mode}")
    if conflicts:
        print()
        print(f"Conflicts (oxi wins, openclaw values ignored): {len(conflicts)}")
        for c in conflicts: print(c)
    print()
    print("Done." + (" [DRY RUN]" if dry_run else ""))
    if new_files:
        print()
        print("First 30 lines of one new file (chutes.toml if it exists):")
        sample = OXI_OPENCLAW_DIR / "chutes.toml"
        if sample.exists():
            for line in sample.read_text().splitlines()[:30]:
                print(f"  {line}")

if __name__ == "__main__":
    main()
