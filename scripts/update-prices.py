#!/usr/bin/env python3
"""Refreshes the model prices shipped with PilotHouse for the usage
dashboard's estimate (crates/workspace/src/usage_view/prices.json) from
LiteLLM's public price list. Run it before a release and commit the result.

usage: scripts/update-prices.py
"""
import json
import os
import time
import urllib.request

SOURCE = "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json"
OUT = os.path.join(os.path.dirname(__file__), "..", "crates", "workspace", "src", "usage_view", "prices.json")
# The names the app looks models up by (see prices.rs `Prices::find`).
PREFIXES = ("", "anthropic/", "openai/", "gemini/", "vertex_ai/")
FAMILIES = ("claude", "gpt", "o1", "o3", "o4", "codex", "gemini")


def wanted(name):
    for prefix in PREFIXES:
        rest = name[len(prefix):]
        if name.startswith(prefix) and "/" not in rest and rest.startswith(FAMILIES):
            return True
    return False


def main():
    with urllib.request.urlopen(SOURCE, timeout=60) as response:
        listed = json.load(response)
    models = {}
    for name, entry in listed.items():
        name = name.lower()
        if not isinstance(entry, dict) or not wanted(name):
            continue
        inp, out = entry.get("input_cost_per_token"), entry.get("output_cost_per_token")
        if not isinstance(inp, (int, float)) or not isinstance(out, (int, float)):
            continue
        models[name] = {
            "input": inp,
            "output": out,
            "cache_read": entry.get("cache_read_input_token_cost"),
            "cache_write": entry.get("cache_creation_input_token_cost"),
        }
    if not models:
        raise SystemExit("no prices found")
    with open(OUT, "w") as file:
        json.dump({"fetched_at": int(time.time() * 1000), "models": dict(sorted(models.items()))}, file, separators=(",", ":"))
        file.write("\n")
    print(f"{len(models)} models written to {os.path.relpath(OUT)}")


if __name__ == "__main__":
    main()
