#!/usr/bin/env python3
"""Fix vendored Cargo.toml: internal refs → oxi-vendor paths, inline workspace deps."""
import re, os
from pathlib import Path

OXI = Path("/Volumes/MERCURY/PROJECTS/oxi")
GROK = Path("/tmp/ref-porter/xai-org-grok-build")

# Parse grok workspace deps for version inlining
grok_ws = (GROK / "Cargo.toml").read_text()
grok_deps = {}
for m in re.finditer(r'^(\S+)\s*=\s*(\{.+\}|\".+\")', grok_ws, re.MULTILINE):
    grok_deps[m.group(1)] = m.group(2)

# Build oxi-vendor name map
name_map = {}
for d in os.listdir(OXI):
    if d.startswith('oxi-vendor-') and os.path.isdir(OXI / d) and d not in ('oxi-vendor-grok-shim', 'oxi-vendor-grok-pager'):
        suffix = d.replace('oxi-vendor-', '')
        name_map[suffix] = d
        name_map['xai-' + suffix] = d

FIXED = 0
for d in sorted(os.listdir(OXI)):
    if not d.startswith('oxi-vendor-') or not os.path.isdir(OXI / d):
        continue
    if d in ('oxi-vendor-grok-shim', 'oxi-vendor-grok-pager'):
        continue

    toml = OXI / d / 'Cargo.toml'
    if not toml.exists():
        continue
    content = toml.read_text()
    original = content

    # Fix package name
    for grok_name, oxi_name in name_map.items():
        if f'name = "{grok_name}"' in content:
            content = content.replace(f'name = "{grok_name}"', f'name = "{oxi_name}"')
            break

    # Fix edition
    content = content.replace('edition.workspace = true', 'edition = "2024"')

    # Fix internal path refs: xai-grok-foo = { workspace = true } → oxi-vendor-grok-foo = { path = ... }
    for grok_name, oxi_name in sorted(name_map.items(), key=lambda x: -len(x[0])):
        pattern = f'{grok_name} = {{ workspace = true'
        if pattern in content:
            content = content.replace(pattern, f'{oxi_name} = {{ path = "../{oxi_name}"')

    # Fix workspace + features variant for internal refs
    for grok_name, oxi_name in sorted(name_map.items(), key=lambda x: -len(x[0])):
        content = re.sub(
            rf'{re.escape(grok_name)}\s*=\s*\{{\s*workspace\s*=\s*true\s*,\s*features\s*=',
            f'{oxi_name} = {{ path = "../{oxi_name}", features =',
            content
        )

    # Inline remaining workspace = true deps
    def inline_ws(m):
        dep = m.group(1)
        if dep in grok_deps:
            val = grok_deps[dep]
            # Extract features if present
            feat_match = re.search(r'features\s*=\s*(\[.+?\])', m.group(0))
            if feat_match:
                if '{' in val:
                    return f'{dep} = {val.rstrip(" }")}, features = {feat_match.group(1)} }}'
                else:
                    return f'{dep} = {{ version = {val}, features = {feat_match.group(1)} }}'
            return f'{dep} = {val}'
        return m.group(0)

    content = re.sub(
        r'^(\S+)\s*=\s*\{\s*workspace\s*=\s*true[^}]*\}',
        inline_ws,
        content,
        flags=re.MULTILINE
    )

    if content != original:
        toml.write_text(content)
        FIXED += 1
        print(f"FIXED: {d}")
    else:
        print(f"OK:    {d}")

print(f"\nFixed {FIXED} crates.")
