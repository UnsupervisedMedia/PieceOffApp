#!/usr/bin/env python3
"""Write the licenses of every Rust crate PieceOff builds with.

    scripts/license-report.py [output-file]

Uses `cargo metadata`, so it lists crates for all platforms (a superset of any
one build). This is a summary for review, not legal advice.
"""
import json
import subprocess
import sys
from collections import defaultdict


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else "-"
    meta = json.loads(
        subprocess.check_output(
            ["cargo", "metadata", "--format-version", "1", "--locked"],
            text=True,
        )
    )
    workspace = set(meta["workspace_members"])
    by_license = defaultdict(list)
    missing = []
    for package in sorted(meta["packages"], key=lambda p: (p["name"], p["version"])):
        if package["id"] in workspace:
            continue
        license_text = package.get("license")
        if not license_text and package.get("license_file"):
            license_text = f"see {package['license_file']}"
        if not license_text:
            missing.append(f"{package['name']} {package['version']}")
            continue
        by_license[license_text.replace(" OR ", " / ")].append(
            f"{package['name']} {package['version']}"
        )
    lines = [
        "PieceOff third-party Rust crates",
        "================================",
        "Each crate is available under at least the terms shown. Where several",
        "licenses are listed (A / B), you may choose any one of them.",
        "",
    ]
    for license_text in sorted(by_license, key=lambda k: (-len(by_license[k]), k)):
        crates = by_license[license_text]
        lines.append(f"{license_text}  ({len(crates)} crates)")
        for crate in crates:
            lines.append(f"    {crate}")
        lines.append("")
    if missing:
        lines.append("No license declared (check each crate's own files):")
        lines.extend(f"    {crate}" for crate in missing)
        lines.append("")
    text = "\n".join(lines)
    if out == "-":
        print(text)
    else:
        with open(out, "w") as handle:
            handle.write(text)
        print(
            f"wrote {out}: {sum(len(v) for v in by_license.values())} crates, "
            f"{len(by_license)} license expressions, {len(missing)} undeclared"
        )


if __name__ == "__main__":
    main()
