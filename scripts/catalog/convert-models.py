#!/usr/bin/env python3
"""Convert Rust model_db.rs static arrays into per-provider TOML files."""
from __future__ import annotations
import os, re, sys, tomllib
from pathlib import Path

SRC = Path("/Volumes/MERCURY/PROJECTS/oxi/oxi-ai/src/model_db.rs")
DST_DIR = Path("/Volumes/MERCURY/PROJECTS/oxi/oxi-ai/data/catalog/models")

API_MAP = {
    "AnthropicMessages": "anthropic-messages",
    "OpenAiCompletions": "openai-completions",
    "OpenAiResponses": "openai-responses",
    "GoogleGenerativeAi": "google-generative-ai",
    "GoogleVertex": "google-vertex",
    "MistralConversations": "mistral-conversations",
    "AzureOpenAiResponses": "azure-openai-responses",
    "BedrockConverseStream": "bedrock-converse-stream",
}
INPUT_MAP = {"Text": "text", "Image": "image"}
DISPLAY_NAMES = {
    "amazon-bedrock": "Amazon Bedrock",
    "anthropic": "Anthropic",
    "azure-openai-responses": "Azure OpenAI Responses",
    "cerebras": "Cerebras",
    "cloudflare-ai-gateway": "Cloudflare AI Gateway",
    "cloudflare-workers-ai": "Cloudflare Workers AI",
    "deepseek": "DeepSeek",
    "fireworks": "Fireworks",
    "github-copilot": "GitHub Copilot",
    "google": "Google",
    "google-vertex": "Google Vertex",
    "groq": "Groq",
    "huggingface": "Hugging Face",
    "kimi-coding": "Kimi Coding",
    "minimax": "MiniMax",
    "minimax-cn": "MiniMax (China)",
    "mistral": "Mistral",
    "moonshotai": "Moonshot AI",
    "moonshotai-cn": "Moonshot AI (China)",
    "openai": "OpenAI",
    "openai-codex": "OpenAI Codex",
    "opencode": "OpenCode",
    "opencode-go": "OpenCode Go",
    "openrouter": "OpenRouter",
    "vercel-ai-gateway": "Vercel AI Gateway",
    "xai": "xAI",
    "xiaomi": "Xiaomi",
    "zai": "ZAI",
    "together": "Together",
}

STATIC_RE = re.compile(
    r"static\s+([A-Z][A-Z0-9_]*_MODELS)\s*:\s*&\[ModelEntry\]\s*=\s*&\[\s*(.*?)\s*\]\s*;",
    re.DOTALL,
)
ENTRY_RE = re.compile(r"ModelEntry\s*\{(.*?)\}", re.DOTALL)
# Match either a numeric literal or an identifier-with-colons.
NUM_OR_IDENT = re.compile(r"(?:-?\d[\d_]*(?:\.\d[\d_]*)?(?:[eE][+-]?\d+)?|[A-Za-z_][A-Za-z0-9_:]*)")


def array_name_to_provider(name: str) -> str:
    assert name.endswith("_MODELS"), name
    return name[: -len("_MODELS")].lower().replace("_", "-")


def strip_comments_and_whitespace(src: str) -> str:
    src = re.sub(r"//[^\n]*", "", src)
    src = re.sub(r"\s+", " ", src)
    return src.strip()


def split_fields(body: str) -> list[tuple[str, str]]:
    body = body.strip().rstrip(",")
    fields = []
    i, n = 0, len(body)
    while i < n:
        while i < n and body[i] in " ,":
            i += 1
        if i >= n:
            break
        m = re.match(r"([A-Za-z_][A-Za-z0-9_]*)\s*:\s*", body[i:])
        if not m:
            raise ValueError(f"Expected field name at pos {i}: ...{body[i:i+40]!r}...")
        name, i = m.group(1), i + m.end()
        if body[i] == '"':
            j = i + 1
            buf = ['"']
            while j < n:
                c = body[j]
                if c == "\\" and j + 1 < n:
                    buf.append(c)
                    buf.append(body[j + 1])
                    j += 2
                    continue
                buf.append(c)
                if c == '"':
                    j += 1
                    break
                j += 1
            value = "".join(buf)
            i = j
        elif body[i] == "&" and i + 1 < n and body[i + 1] == "[":
            depth = 0
            j = i
            while j < n:
                c = body[j]
                if c == "[":
                    depth += 1
                elif c == "]":
                    depth -= 1
                    if depth == 0:
                        j += 1
                        break
                j += 1
            value = body[i:j]
            i = j
        else:
            m2 = NUM_OR_IDENT.match(body, i)
            if not m2:
                raise ValueError(f"Expected value at pos {i}: ...{body[i:i+40]!r}...")
            value, i = m2.group(0), m2.end()
        fields.append((name, value))
    return fields


def parse_entry(body: str) -> dict:
    body = strip_comments_and_whitespace(body)
    fields = split_fields(body)
    out: dict = {}
    for name, raw in fields:
        if name == "id":
            out["id"] = _unquote(raw)
        elif name == "name":
            out["name"] = _unquote(raw)
        elif name == "api":
            assert raw.startswith("Api::"), f"bad api: {raw}"
            out["api"] = API_MAP.get(raw[len("Api::"):], raw)
        elif name == "provider":
            out["provider"] = _unquote(raw)
        elif name == "reasoning":
            out["reasoning"] = (raw == "true")
        elif name == "input":
            assert raw.startswith("&[") and raw.endswith("]"), f"bad input: {raw}"
            inner = raw[2:-1]
            mods = []
            for piece in inner.split(","):
                piece = piece.strip()
                if not piece:
                    continue
                assert piece.startswith("InputModality::"), f"bad input piece: {piece}"
                mods.append(INPUT_MAP[piece[len("InputModality::"):]])
            out["input"] = mods
        elif name == "cost_input":
            out["cost_input"] = _parse_float(raw)
        elif name == "cost_output":
            out["cost_output"] = _parse_float(raw)
        elif name == "cost_cache_read":
            out["cost_cache_read"] = _parse_float(raw)
        elif name == "cost_cache_write":
            out["cost_cache_write"] = _parse_float(raw)
        elif name == "context_window":
            out["context_window"] = int(raw)
        elif name == "max_tokens":
            out["max_tokens"] = int(raw)
        else:
            raise ValueError(f"Unknown field: {name}")
    return out


def _unquote(s: str) -> str:
    assert s.startswith('"') and s.endswith('"'), f"not a string: {s!r}"
    body, out, i = s[1:-1], [], 0
    while i < len(body):
        c = body[i]
        if c == "\\" and i + 1 < len(body):
            nxt = body[i + 1]
            if nxt == "n": out.append("\n")
            elif nxt == "t": out.append("\t")
            elif nxt == "r": out.append("\r")
            elif nxt == '"': out.append('"')
            elif nxt == "\\": out.append("\\")
            elif nxt == "0": out.append("\0")
            else:
                out.append(c)
                out.append(nxt)
            i += 2
        else:
            out.append(c)
            i += 1
    return "".join(out)


def _parse_float(s: str) -> float:
    return float(s.replace("_", ""))


def toml_escape(s: str) -> str:
    out = []
    for c in s:
        if c == "\\": out.append("\\\\")
        elif c == '"': out.append('\\"')
        elif c == "\n": out.append("\\n")
        elif c == "\r": out.append("\\r")
        elif c == "\t": out.append("\\t")
        elif c == "\b": out.append("\\b")
        elif c == "\f": out.append("\\f")
        elif ord(c) < 0x20: out.append(f"\\u{ord(c):04X}")
        else: out.append(c)
    return '"' + "".join(out) + '"'


def toml_float(x: float) -> str:
    if x != x: return "nan"
    if x == float("inf"): return "inf"
    if x == float("-inf"): return "-inf"
    s = repr(x)
    if "e" not in s and "E" not in s and "." not in s:
        s += ".0"
    return s


def render_toml(provider_id: str, models: list[dict]) -> str:
    display = DISPLAY_NAMES.get(provider_id, provider_id)
    lines = [
        f"# {display} models ({len(models)} entries)",
        f'provider = "{provider_id}"',
        "",
    ]
    for m in models:
        lines.append("[[model]]")
        lines.append(f"id = {toml_escape(m['id'])}")
        lines.append(f"name = {toml_escape(m['name'])}")
        lines.append(f"api = {toml_escape(m['api'])}")
        lines.append(f"provider = {toml_escape(m['provider'])}")
        lines.append(f"reasoning = {'true' if m['reasoning'] else 'false'}")
        if m.get("input"):
            inner = ", ".join(toml_escape(x) for x in m["input"])
            lines.append(f"input = [{inner}]")
        else:
            lines.append('input = []')
        lines.append(f"cost_input = {toml_float(m['cost_input'])}")
        lines.append(f"cost_output = {toml_float(m['cost_output'])}")
        lines.append(f"cost_cache_read = {toml_float(m['cost_cache_read'])}")
        lines.append(f"cost_cache_write = {toml_float(m['cost_cache_write'])}")
        lines.append(f"context_window = {int(m['context_window'])}")
        lines.append(f"max_tokens = {int(m['max_tokens'])}")
        lines.append("")
    while lines and lines[-1] == "":
        lines.pop()
    lines.append("")
    return "\n".join(lines)


def main() -> int:
    if not SRC.exists():
        print(f"FATAL: source not found: {SRC}", file=sys.stderr)
        return 1
    DST_DIR.mkdir(parents=True, exist_ok=True)

    src_text = SRC.read_text(encoding="utf-8")
    blocks = STATIC_RE.findall(src_text)
    print(f"Found {len(blocks)} static arrays in {SRC.name}")

    total = 0
    errors: list[str] = []
    file_count = 0
    summary: list[tuple[str, int, str | None]] = []
    for arr_name, body in blocks:
        provider_id = array_name_to_provider(arr_name)
        try:
            entry_bodies = ENTRY_RE.findall(body)
            if not entry_bodies:
                summary.append((provider_id, 0, "no entries parsed"))
                continue
            models: list[dict] = []
            for entry_body in entry_bodies:
                eb = entry_body.strip()
                if eb.startswith("{"): eb = eb[1:]
                if eb.endswith("}"): eb = eb[:-1]
                models.append(parse_entry(eb))
            out_path = DST_DIR / f"{provider_id}.toml"
            out_text = render_toml(provider_id, models)
            out_path.write_text(out_text, encoding="utf-8")
            total += len(models)
            file_count += 1
            summary.append((provider_id, len(models), None))
        except Exception as e:
            msg = f"{provider_id}: {e}"
            errors.append(msg)
            summary.append((provider_id, 0, str(e)))

    print(f"\n=== Per-provider counts ===")
    for pid, count, err in summary:
        if err:
            print(f"  {pid:30s}  ERROR: {err}")
        else:
            print(f"  {pid:30s}  {count:>4d} models")
    print(f"\nTotal TOML files: {file_count}")
    print(f"Total models:      {total}")
    if errors:
        print(f"\nErrors: {len(errors)}")
        for e in errors:
            print(f"  - {e}")
        return 1

    print(f"\n=== Round-trip validation ===")
    bad: list[str] = []
    for toml_path in sorted(DST_DIR.glob("*.toml")):
        try:
            with open(toml_path, "rb") as f:
                data = tomllib.load(f)
        except Exception as e:
            bad.append(f"{toml_path.name}: {e}")
            continue
        if "provider" not in data:
            bad.append(f"{toml_path.name}: missing top-level 'provider'")
        if "model" not in data or not isinstance(data["model"], list):
            bad.append(f"{toml_path.name}: missing or wrong-typed 'model' list")
    if bad:
        print("Validation errors:")
        for b in bad:
            print(f"  - {b}")
        return 2
    print(f"  All {file_count} files parse cleanly with tomllib OK")

    sample = DST_DIR / "anthropic.toml"
    if sample.exists():
        print(f"\n=== First 30 lines of {sample.name} ===")
        for i, line in enumerate(sample.read_text().splitlines()[:30], 1):
            print(f"  {i:3d}  {line}")

    return 0


if __name__ == "__main__":
    sys.exit(main())
