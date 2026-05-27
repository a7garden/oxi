#!/usr/bin/env python3
"""Extract model data from model_db.rs into models.json for the generator.

Parses each ModelEntry's `api` field to determine per-provider API type.
If a provider has models with different APIs, uses per-model api overrides.

Usage: python3 scripts/extract-models.py [path/to/model_db.rs]
Output: scripts/models.json
"""

import re
import json
import sys
from collections import Counter

API_MAP = {
    "Api::OpenAiCompletions": "openai-completions",
    "Api::OpenAiResponses": "openai-responses",
    "Api::AnthropicMessages": "anthropic-messages",
    "Api::GoogleGenerativeAi": "google-generative-ai",
    "Api::GoogleVertex": "google-vertex",
    "Api::MistralConversations": "mistral-conversations",
    "Api::AzureOpenAiResponses": "azure-openai-responses",
    "Api::BedrockConverseStream": "bedrock-converse-stream",
}

MODALITY_MAP = {
    "InputModality::Text": "text",
    "InputModality::Image": "image",
}


def parse_model_db(path):
    with open(path) as f:
        content = f.read()

    # Extract ALL_PROVIDER_MODELS entries to get provider→array_name mapping
    all_section = content[content.find("static ALL_PROVIDER_MODELS"):]
    all_section = all_section[:all_section.find("];")]
    provider_order = re.findall(r'"([^"]+)",\s+(\w+_MODELS)', all_section)

    providers = []
    for provider_name, array_name in provider_order:
        # Find the static array — match from declaration to "];"
        pattern = rf'static {array_name}.*?=\s*&\[(.*?)\];'
        m = re.search(pattern, content, re.DOTALL)
        if not m:
            print(f"WARNING: Could not find {array_name}", file=sys.stderr)
            continue

        array_content = m.group(1)

        # Split into individual ModelEntry blocks
        # Each starts with "ModelEntry {" and ends with "},"
        entries = re.split(r'ModelEntry\s*\{', array_content)
        models = []

        for entry in entries:
            if not entry.strip():
                continue

            def get_str(name):
                """Extract a string field value."""
                pat = rf'{name}:\s*"([^"]*)"'
                m = re.search(pat, entry)
                return m.group(1) if m else ""

            def get_val(name, default=""):
                """Extract a non-string field value."""
                pat = rf'{name}:\s*([^,\n]+)'
                m = re.search(pat, entry)
                if m:
                    return m.group(1).strip().rstrip(',')
                return default

            model_id = get_str("id")
            if not model_id:
                continue

            model_name = get_str("name")
            api_raw = get_val("api", "Api::OpenAiCompletions")
            provider_field = get_str("provider")
            reasoning = get_val("reasoning", "false") == "true"

            # Parse input modalities
            input_match = re.search(r'input:\s*(&\[.*?\])', entry, re.DOTALL)
            input_modalities = []
            if input_match:
                input_str = input_match.group(1)
                for mod_name, mod_val in MODALITY_MAP.items():
                    if mod_name in input_str:
                        input_modalities.append(mod_val)
            if not input_modalities:
                input_modalities = ["text"]

            cost_input = float(get_val("cost_input", "0.0"))
            cost_output = float(get_val("cost_output", "0.0"))
            cost_cache_read = float(get_val("cost_cache_read", "0.0"))
            cost_cache_write = float(get_val("cost_cache_write", "0.0"))
            context_window = int(get_val("context_window", "0"))
            max_tokens = int(get_val("max_tokens", "0"))

            api_json = API_MAP.get(api_raw, "openai-completions")

            model = {
                "id": model_id,
                "name": model_name,
                "reasoning": reasoning,
                "input": input_modalities,
                "cost_input": cost_input,
                "cost_output": cost_output,
                "cost_cache_read": cost_cache_read,
                "cost_cache_write": cost_cache_write,
                "context_window": context_window,
                "max_tokens": max_tokens,
            }

            # If model's API differs from provider default, add per-model api
            models.append((api_json, model))

        # Determine provider-level API (most common among its models)
        if models:
            api_counter = Counter(api for api, _ in models)
            provider_api = api_counter.most_common(1)[0][0]
        else:
            provider_api = "openai-completions"

        clean_models = []
        for api_json, model in models:
            # Only add per-model api if it differs from provider api
            if api_json != provider_api:
                model["api"] = api_json
            clean_models.append(model)

        providers.append({
            "name": provider_name,
            "api": provider_api,
            "models": clean_models,
        })

    return {"providers": providers}


if __name__ == "__main__":
    path = sys.argv[1] if len(sys.argv) > 1 else "oxi-ai/src/model_db.rs"
    db = parse_model_db(path)

    total = sum(len(p["models"]) for p in db["providers"])
    print(f"Parsed {total} models across {len(db['providers'])} providers", file=sys.stderr)

    with open("scripts/models.json", "w") as f:
        json.dump(db, f, indent=2, ensure_ascii=False)
    print(f"Written to scripts/models.json", file=sys.stderr)
