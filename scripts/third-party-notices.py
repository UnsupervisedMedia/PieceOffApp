#!/usr/bin/env python3
"""Collect third-party license texts and notices for the locked Cargo graph.

    scripts/third-party-notices.py [output-file]

This includes crate metadata, legal files shipped with each resolved crate,
and the license texts for epaint's embedded default fonts. It fails if a
package declares no license, so new dependencies need review before release.
"""
import hashlib
import json
import os
import subprocess
import sys


LEGAL_NAME = ("license", "copying", "notice", "copyright", "ofl", "ufl")


def legal_file(path):
    name = os.path.basename(path).lower()
    return any(token in name for token in LEGAL_NAME)


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else "-"
    meta = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--locked"], text=True
    ))
    workspace = set(meta["workspace_members"])
    entries = {}
    missing = []
    packages = []
    for pkg in sorted(meta["packages"], key=lambda p: (p["name"], p["version"])):
        if pkg["id"] in workspace:
            continue
        license_expr = pkg.get("license") or ("file: " + pkg["license_file"] if pkg.get("license_file") else "")
        if not license_expr:
            missing.append(f"{pkg['name']} {pkg['version']}: missing declared license")
            continue
        authors = ", ".join(pkg.get("authors") or []) or "not declared in Cargo metadata"
        packages.append(f"{pkg['name']} {pkg['version']} | {license_expr} | {authors}")
        root = os.path.dirname(pkg["manifest_path"])
        paths = []
        for current, dirs, files in os.walk(root):
            dirs[:] = [d for d in dirs if d not in {".git", "target", "tests", "examples"}]
            for filename in files:
                path = os.path.join(current, filename)
                rel = os.path.relpath(path, root).replace(os.sep, "/")
                if legal_file(rel) and os.path.getsize(path) <= 1_000_000:
                    paths.append((rel, path))
        # epaint embeds fonts; their package keeps the notices in fonts/*.txt,
        # including Hack's attribution file which doesn't have a legal name.
        if pkg["name"] == "epaint_default_fonts":
            fonts = os.path.join(root, "fonts")
            if os.path.isdir(fonts):
                paths.extend(("fonts/" + n, os.path.join(fonts, n))
                             for n in os.listdir(fonts) if n.lower().endswith(".txt"))
        paths = sorted(set(paths))
        for rel, path in paths:
            with open(path, "rb") as f:
                data = f.read()
            key = hashlib.sha256(data).hexdigest()
            item = entries.setdefault(key, {"data": data, "refs": []})
            item["refs"].append(f"{pkg['name']} {pkg['version']} — {rel} (declared {license_expr})")
    if missing:
        sys.stderr.write("Cannot create release notices; review these packages:\n")
        sys.stderr.write("\n".join("  " + line for line in missing) + "\n")
        return 1
    chunks = [
        "PieceOff third-party license texts and notices",
        "==============================================",
        "Generated from Cargo.lock and the local Cargo registry sources. Each notice below is retained from its upstream crate source. The applicable license is identified by the crate's Cargo metadata. This file does not change any third-party license terms.",
        "",
        "Resolved third-party crate metadata (name | version | declared SPDX license | Cargo author attribution):",
        *packages,
        "",
        "The crate registry may omit a separate license file from a published package archive. For those packages, the declared license expression and author metadata above are retained; all license and notice files present in the resolved crate sources follow below. Review upstream provenance when changing dependencies.",
        "",
    ]
    for key in sorted(entries):
        item = entries[key]
        chunks.append("Packages and source files:")
        chunks.extend("  " + ref for ref in sorted(set(item["refs"])))
        chunks.append("SHA-256: " + key)
        chunks.append("-" * 72)
        chunks.append(item["data"].decode("utf-8", errors="replace").rstrip())
        chunks.extend(["", "=" * 72, ""])
    text = "\n".join(chunks)
    if out == "-":
        sys.stdout.write(text)
    else:
        with open(out, "w", encoding="utf-8", newline="\n") as f:
            f.write(text)
        print(f"wrote {out}: {len(entries)} distinct notice/license texts")
    return 0


if __name__ == "__main__":
    sys.exit(main())
