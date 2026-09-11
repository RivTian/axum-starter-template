#!/usr/bin/env python3
"""把生成目录里的改动同步回模板源码（反向替换占位符）。

模板源码含 `{{crate_prefix}}` 一类占位符，rustfmt 解析不了，所以 `make fmt`
在生成目录里格式化，再由本脚本把结果写回模板。规则刻意保守：

- 只回写**模板里已经存在的 `.rs` 文件**；生成目录里多出来的（Cargo.lock、target/ …）
  不碰，非 Rust 文件也不碰（它们可能含 `{{authors}}` 这类反向映射不认识的内置占位符）；
- cargo-generate.toml 里 `ignore` 的文件不碰（它们不在生成目录里）；`exclude` 的
  文件照常回写：它们没经过 Liquid、也不含占位符，反向替换是恒等变换，但
  rustfmt 的结果同样需要带回来；
- 反向替换按 token 长度从长到短做，五个 token 由 Makefile 的两个开发用名字派生，
  与 hooks/pre.rhai 的派生规则保持一致。
"""

from __future__ import annotations

import argparse
import sys
import tomllib
from pathlib import Path

# 只回写 Rust 源码：rustfmt 只改 .rs；其他文件可能含反向映射不认识的内置占位符
# （如 Cargo.toml 的 `{{authors}}`），回写会把展开值当成源码写回去
SYNC_SUFFIXES = {".rs"}


def tokens(project_name: str, crate_prefix: str) -> list[tuple[str, str]]:
    crate_name = project_name.replace("-", "_")
    env_prefix = crate_name.upper()
    pairs = [
        (env_prefix, "{{env_prefix}}"),
        (project_name, "{{project-name}}"),
        (crate_name, "{{crate_name}}"),
        (f"{crate_prefix}-", "{{crate_prefix}}-"),
        (f"{crate_prefix.replace('-', '_')}_", "{{crate_prefix_snake}}_"),
    ]
    # 长 token 先替换，短 token 才不会把长 token 拆坏
    return sorted(pairs, key=lambda p: -len(p[0]))


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--gen-dir", required=True, type=Path)
    ap.add_argument("--project-name", required=True)
    ap.add_argument("--crate-prefix", required=True)
    args = ap.parse_args()

    root = Path(__file__).resolve().parent.parent
    cfg = tomllib.loads((root / "cargo-generate.toml").read_text(encoding="utf-8"))
    tpl = cfg.get("template", {})
    ignored = set(tpl.get("ignore", [])) | {".git", "cargo-generate.toml"}
    pairs = tokens(args.project_name, args.crate_prefix)

    changed = 0
    for src in root.rglob("*"):
        if not src.is_file() or src.suffix not in SYNC_SUFFIXES:
            continue
        rel = src.relative_to(root)
        if rel.parts[0] in ignored or str(rel) in ignored:
            continue
        gen = args.gen_dir / rel
        if not gen.is_file():
            continue
        text = gen.read_text(encoding="utf-8")
        for concrete, placeholder in pairs:
            text = text.replace(concrete, placeholder)
        if text != src.read_text(encoding="utf-8"):
            src.write_text(text, encoding="utf-8")
            changed += 1
            print(f"synced {rel}")
    print(f"{changed} file(s) updated")
    return 0


if __name__ == "__main__":
    sys.exit(main())
