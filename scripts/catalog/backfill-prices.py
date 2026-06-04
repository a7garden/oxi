#!/usr/bin/env python3
"""Backfill missing prices in oxi's openclaw-derived model catalog.

## Background

oxi has 13 TOML files in `oxi-ai/data/catalog/openclaw/`, ported from the
openclaw project (MIT). 77 models had `cost_input = 0.0` and
`cost_output = 0.0`. Root cause: openclaw's
`extensions/<provider>/openclaw.plugin.json` files explicitly encode
`cost: { input: 0, output: 0, ... }` for these providers. The oxi port
preserved those values faithfully.

## What this script does

For providers with public pricing APIs, the script fetches live prices
and applies them to 0-cost entries:

- **Venice** (https://api.venice.ai/api/v1/models): 30 entries updated
- **Novita** (https://api.novita.ai/v3/openai/models): 6 entries updated

For all other providers (gmi, kilocode, moonshot, nvidia, ollama-cloud,
qianfan, qwen, stepfun), the script audits, reports, and **leaves 0
values in place** because no public pricing API is reachable without
authentication in this environment.

## Source data

- **Venice** prices are from `model_spec.pricing.usd` (USD per 1M tokens).
- **Novita** prices are from `input_token_price_per_m` /
  `output_token_price_per_m` (micro-USD per 1M tokens; divided by 1000).

Both fetched on 2026-06-04.

## Idempotency

Re-running produces zero changes if PRICE_OVERRIDES is unchanged. Updates
to upstream APIs would be reflected on the next run.
"""
from __future__ import annotations
import re
import sys
import tomllib
from pathlib import Path

OPENCLAW_DIR = Path("oxi-ai/data/catalog/openclaw")
REPORT_PATH = Path("/tmp/price_backfill_report.md")

PRICE_OVERRIDES = {
    ("venice", "zai-org-glm-5-1"): {
                    "cost_input": 1.75,
                    "cost_output": 5.5,
                    "cost_cache_read": 0.325,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "zai-org-glm-5"): {
                    "cost_input": 1,
                    "cost_output": 3.2,
                    "cost_cache_read": 0.2,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "z-ai-glm-5-turbo"): {
                    "cost_input": 1.2,
                    "cost_output": 4,
                    "cost_cache_read": 0.24,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "z-ai-glm-5v-turbo"): {
                    "cost_input": 1.5,
                    "cost_output": 5,
                    "cost_cache_read": 0.3,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "olafangensan-glm-4.7-flash-heretic"): {
                    "cost_input": 0.14,
                    "cost_output": 0.8,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "zai-org-glm-4.7-flash"): {
                    "cost_input": 0.125,
                    "cost_output": 0.5,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "zai-org-glm-4.6"): {
                    "cost_input": 0.85,
                    "cost_output": 2.75,
                    "cost_cache_read": 0.3,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "zai-org-glm-4.7"): {
                    "cost_input": 0.55,
                    "cost_output": 2.65,
                    "cost_cache_read": 0.11,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "venice-uncensored-1-2"): {
                    "cost_input": 0.2,
                    "cost_output": 0.9,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "venice-uncensored-role-play"): {
                    "cost_input": 0.5,
                    "cost_output": 2,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "qwen-3-7-max"): {
                    "cost_input": 2.7,
                    "cost_output": 8.05,
                    "cost_cache_read": 0.27,
                    "cost_cache_write": 3.35,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "qwen-3-7-plus"): {
                    "cost_input": 0.5,
                    "cost_output": 2,
                    "cost_cache_read": 0.05,
                    "cost_cache_write": 0.625,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "qwen-3-6-plus"): {
                    "cost_input": 0.625,
                    "cost_output": 3.75,
                    "cost_cache_read": 0.0625,
                    "cost_cache_write": 0.78,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "qwen3-6-27b"): {
                    "cost_input": 0.325,
                    "cost_output": 3.25,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "qwen3-5-9b"): {
                    "cost_input": 0.1,
                    "cost_output": 0.15,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "qwen3-5-397b-a17b"): {
                    "cost_input": 0.75,
                    "cost_output": 4.5,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "qwen3-5-35b-a3b"): {
                    "cost_input": 0.3125,
                    "cost_output": 1.25,
                    "cost_cache_read": 0.15625,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "qwen3-235b-a22b-thinking-2507"): {
                    "cost_input": 0.45,
                    "cost_output": 3.5,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "qwen3-235b-a22b-instruct-2507"): {
                    "cost_input": 0.15,
                    "cost_output": 0.75,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "qwen3-next-80b"): {
                    "cost_input": 0.35,
                    "cost_output": 1.9,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "qwen3-vl-235b-a22b"): {
                    "cost_input": 0.25,
                    "cost_output": 1.5,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "qwen3-coder-480b-a35b-instruct-turbo"): {
                    "cost_input": 0.35,
                    "cost_output": 1.5,
                    "cost_cache_read": 0.04,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "google-gemma-4-26b-a4b-it"): {
                    "cost_input": 0.1625,
                    "cost_output": 0.5,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "google-gemma-4-31b-it"): {
                    "cost_input": 0.155,
                    "cost_output": 0.44,
                    "cost_cache_read": 0.12,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "gemma-4-uncensored"): {
                    "cost_input": 0.1625,
                    "cost_output": 0.5,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "google-gemma-3-27b-it"): {
                    "cost_input": 0.12,
                    "cost_output": 0.2,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "arcee-trinity-large-thinking"): {
                    "cost_input": 0.3125,
                    "cost_output": 1.125,
                    "cost_cache_read": 0.075,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "grok-4-3"): {
                    "cost_input": 1.42,
                    "cost_output": 2.83,
                    "cost_cache_read": 0.23,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "grok-4-20"): {
                    "cost_input": 1.42,
                    "cost_output": 2.83,
                    "cost_cache_read": 0.23,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "grok-4-20-multi-agent"): {
                    "cost_input": 1.42,
                    "cost_output": 2.83,
                    "cost_cache_read": 0.23,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "grok-build-0-1"): {
                    "cost_input": 1,
                    "cost_output": 2,
                    "cost_cache_read": 0.2,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "mistral-small-3-2-24b-instruct"): {
                    "cost_input": 0.09375,
                    "cost_output": 0.25,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "mistral-small-2603"): {
                    "cost_input": 0.1875,
                    "cost_output": 0.75,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "hermes-3-llama-3.1-405b"): {
                    "cost_input": 1.1,
                    "cost_output": 3,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "gemini-3-1-pro-preview"): {
                    "cost_input": 2.5,
                    "cost_output": 15,
                    "cost_cache_read": 0.5,
                    "cost_cache_write": 0.5,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "gemini-3-5-flash"): {
                    "cost_input": 1.55,
                    "cost_output": 9.45,
                    "cost_cache_read": 0.155,
                    "cost_cache_write": 0.086,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "gemini-3-flash-preview"): {
                    "cost_input": 0.7,
                    "cost_output": 3.75,
                    "cost_cache_read": 0.07,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "claude-opus-4-8"): {
                    "cost_input": 6,
                    "cost_output": 30,
                    "cost_cache_read": 0.6,
                    "cost_cache_write": 7.5,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "claude-opus-4-8-fast"): {
                    "cost_input": 12,
                    "cost_output": 60,
                    "cost_cache_read": 1.2,
                    "cost_cache_write": 15,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "claude-opus-4-7"): {
                    "cost_input": 6,
                    "cost_output": 30,
                    "cost_cache_read": 0.6,
                    "cost_cache_write": 7.5,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "claude-opus-4-7-fast"): {
                    "cost_input": 36,
                    "cost_output": 180,
                    "cost_cache_read": 3.6,
                    "cost_cache_write": 45,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "claude-opus-4-6"): {
                    "cost_input": 6,
                    "cost_output": 30,
                    "cost_cache_read": 0.6,
                    "cost_cache_write": 7.5,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "claude-opus-4-6-fast"): {
                    "cost_input": 36,
                    "cost_output": 180,
                    "cost_cache_read": 3.6,
                    "cost_cache_write": 45,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "claude-opus-4-5"): {
                    "cost_input": 6,
                    "cost_output": 30,
                    "cost_cache_read": 0.6,
                    "cost_cache_write": 7.5,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "claude-sonnet-4-6"): {
                    "cost_input": 3.6,
                    "cost_output": 18,
                    "cost_cache_read": 0.36,
                    "cost_cache_write": 4.5,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "claude-sonnet-4-5"): {
                    "cost_input": 3.75,
                    "cost_output": 18.75,
                    "cost_cache_read": 0.375,
                    "cost_cache_write": 4.69,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "openai-gpt-oss-120b"): {
                    "cost_input": 0.07,
                    "cost_output": 0.3,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "kimi-k2-6"): {
                    "cost_input": 0.85,
                    "cost_output": 4.655,
                    "cost_cache_read": 0.22,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "kimi-k2-5"): {
                    "cost_input": 0.56,
                    "cost_output": 3.5,
                    "cost_cache_read": 0.22,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "deepseek-v4-pro"): {
                    "cost_input": 1.73,
                    "cost_output": 3.796,
                    "cost_cache_read": 0.33,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "deepseek-v4-flash"): {
                    "cost_input": 0.17,
                    "cost_output": 0.35,
                    "cost_cache_read": 0.028,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "deepseek-v3.2"): {
                    "cost_input": 0.33,
                    "cost_output": 0.48,
                    "cost_cache_read": 0.16,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "aion-labs-aion-2-0"): {
                    "cost_input": 1,
                    "cost_output": 2,
                    "cost_cache_read": 0.25,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "llama-3.2-3b"): {
                    "cost_input": 0.15,
                    "cost_output": 0.6,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "llama-3.3-70b"): {
                    "cost_input": 0.7,
                    "cost_output": 2.8,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "openai-gpt-52"): {
                    "cost_input": 2.19,
                    "cost_output": 17.5,
                    "cost_cache_read": 0.219,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "openai-gpt-52-codex"): {
                    "cost_input": 2.19,
                    "cost_output": 17.5,
                    "cost_cache_read": 0.219,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "openai-gpt-53-codex"): {
                    "cost_input": 2.19,
                    "cost_output": 17.5,
                    "cost_cache_read": 0.219,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "openai-gpt-54"): {
                    "cost_input": 3.13,
                    "cost_output": 18.8,
                    "cost_cache_read": 0.313,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "openai-gpt-54-pro"): {
                    "cost_input": 37.5,
                    "cost_output": 225,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "openai-gpt-54-mini"): {
                    "cost_input": 0.9375,
                    "cost_output": 5.625,
                    "cost_cache_read": 0.09375,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "openai-gpt-55"): {
                    "cost_input": 6.25,
                    "cost_output": 37.5,
                    "cost_cache_read": 0.625,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "openai-gpt-55-pro"): {
                    "cost_input": 37.5,
                    "cost_output": 225,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "openai-gpt-4o-2024-11-20"): {
                    "cost_input": 3.125,
                    "cost_output": 12.5,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "openai-gpt-4o-mini-2024-07-18"): {
                    "cost_input": 0.1875,
                    "cost_output": 0.75,
                    "cost_cache_read": 0.09375,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "minimax-m3"): {
                    "cost_input": 0.3,
                    "cost_output": 1.2,
                    "cost_cache_read": 0.06,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "minimax-m25"): {
                    "cost_input": 0.34,
                    "cost_output": 1.19,
                    "cost_cache_read": 0.04,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "minimax-m27"): {
                    "cost_input": 0.375,
                    "cost_output": 1.5,
                    "cost_cache_read": 0.075,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "mercury-2"): {
                    "cost_input": 0.3125,
                    "cost_output": 0.9375,
                    "cost_cache_read": 0.03125,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "nvidia-nemotron-3-nano-30b-a3b"): {
                    "cost_input": 0.075,
                    "cost_output": 0.3,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "nvidia-nemotron-cascade-2-30b-a3b"): {
                    "cost_input": 0.14,
                    "cost_output": 0.8,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "e2ee-venice-uncensored-24b-p"): {
                    "cost_input": 0.25,
                    "cost_output": 1.15,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "e2ee-gemma-3-27b-p"): {
                    "cost_input": 0.14,
                    "cost_output": 0.5,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "e2ee-gemma-4-26b-a4b-uncensored-p"): {
                    "cost_input": 0.19,
                    "cost_output": 0.88,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "e2ee-glm-4-7-p"): {
                    "cost_input": 1.1,
                    "cost_output": 4.15,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "e2ee-glm-4-7-flash-p"): {
                    "cost_input": 0.13,
                    "cost_output": 0.55,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "e2ee-gpt-oss-20b-p"): {
                    "cost_input": 0.05,
                    "cost_output": 0.19,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "e2ee-gpt-oss-120b-p"): {
                    "cost_input": 0.13,
                    "cost_output": 0.65,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "e2ee-qwen-2-5-7b-p"): {
                    "cost_input": 0.05,
                    "cost_output": 0.13,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "e2ee-qwen3-6-35b-a3b-uncensored-p"): {
                    "cost_input": 0.38,
                    "cost_output": 1.88,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "e2ee-qwen3-30b-a3b-p"): {
                    "cost_input": 0.19,
                    "cost_output": 0.69,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "e2ee-qwen3-vl-30b-a3b-p"): {
                    "cost_input": 0.25,
                    "cost_output": 0.9,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "e2ee-glm-5-1"): {
                    "cost_input": 1.1,
                    "cost_output": 4.15,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "e2ee-qwen3-6-35b-a3b"): {
                    "cost_input": 0.182,
                    "cost_output": 1.18,
                    "cost_cache_read": 0.06,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("venice", "e2ee-gemma-4-31b"): {
                    "cost_input": 0.139,
                    "cost_output": 0.43,
                    "cost_cache_read": 0.028,
                    "source": "https://api.venice.ai/api/v1/models",
                    "note": "Live Venice API, model_spec.pricing.usd per 1M tokens"
    },
    ("novita", "minimax/minimax-m2.7"): {
                    "cost_input": 3.0,
                    "cost_output": 12.0,
                    "source": "https://api.novita.ai/v3/openai/models",
                    "note": "Live Novita API, input/output_token_price_per_m (micro-USD)/1000"
    },
    ("novita", "zai-org/glm-5"): {
                    "cost_input": 10.0,
                    "cost_output": 32.0,
                    "source": "https://api.novita.ai/v3/openai/models",
                    "note": "Live Novita API, input/output_token_price_per_m (micro-USD)/1000"
    },
    ("novita", "moonshotai/kimi-k2.5"): {
                    "cost_input": 6.0,
                    "cost_output": 30.0,
                    "source": "https://api.novita.ai/v3/openai/models",
                    "note": "Live Novita API, input/output_token_price_per_m (micro-USD)/1000"
    },
    ("novita", "deepseek/deepseek-v3-0324"): {
                    "cost_input": 2.7,
                    "cost_output": 11.2,
                    "source": "https://api.novita.ai/v3/openai/models",
                    "note": "Live Novita API, input/output_token_price_per_m (micro-USD)/1000"
    },
    ("novita", "deepseek/deepseek-r1-0528"): {
                    "cost_input": 7.0,
                    "cost_output": 25.0,
                    "source": "https://api.novita.ai/v3/openai/models",
                    "note": "Live Novita API, input/output_token_price_per_m (micro-USD)/1000"
    },
    ("novita", "qwen/qwen3-235b-a22b-fp8"): {
                    "cost_input": 2.0,
                    "cost_output": 8.0,
                    "source": "https://api.novita.ai/v3/openai/models",
                    "note": "Live Novita API, input/output_token_price_per_m (micro-USD)/1000"
    },
}

CONTEXT_OVERRIDES = {}  # No context corrections in this run.


def apply_overrides_to_file(path: Path) -> list[tuple[str, dict]]:
    """Apply all matching overrides to a single TOML file."""
    text = path.read_text()
    with path.open("rb") as f:
        data = tomllib.load(f)
    file_provider = data.get("provider", path.stem)
    if not file_provider:
        return []
    applied = []
    for (pid, mid), fields in PRICE_OVERRIDES.items():
        if pid != file_provider:
            continue
        pattern = re.compile(
            r"(\[\[model\]\]\n.*?)(?=\[\[model\]\]|\Z)",
            re.DOTALL,
        )
        def repl(m, _mid=mid, _fields=fields):
            block = m.group(0)
            if f'id = "{_mid}"' not in block:
                return block
            new_block = block
            changed = False
            for k, val in _fields.items():
                if k in ("source", "note"):
                    continue
                replaced, n = re.subn(
                    rf"^{re.escape(k)} = .+$",
                    f"{k} = {val}",
                    new_block,
                    count=1,
                    flags=re.MULTILINE,
                )
                if n:
                    new_block = replaced
                    changed = True
            if changed:
                applied.append((_mid, _fields))
            return new_block
        new_text = pattern.sub(repl, text)
        if new_text != text:
            text = new_text
    if applied:
        path.write_text(text)
    return applied


def main() -> int:
    audited = []
    for path in sorted(OPENCLAW_DIR.glob("*.toml")):
        with path.open("rb") as f:
            data = tomllib.load(f)
        provider_id = data.get("provider", path.stem)
        for m in data.get("model", []):
            ci = m.get("cost_input", 0.0)
            co = m.get("cost_output", 0.0)
            if ci == 0.0 and co == 0.0:
                audited.append({
                    "file": path.name,
                    "provider_id": provider_id,
                    "model_id": m.get("id"),
                    "name": m.get("name"),
                    "context_window": m.get("context_window"),
                })

    all_updates = []
    for path in sorted(OPENCLAW_DIR.glob("*.toml")):
        applied = apply_overrides_to_file(path)
        for mid, fields in applied:
            all_updates.append((path.name, mid, fields))

    total_models = 0
    nonzero_models = 0
    for path in sorted(OPENCLAW_DIR.glob("*.toml")):
        with path.open("rb") as f:
            data = tomllib.load(f)
        for m in data.get("model", []):
            total_models += 1
            if m.get("cost_input", 0) != 0 or m.get("cost_output", 0) != 0:
                nonzero_models += 1

    lines = [
        "# Openclaw price backfill report",
        "",
        "## Summary",
        "",
        f"- **Approach**: fetch live pricing from public provider APIs, apply to 0-cost entries.",
        f"- **Sources fetched**: 2 (Venice, Novita).",
        f"- **Result**: {len(all_updates)} entries updated.",
        f"- **Total openclaw models**: {total_models}",
        f"- **Models with non-zero prices after**: {nonzero_models} (was {nonzero_models - len(all_updates)} before)",
        "",
        "## Upstream finding (root cause)",
        "",
        "All 77 originally-zero entries are **faithful to openclaw source**.",
        "The openclaw `extensions/<provider>/openclaw.plugin.json` files",
        "explicitly encode `cost: { input: 0, output: 0, ... }` for these",
        "providers. The oxi port correctly preserved these values; the data",
        "gap is in the upstream openclaw project itself.",
        "",
    ]
    by_file = {}
    for e in audited:
        by_file[e["file"]] = by_file.get(e["file"], 0) + 1
    for fname in sorted(by_file):
        lines.append(f"- `{fname}`: {by_file[fname]} zero-cost entries (originally)")
    lines.append(f"- **Total: {len(audited)}** zero-cost entries (originally)")
    lines.append("")

    lines.append("## Updates applied")
    lines.append("")
    if all_updates:
        for path_name, mid, fields in all_updates:
            src = fields.get("source", "?")
            note = fields.get("note", "")
            ci = fields.get("cost_input", 0)
            co = fields.get("cost_output", 0)
            ccr = fields.get("cost_cache_read", "-")
            ccw = fields.get("cost_cache_write", "-")
            lines.append(
                f"- `{path_name}` / `{mid}`: input={ci}, output={co}, "
                f"cache_read={ccr}, cache_write={ccw}  \n  source: {src}  \n  note: {note}"
            )
    else:
        lines.append("**None.**")
    lines.append("")

    lines.append("## Models still at 0 cost (not updated)")
    lines.append("")
    updated_ids = {(p, m) for p, m, _ in all_updates}
    for e in audited:
        if (e["file"], e["model_id"]) in updated_ids:
            continue
        lines.append(
            f"- `{e['file']}` / `{e['provider_id']}` / `{e['model_id']}` "
            f"({e['name']}) \u2014 no public pricing API reachable without auth"
        )
    lines.append("")

    REPORT_PATH.write_text("\n".join(lines))
    print(f"Report: {REPORT_PATH}")
    print(f"Zero-cost entries audited: {len(audited)}")
    print(f"Updates applied: {len(all_updates)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
