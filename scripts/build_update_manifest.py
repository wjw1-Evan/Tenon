#!/usr/bin/env python3
"""聚合 Tauri 更新器所需 latest.json（v1.90 / §6.2）。

用法：build_update_manifest.py <updater目录> <release资产目录> <输出文件> <vX.Y.Z>
所有目标都必须同时存在产物和 minisign 签名；缺失或空签名时 fail closed。
"""

from __future__ import annotations

import datetime as dt
import json
import sys
from pathlib import Path
from typing import Iterator
from urllib.parse import quote

# latest.json 平台键（Tauri 约定）-> (release.yml 归集命名前缀 = Rust target triple, 产物 glob)
PLATFORM_GLOBS = {
    "darwin-aarch64": ("aarch64-apple-darwin", "*-*.app.tar.gz"),
    "darwin-x86_64": ("x86_64-apple-darwin", "*-*.app.tar.gz"),
    "linux-x86_64": ("x86_64-unknown-linux-gnu", "*-*.AppImage"),
    "windows-x86_64": ("x86_64-pc-windows-msvc", "*-*-setup.exe"),
}


def signed_assets(updater: Path, prefix: str, pattern: str) -> Iterator[tuple[Path, Path]]:
    for artifact in sorted(updater.glob(pattern)):
        if artifact.name.endswith(".sig"):
            continue
        signature = artifact.with_name(artifact.name + ".sig")
        yield artifact, signature


def main() -> int:
    if len(sys.argv) != 5:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    updater_dir = Path(sys.argv[1])
    _assets_dir = Path(sys.argv[2])
    output = Path(sys.argv[3])
    tag = sys.argv[4]
    if not tag.startswith("v") or not tag[1:].replace(".", "").isdigit():
        print(f"非法 tag：{tag}", file=sys.stderr)
        return 2

    platforms: dict[str, dict[str, str]] = {}
    for platform, (prefix, pattern) in PLATFORM_GLOBS.items():
        matches = list(signed_assets(updater_dir, prefix, pattern))
        matches = [(a, s) for a, s in matches if a.name.startswith(f"{prefix}-")]
        if len(matches) != 1:
            print(
                f"{platform}: 期望一个更新产物，实际 {len(matches)} 个 "
                f"({', '.join(a.name for a, _ in matches) or '无'})",
                file=sys.stderr,
            )
            return 1
        artifact, signature = matches[0]
        signature_text = signature.read_text(encoding="utf-8").strip()
        if not signature_text:
            print(f"{signature.name}: 签名为空", file=sys.stderr)
            return 1
        platforms[platform] = {
            "signature": signature_text,
            "url": "https://github.com/wjw1-Evan/Tenon/releases/download/"
            f"{tag}/{quote(artifact.name)}",
        }

    manifest = {
        "version": tag[1:],
        "notes": f"Tenon {tag[1:]} release notes: see GitHub Release.",
        "pub_date": dt.datetime.now(dt.timezone.utc).isoformat().replace("+00:00", "Z"),
        "platforms": platforms,
    }
    output.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print(f"latest.json: {output} ({', '.join(platforms)})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
