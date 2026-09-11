#!/usr/bin/env python3
"""模板的「换个名字仍然 fmt-clean」门禁。

模板仓库本身不可 rustfmt（源码里是 `{{...}}`），格式化只发生在生成目录里。
于是有一类缺陷只在**换一组名字**之后才暴露：rustfmt 的决策取决于替换**之后**
的行宽与排序，同一份模板对 `tpl-project/tplx` 是 fmt-clean 的，对
`acme-billing-service/acme` 就不是。`make check` 只跑一组名字，抓不到。

这个脚本静态地抓两类：

1. **宽度翻转**：一行在短名字下 ≤ 100 列、在长名字下 > 100 列。rustfmt 会在
   后者重排这一行，模板里存的却是前者的排法。修法是把项目名绑到 `const` /
   `let` 上，让表达式的宽度与名字无关（长字符串字面量本身不会被拆，
   `format_strings` 默认关）。
2. **表达式里的全限定路径**：`{{crate_prefix_snake}}_core::AppError::Storage(_)` 这种
   写法把 crate 前缀带进了表达式宽度，而表达式还受 `fn_call_width` / `chain_width`
   这些 60 列的子阈值管——比 100 列更容易被撞上，且难以静态建模。一律写进 `use`
   （需要保留「跨 crate」的可读性就取别名：`use x_api as api;`）。
3. **import 分组混排**：同一个空行分隔的 `use` 组里既有 `<prefix>_xxx` 的
   workspace 依赖、又有别的根（std / 第三方 / crate / super）。组内排序按替换后的
   名字走，`acme_core` 排在 `std` 前、`tplx_core` 排在 `std` 后——同一份模板
   只对其中一半名字成立。修法是给 workspace 依赖单独一组（空行隔开）。

用法：`python3 scripts/check-fmt-portability.py`（`make fmt-portable`）。
"""

import pathlib
import re
import sys
import unicodedata

MAX_WIDTH = 100

# 名字的取值范围取得比现实宽一档：门禁宁可早响，也不要等到某个用户用了长名字才发现
SHORT = {
    "project-name": "ab",
    "crate_name": "ab",
    "crate_prefix": "a",
    "crate_prefix_snake": "a",
    "env_prefix": "AB",
}
LONG = {
    "project-name": "a" * 40,
    "crate_name": "a" * 40,
    "crate_prefix": "a" * 20,
    "crate_prefix_snake": "a" * 20,
    "env_prefix": "A" * 40,
}

# 已知安全的宽度翻转：rustfmt 拆不动的东西，超宽也只能原样留着。
# 每一条都要写清楚为什么——这个名单是用来讨论的，不是用来堆的。
WIDTH_ALLOW = [
    (
        "core/src/config/app.rs",
        "_NEVER_SET_HTTP_HOST",
        "已经是 fs::write 竖排参数表里独占一行的字符串字面量；字面量拆不开，超宽 rustfmt 原样留",
    ),
]

WS_PLACEHOLDER = "{{crate_prefix_snake}}_"
PLACEHOLDER = re.compile(r"\{\{([^}]*)\}\}")
SKIP_DIRS = {"target", ".git", "coverage", "tmp"}


def width(s: str) -> int:
    """终端/rustfmt 口径的显示宽度：CJK 全角算两列"""
    return sum(2 if unicodedata.east_asian_width(c) in "WF" else 1 for c in s)


def substitute(line: str, names: dict) -> str:
    return PLACEHOLDER.sub(lambda m: names.get(m.group(1).strip(), "?"), line)


def sources(root: pathlib.Path):
    for path in sorted(root.rglob("*.rs")):
        if SKIP_DIRS & set(path.parts):
            continue
        yield path


def allowed(rel: str, line: str) -> bool:
    return any(rel == f and needle in line for f, needle, _ in WIDTH_ALLOW)


def check_width(rel: str, lines: list) -> list:
    out = []
    for i, line in enumerate(lines, 1):
        if "{{" not in line or line.lstrip().startswith("//"):
            continue  # 注释不被 rustfmt 重排（wrap_comments 默认关）
        if allowed(rel, line):
            continue
        short, long = width(substitute(line, SHORT)), width(substitute(line, LONG))
        if short <= MAX_WIDTH < long:
            out.append(f"{rel}:{i}: 宽度随名字翻转（{short} -> {long} 列）: {line.strip()}")
    return out


def check_qualified_paths(rel: str, lines: list) -> list:
    """workspace crate 的路径只允许出现在 `use` 里，不许留在表达式中间"""
    out = []
    for i, line in enumerate(lines, 1):
        stripped = line.lstrip()
        if WS_PLACEHOLDER not in line or stripped.startswith(("//", "use ")):
            continue
        out.append(
            f"{rel}:{i}: 表达式里出现 workspace crate 的全限定路径，"
            f"宽度会随 crate 前缀变化；改成 use 导入（可用 `as` 取别名）: {stripped}"
        )
    return out


def check_import_groups(rel: str, lines: list) -> list:
    """把空行分隔的 use 组切出来，检查组内是否混了 workspace 依赖与别的根"""
    out = []
    group, start = [], 0
    for i, line in enumerate(lines + [""], 1):
        if line.startswith("use "):
            if not group:
                start = i
            group.append(line)
            continue
        if group:
            roots = {"ws" if "{{crate_prefix_snake}}_" in u else "other" for u in group}
            if len(roots) > 1:
                out.append(
                    f"{rel}:{start}: use 组里混了 workspace 依赖与其他根，"
                    f"排序会随 crate 前缀翻转；给 workspace 依赖单独一组"
                )
            group = []
    return out


def main() -> int:
    root = pathlib.Path(__file__).resolve().parent.parent
    problems = []
    for path in sources(root):
        rel = str(path.relative_to(root))
        lines = path.read_text(encoding="utf-8").splitlines()
        problems += check_width(rel, lines)
        problems += check_qualified_paths(rel, lines)
        problems += check_import_groups(rel, lines)
    if problems:
        print("fmt 可移植性检查失败：换一组名字后 rustfmt 会重排下列位置\n")
        for p in problems:
            print(f"  {p}")
        print(f"\n共 {len(problems)} 处")
        return 1
    print("fmt-portable: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
