#!/usr/bin/env python3
"""Lint for the template repository.

The generated project must stand on its own (no references to the task brief, design
documents or upstream source lines), the template must avoid cargo-generate pitfalls, and
the crates must keep the structure rules (file size, no mod.rs, nesting depth).

usage: scripts/template-lint.py [--root DIR] [--self-test]

--self-test plants one violation per rule in a copy of the repository and requires the lint
to report exactly that rule; the unchanged copy must pass.
"""
import os
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib

ALLOWED_VARIABLES = {"project-name", "license", "with_docker", "with_ci", "authors", "year"}
ALLOWED_FILTERS = {"shouty_snake_case"}
ALLOWED_TAGS = {"if", "elsif", "else", "endif"}
LIQUID_WORDS = {"and", "or", "contains", "true", "false"}
NOT_RENDERED = {"cargo-generate.toml", ".genignore"}
SOURCE_TAGS = re.compile(r"\[(Pumpkin|Pingora|ratatui|cargo-generate|本模板)\]")
BRIEF_IDS = re.compile(r"\b((FD|HL|EI|LT|XB|EC|V6|[LDFPCUHXESBTNKMG])-\d+|(R|M|EI|CR|LT|WP|HL|XB|EC|PC|PQ|PN)\d+)\b")
UPSTREAM_LINE = re.compile(r"\S+\.(rs|toml|md|ya?ml)(:\d+|#L\d+)")
LIQUID_IN_EXCLUDED = re.compile(r"\{\{\s*(project-name|license|with_docker|with_ci|authors|year)\b|\{%")
VENDORED = re.compile(r"crates/[^/]+-util/src/error/pingora(/immut_str)?\.rs")
MAX_LINES = 500
SOFT_CODE_LINES = 300
MAX_DEPTH = 3  # src/<module>/<submodule>/<file>.rs


def repo_files(root):
    """Tracked files plus untracked files that git would not ignore."""
    out = subprocess.run(["git", "-C", root, "ls-files", "--cached", "--others", "--exclude-standard"],
                         capture_output=True, text=True, check=True).stdout
    return sorted(p for p in out.splitlines() if p)


def glob_regex(pattern):
    anchored = "/" in pattern.rstrip("/")
    pattern = pattern.strip("/")
    out, i = [], 0
    while i < len(pattern):
        if pattern.startswith("**/", i):
            out.append("(?:.*/)?"); i += 3
        elif pattern.startswith("**", i):
            out.append(".*"); i += 2
        elif pattern[i] == "*":
            out.append("[^/]*"); i += 1
        else:
            out.append(re.escape(pattern[i])); i += 1
    return re.compile(("^" if anchored else "^(?:.*/)?") + "".join(out) + "$")


def excluded(path, patterns):
    parts = path.split("/")
    candidates = ["/".join(parts[:n]) for n in range(1, len(parts) + 1)]
    result = False
    for raw in patterns:
        negate = raw.startswith("!")
        rx = glob_regex(raw[1:] if negate else raw)
        if any(rx.match(c) for c in candidates):
            result = not negate
    return result


def read_text(path):
    try:
        with open(path, encoding="utf-8") as f:
            return f.read()
    except (UnicodeDecodeError, IsADirectoryError, FileNotFoundError):
        return None


def liquid_problems(text):
    for m in re.finditer(r"\{\{-?(.*?)-?\}\}", text, re.S):
        parts = [p.strip() for p in m.group(1).split("|")]
        if parts[0] not in ALLOWED_VARIABLES:
            yield f"unknown variable {parts[0]!r}"
        for f in parts[1:]:
            if f.split(":")[0].strip() not in ALLOWED_FILTERS:
                yield f"unknown filter {f!r}"
    for m in re.finditer(r"\{%-?(.*?)-?%\}", text, re.S):
        words = m.group(1).split()
        if not words or words[0] not in ALLOWED_TAGS:
            yield f"unknown tag {' '.join(words[:1])!r}"
            continue
        expr = re.sub(r'"[^"]*"', " ", " ".join(words[1:]))
        for ident in re.findall(r"[A-Za-z_][A-Za-z0-9_-]*", expr):
            if ident not in ALLOWED_VARIABLES and ident not in LIQUID_WORDS:
                yield f"unknown name {ident!r} in tag"


def lint(root, warnings=None):
    findings = []

    def report(rule, path, message):
        findings.append((rule, path, message))

    files = repo_files(root)
    tdir = os.path.join(root, "template")
    try:
        with open(os.path.join(tdir, "cargo-generate.toml"), "rb") as f:
            config = tomllib.load(f)
    except FileNotFoundError:
        config = {}
    patterns = config.get("template", {}).get("exclude", [])
    template_files = [p[len("template/"):] for p in files if p.startswith("template/")]

    for rel in template_files:
        path = "template/" + rel
        text = read_text(os.path.join(tdir, rel))
        if text is None:
            continue
        if "§" in text or "docs/design" in text:
            report("self-contained", path, "reference to a design document")
        for rx, what in ((BRIEF_IDS, "task brief identifier"), (UPSTREAM_LINE, "upstream path:line"),
                         (SOURCE_TAGS, "source tag")):
            for m in rx.finditer(text):
                report("self-contained", path, f"{what} {m.group(0)}")
        is_hook = rel.startswith("hooks/")
        if not is_hook and rel not in NOT_RENDERED:
            if not excluded(rel, patterns):
                for problem in liquid_problems(text):
                    report("liquid", path, problem)
                if "${{" in text:
                    report("liquid", path, "GitHub Actions expression ${{ in a rendered file")
            elif LIQUID_IN_EXCLUDED.search(text):
                report("liquid", path, "Liquid in a file that is copied without rendering")
        if rel.endswith(".liquid"):
            report("liquid", path, "file name ends with .liquid")
        if VENDORED.fullmatch(rel):
            if "Licensed under the Apache License, Version 2.0" not in text:
                report("vendored", path, "lacks the upstream license header")
            if "Modified by the project authors" not in text:
                report("vendored", path, "lacks the modification notice")
        m = re.fullmatch(r"crates/[^/]+/(src|tests)/(.+)\.rs", rel)
        if m and not VENDORED.fullmatch(rel):
            lines = text.count("\n")
            if lines > MAX_LINES:
                report("structure", path, f"{lines} lines, more than {MAX_LINES}")
            if m.group(2).endswith("mod") and m.group(1) == "src":
                report("structure", path, "mod.rs: use foo.rs next to foo/")
            if m.group(1) == "src" and m.group(2).count("/") >= MAX_DEPTH:
                report("structure", path, "nested deeper than one directory below a top-level module")
            code = text.split("#[cfg(test)]")[0].count("\n")
            if m.group(1) == "src" and code > SOFT_CODE_LINES and warnings is not None:
                warnings.append(f"{path}: {code} lines of code, more than {SOFT_CODE_LINES}; give the reason in docs/log.md")

    if any(VENDORED.fullmatch(rel) for rel in template_files):
        r = subprocess.run([sys.executable, os.path.join(root, "scripts", "vendor-pingora-error.py"), "--check-patches"],
                           capture_output=True, text=True)
        if r.returncode != 0:
            report("vendored", "template/crates/*-util/src/error", (r.stderr or r.stdout).strip())

    configs = [p for p in files if os.path.basename(p) == "cargo-generate.toml"]
    if configs != ["template/cargo-generate.toml"]:
        report("cargo-generate", "template/cargo-generate.toml", f"expected exactly this file, found {configs}")
    for raw in patterns:
        pattern = raw.lstrip("!")
        if "{{" in pattern or pattern.endswith("/") or pattern.rstrip("/").split("/")[-1] in ("*", "**"):
            report("cargo-generate", "template/cargo-generate.toml", f"exclude {raw!r} is not a file-level glob")
    for name, spec in config.get("placeholders", {}).items():
        if not re.fullmatch(r"[a-z][a-z0-9_]*", name) or "default" not in spec:
            report("cargo-generate", "template/cargo-generate.toml", f"placeholder {name!r} needs a snake_case name and a default")
        elif "choices" in spec and spec["default"] not in spec["choices"]:
            report("cargo-generate", "template/cargo-generate.toml", f"default of {name!r} is not one of its choices")

    hooks = {rel: read_text(os.path.join(tdir, rel)) or "" for rel in template_files if rel.startswith("hooks/")}
    for rel, text in hooks.items():
        if "system::command" in text:
            report("hooks", "template/" + rel, "hooks must not run commands")
        if "variable::prompt" in text and rel != "hooks/init.rhai":
            report("hooks", "template/" + rel, "only the init hook may prompt")
    if 'file::delete("hooks")' not in hooks.get("hooks/pre.rhai", ""):
        report("hooks", "template/hooks/pre.rhai", "the pre hook must delete the hooks directory")

    for p in files:
        if p.startswith(("generated/", "target/")) or "/target/" in p or p == "template/Cargo.lock":
            report("artifacts", p, "rendered output or build artifact in the repository")
        if os.path.basename(p) == ".DS_Store":
            report("artifacts", p, ".DS_Store is tracked")
    if ".DS_Store" not in (read_text(os.path.join(tdir, ".genignore")) or "").split():
        report("artifacts", "template/.genignore", "must list .DS_Store")
    return findings


def copy_repo(root, dest):
    for rel in repo_files(root):
        src = os.path.join(root, rel)
        if os.path.isfile(src):
            os.makedirs(os.path.dirname(os.path.join(dest, rel)), exist_ok=True)
            shutil.copy2(src, os.path.join(dest, rel))
    subprocess.run(["git", "init", "-q", dest], check=True)


def append(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "a", encoding="utf-8") as f:
        f.write(text)


BIN_LIB = "template/crates/{{project-name}}/src/lib.rs"
PLANTS = {
    "self-contained": lambda r: append(os.path.join(r, BIN_LIB), "// As decided in D-28.\n"),
    "liquid": lambda r: append(os.path.join(r, BIN_LIB), "// {{project-name}}\n"),
    "vendored": lambda r: append(os.path.join(r, "template/crates/{{project-name}}-util/src/error/pingora.rs"), "//! Error.\n"),
    "structure": lambda r: append(os.path.join(r, "template/crates/{{project-name}}-util/src/a/mod.rs"), "//! A.\n"),
    "cargo-generate": lambda r: append(os.path.join(r, "extra/cargo-generate.toml"), "[template]\n"),
    "hooks": lambda r: append(os.path.join(r, "template/hooks/pre.rhai"), 'let listing = system::command("ls", []);\n'),
    "artifacts": lambda r: append(os.path.join(r, "generated/README.md"), "rendered\n"),
}


def self_test(root):
    failures = 0
    with tempfile.TemporaryDirectory() as tmp:
        clean = os.path.join(tmp, "clean")
        copy_repo(root, clean)
        found = lint(clean)
        print("self-test: unchanged copy " + ("passes" if not found else f"reports {found}"))
        failures += bool(found)
        for rule, plant in PLANTS.items():
            copy = os.path.join(tmp, rule)
            copy_repo(root, copy)
            plant(copy)
            # -f: a planted artifact counts even where .gitignore would keep it out.
            subprocess.run(["git", "-C", copy, "add", "-A", "-f"], check=True)
            reported = sorted({f[0] for f in lint(copy)})
            ok = reported == [rule]
            print(f"self-test: {rule} planted, reported {reported or 'nothing'}" + ("" if ok else "  FAILED"))
            failures += not ok
    return failures


def main():
    args = sys.argv[1:]
    root = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
    if "--root" in args:
        root = os.path.abspath(args[args.index("--root") + 1])
    if "--self-test" in args:
        sys.exit(1 if self_test(root) else 0)
    warnings = []
    findings = lint(root, warnings)
    for rule, path, message in findings:
        print(f"{rule}: {path}: {message}")
    for warning in warnings:
        print(f"warning: {warning}")
    print(f"template-lint: {len(findings)} findings, {len(warnings)} warnings")
    sys.exit(1 if findings else 0)


if __name__ == "__main__":
    main()
