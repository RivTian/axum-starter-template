#!/usr/bin/env python3
"""apply-slices.sh 的实现：把 README 的两条切片插入生成项目。

单独放一个文件（而不是内联 heredoc）是为了让 shell 与 Python 的语法互不干扰。
"""

import pathlib
import sys


def insert_after_marker(path: pathlib.Path, marker: str, block: str) -> None:
    lines = path.read_text().splitlines(keepends=True)
    for index, line in enumerate(lines):
        if marker in line:
            lines.insert(index + 1, block if block.endswith("\n") else block + "\n")
            path.write_text("".join(lines))
            return
    raise SystemExit(f"找不到锚点 `{marker}`：{path}")


def insert_sorted(path: pathlib.Path, prefix: str, statement: str) -> None:
    """把 statement 插到以 prefix 开头的语句之间，保持字母序（rustfmt 会重排 mod 与 use）。"""
    lines = path.read_text().splitlines(keepends=True)
    positions = [i for i, line in enumerate(lines) if line.startswith(prefix)]
    if not positions:
        raise SystemExit(f"找不到 `{prefix}` 语句：{path}")
    target = positions[-1] + 1
    for index in positions:
        if lines[index].strip() > statement.strip():
            target = index
            break
    lines.insert(target, statement if statement.endswith("\n") else statement + "\n")
    path.write_text("".join(lines))


def main() -> None:
    project = pathlib.Path(sys.argv[1])
    slices = pathlib.Path(sys.argv[2])
    which = sys.argv[3]

    if which in ("task", "both"):
        (project / "crates/app/src/flush.rs").write_text((slices / "task/flush.rs").read_text())
        insert_sorted(project / "crates/app/src/lib.rs", "mod ", "mod flush;")
        insert_after_marker(
            project / "crates/app/src/assembly.rs",
            "你自己的任务面注册在这里",
            (slices / "task/register.rs").read_text(),
        )
        print("slice task: applied")

    if which in ("repo", "both"):
        (project / "crates/storage/migrations/0001_create_notes.sql").write_text(
            (slices / "repo/migrations/0001_create_notes.sql").read_text()
        )
        (project / "crates/storage/src/notes.rs").write_text(
            (slices / "repo/notes.rs").read_text()
        )
        storage_lib = project / "crates/storage/src/lib.rs"
        insert_sorted(storage_lib, "mod ", "mod notes;")
        insert_sorted(storage_lib, "pub use ", "pub use notes::NotesRepo;")
        (project / "crates/app/src/writer.rs").write_text((slices / "repo/writer.rs").read_text())
        insert_sorted(project / "crates/app/src/lib.rs", "mod ", "mod writer;")
        insert_after_marker(
            project / "crates/app/src/assembly.rs",
            "你自己的任务面注册在这里",
            (slices / "repo/register.rs").read_text(),
        )
        print("slice repo: applied")


if __name__ == "__main__":
    main()
