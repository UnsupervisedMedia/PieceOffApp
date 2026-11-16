#!/usr/bin/env python3
"""Collect third-party license texts and notices for the locked Cargo graph.

    scripts/third-party-notices.py [output-file]
    scripts/third-party-notices.py --web META.json NODE_MODULES [output-file]

With --web the notices are for the browser client release asset (web/pack.sh)
instead: only the Rust crates compiled into pieceoff-web-wasm (normal
dependencies for wasm32-unknown-unknown) plus the npm packages that esbuild's
metafile (web/build/meta.json) shows inside the shipped bundle.

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


def wasm_reachable(meta, root_name="pieceoff-web-wasm"):
    """Ids of the packages the web wasm links: normal deps of root_name, transitively."""
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    root = next(p["id"] for p in meta["packages"] if p["name"] == root_name)
    seen, todo = set(), [root]
    while todo:
        cur = todo.pop()
        if cur in seen:
            continue
        seen.add(cur)
        for dep in nodes[cur]["deps"]:
            if any(k.get("kind") is None for k in dep["dep_kinds"]):
                todo.append(dep["pkg"])
    return seen


def bundled_npm(meta_path, node_modules):
    """(name, version, license, [(rel, path)]) for npm packages inside the shipped bundle."""
    with open(meta_path, encoding="utf-8") as f:
        outputs = json.load(f)["outputs"]
    roots = set()
    for out in outputs.values():
        for src in out.get("inputs", {}):
            parts = src.replace("\\", "/").split("/")
            if "node_modules" in parts:
                i = len(parts) - 1 - parts[::-1].index("node_modules")
                n = 2 if parts[i + 1].startswith("@") else 1
                roots.add("/".join(parts[i + 1:i + 1 + n]))
    found = []
    for name in sorted(roots):
        root = os.path.join(node_modules, name)
        with open(os.path.join(root, "package.json"), encoding="utf-8") as f:
            pj = json.load(f)
        lic = pj.get("license") or ""
        paths = [(n, os.path.join(root, n)) for n in sorted(os.listdir(root))
                 if legal_file(n) and os.path.isfile(os.path.join(root, n))]
        found.append((name, pj["version"], lic, paths, pj.get("author")))
    return found


def main():
    args = sys.argv[1:]
    web = None
    if args[:1] == ["--web"]:
        if len(args) < 3:
            sys.stderr.write(__doc__)
            return 2
        web = (args[1], args[2])
        args = args[3:]
    out = args[0] if args else "-"
    cmd = ["cargo", "metadata", "--format-version", "1", "--locked"]
    if web:
        cmd += ["--filter-platform", "wasm32-unknown-unknown"]
    meta = json.loads(subprocess.check_output(cmd, text=True))
    workspace = set(meta["workspace_members"])
    keep = wasm_reachable(meta) if web else None
    entries = {}
    missing = []
    packages = []
    for pkg in sorted(meta["packages"], key=lambda p: (p["name"], p["version"])):
        if pkg["id"] in workspace or (keep is not None and pkg["id"] not in keep):
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
    if web:
        for name, version, lic, npaths, author in bundled_npm(*web):
            if not lic or not npaths:
                missing.append(f"npm {name} {version}: missing license or license file")
                continue
            a = author if isinstance(author, str) else (author or {}).get("name", "") if author else ""
            packages.append(f"{name} {version} (npm) | {lic} | {a or 'not declared in package.json'}")
            for rel, path in npaths:
                with open(path, "rb") as f:
                    data = f.read()
                item = entries.setdefault(hashlib.sha256(data).hexdigest(), {"data": data, "refs": []})
                item["refs"].append(f"{name} {version} (npm) — {rel} (declared {lic})")
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
