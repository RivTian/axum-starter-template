#!/usr/bin/env python3
"""Template maintenance: isolated generation, structural gates and safe lock identity mapping.

Rust source is copied unchanged. No reverse replacement touches source text.
Only directories carrying our ownership marker can be removed by `clean`.
"""
from __future__ import annotations

import argparse
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager, nullcontext
import hashlib
import json
import os
import platform
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import tempfile

if sys.version_info < (3, 11):
    raise SystemExit("Python 3.11+ is required; select it with make PYTHON=/path/to/python3")
import tomllib

ROOT = Path(__file__).resolve().parents[1]
MEMBERS = {"app", "api", "core", "storage", "worker"}
EDGES = {"app": {"api", "worker", "storage", "core"}, "api": {"core", "storage"},
         "worker": {"core"}, "storage": set(), "core": set()}
MARKER = ".axum-template-owned.json"
CARGO = os.environ.get("CARGO", "cargo")
PINNED_GENERATOR = "0.24.0"
DEFAULT_GEN_ROOT = Path.home() / ".cache/axum-starter-template/workspaces"
MANAGED_PREFIX = "gen-"
# Old directories remain explicitly cleanable with their original --gen-root.
# This is a compatibility identifier, never a name used for new output.
LEGACY_MANAGED_PREFIX = "m1-"
NAME = re.compile(r"[a-z][a-z0-9]*(?:-[a-z0-9]+)*\Z")
HISTORY_MARKER = re.compile(r"(?<![a-z0-9])m\d+(?![a-z0-9])", re.IGNORECASE)
UNSHIPPED_DESIGN_REFERENCE = re.compile(
    r"\b(?:docs/)?(?:architecture|acceptance|m\d+-verification)\.md\b|§\s*\d", re.IGNORECASE
)
NAME_MATRIX = (
    ("short", "a", "x"),
    ("long", "long-" + "n" * 59, "p" * 32),
    ("hyphen-and-different-prefix", "billing-edge-demo", "svc"),
    ("registry-collision", "registry-name-collision", "sqlx"),
)


def require(condition: bool, message: object) -> None:
    # Gate checks must remain active under PYTHONOPTIMIZE, unlike bare assert.
    if not condition:
        raise RuntimeError(str(message))


def rust_sources() -> list[Path]:
    result = []
    for member in sorted(MEMBERS):
        for path in sorted((ROOT / member).rglob("*.rs")):
            if "target" in path.relative_to(ROOT / member).parts:
                continue
            require(not path.is_symlink() and ROOT in path.resolve().parents,
                    f"source must be a regular workspace file: {path}")
            result.append(path)
    return result


def export_hygiene(project: Path) -> int:
    """Audit the shipped scaffold, not the repository's historical evidence.

    Rendered files are checked at their authored source: a user may legitimately
    choose a project identity such as m1-service. All other files must be exact
    copies, so a hook cannot inject history after the source-side inspection.
    """
    require(not (project / "app/examples").exists(), "release probes belong in tests/fixtures, not app/examples")
    checked = 0
    for target in sorted(project.rglob("*")):
        relative = target.relative_to(project)
        if "target" in relative.parts or ".git" in relative.parts or not target.is_file():
            continue
        require(not target.is_symlink(), f"exported file must not be a symlink: {relative}")
        if relative == Path("Makefile"):
            source = ROOT / "Makefile.project"
        elif relative == Path("README.md"):
            source = ROOT / "README.project.md"
        else:
            source = ROOT / relative
        require(source.is_file() and not source.is_symlink(), f"exported file lacks an authored source: {relative}")
        require(not HISTORY_MARKER.search(str(relative)), f"milestone-derived export path: {relative}")
        # A dependency lock is resolution metadata, not authored commentary.
        if relative == Path("Cargo.lock"):
            continue
        for number, line in enumerate(source.read_text().splitlines(), 1):
            require(not HISTORY_MARKER.search(line), f"milestone history in export source: {source}:{number}")
            require(not UNSHIPPED_DESIGN_REFERENCE.search(line), f"unshipped design reference: {source}:{number}")
        rendered = relative.name == "Cargo.toml" or relative == Path("README.md")
        if not rendered:
            require(target.read_bytes() == source.read_bytes(), f"exported copy differs from source: {relative}")
        checked += 1
    return checked


def run(command: list[str], cwd: Path, env: dict[str, str] | None = None,
        capture: bool = False, timeout: int = 1800) -> str:
    print("+ " + " ".join(command), flush=True)
    child = subprocess.Popen(command, cwd=cwd, env=env, text=True,
                             stdout=subprocess.PIPE if capture else None,
                             stderr=None, start_new_session=os.name == "posix")
    try:
        stdout, _ = child.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        if os.name == "posix":
            os.killpg(child.pid, signal.SIGKILL)
        else:
            child.kill()
        stdout, _ = child.communicate()
        if stdout:
            print(stdout, end="", flush=True)
        raise RuntimeError(f"command exceeded {timeout}s: {command}") from None
    if child.returncode:
        if stdout:
            print(stdout, end="", flush=True)
        raise RuntimeError(f"command failed ({child.returncode}): {command}")
    return stdout or ""


def valid_identity(name: str, prefix: str) -> None:
    if not NAME.fullmatch(name) or len(name) > 64:
        raise ValueError("project name must be lowercase kebab-case, at most 64 characters")
    if not NAME.fullmatch(prefix) or len(prefix) > 32:
        raise ValueError("crate prefix must be lowercase kebab-case, at most 32 characters")


def workspace_base(value: str) -> Path:
    base = Path(value).expanduser().resolve()
    if base == ROOT or ROOT in base.parents or base in (Path('/'), Path.home()):
        raise ValueError("generation root must be a dedicated directory outside this repository")
    base.mkdir(parents=True, exist_ok=True)
    return base


def generate(base: Path, name: str, prefix: str) -> Path:
    valid_identity(name, prefix)
    version = run([CARGO, "generate", "--version"], ROOT, capture=True, timeout=30)
    if version.split()[-1] != PINNED_GENERATOR:
        raise RuntimeError(f"install cargo-generate {PINNED_GENERATOR}; found {version.strip()}")
    parent = Path(tempfile.mkdtemp(prefix=f"{MANAGED_PREFIX}{name}-", dir=base))
    marker = {"template": str(ROOT), "project": name, "prefix": prefix}
    (parent / MARKER).write_text(json.dumps(marker, indent=2) + "\n")
    project = parent / name
    # --path copies the current working tree, not remote main. No overwrite flag.
    run([CARGO, "generate", "--path", str(ROOT), "--destination", str(parent),
         "--name", name, "--define", f"crate_prefix={prefix}", "--vcs", "none", "--silent", "--no-workspace"], ROOT)
    if not project.is_dir():
        raise RuntimeError(f"generator did not create {project}")
    export_hygiene(project)
    print(f"GENERATED_PROJECT={project}", flush=True)
    return project


def environment(base: Path, project: Path, clean_room: bool = False) -> dict[str, str]:
    env = os.environ.copy()
    # All source directories are unique. Only Cargo's lock-protected artifact
    # cache is shared, never source trees or a mutable 'last generated' pointer.
    env["CARGO_TARGET_DIR"] = str(project / "target" if clean_room else base / "cargo-target")
    return env


def clean(project: Path, base: Path) -> None:
    if project.is_symlink() or project.parent.is_symlink():
        raise ValueError("refusing to clean through a symlink")
    project = project.resolve()
    parent = project.parent
    if parent.parent != base or not parent.name.startswith((MANAGED_PREFIX, LEGACY_MANAGED_PREFIX)):
        raise ValueError("refusing to clean an unmanaged directory")
    marker = parent / MARKER
    if not project.is_dir() or marker.is_symlink():
        raise ValueError("project must exist and ownership marker must not be a symlink")
    data = json.loads(marker.read_text())
    if data.get("template") != str(ROOT) or data.get("project") != project.name:
        raise ValueError("ownership marker does not match this template/project")
    shutil.rmtree(parent)
    print(f"REMOVED_MANAGED_PROJECT={project}")


@contextmanager
def writer_lock(base: Path):
    # Serialize our fmt/lock writers. Editors do not take this lock, so snapshot
    # checks below still reject edits observed before writeback.
    import fcntl
    fd = os.open(base / ".template-write.lock", os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, "w") as handle:
        fcntl.flock(handle, fcntl.LOCK_EX)
        yield


def sync_rust(project: Path, snapshot: dict[Path, bytes]) -> None:
    require(set(rust_sources()) == set(snapshot), "Rust source set changed during fmt")
    changes = []
    for source, original in snapshot.items():
        require(source.read_bytes() == original, f"source changed during fmt; refusing to overwrite: {source}")
        generated = project / source.relative_to(ROOT)
        require(not generated.is_symlink() and generated.is_file(), f"invalid formatted source: {generated}")
        formatted = generated.read_bytes()
        if formatted != original:
            changes.append((source, formatted))
    # Validate every file before touching any source. Never reverse-substitute
    # identities, dollar placeholders, Rust braces or GitHub expressions.
    for source, formatted in changes:
        source.write_bytes(formatted)


def normalize_lock(text: str, prefix: str) -> str:
    """Map workspace identities, not substrings; qualify dependency references.

    Fully qualified references also handle generated names colliding with registry
    packages (e.g. prefix=sqlx creates a local sqlx-core next to registry sqlx-core).
    No registry package's name is ever rewritten.
    """
    packages = tomllib.loads(text)["package"]
    local = {p["name"]: p for p in packages if "source" not in p}
    expected = {f"{prefix}-{member}" for member in MEMBERS}
    if set(local) != expected:
        raise ValueError(f"unexpected local packages in lockfile: {set(local)}")
    by_name: dict[str, list[dict]] = defaultdict(list)
    for package in packages:
        by_name[package["name"]].append(package)

    def identity(package: dict) -> str:
        if "source" not in package:
            return "{{crate_prefix}}-" + package["name"][len(prefix) + 1:]
        return package["name"]

    def reference(value: str) -> str:
        match = re.fullmatch(r"([^ ]+)(?: ([^ ]+))?(?: \((.+)\))?", value)
        if match is None:
            raise ValueError(f"unsupported Cargo.lock dependency reference: {value}")
        name, version, source = match.groups()
        candidates = [p for p in by_name[name]
                      if (version is None or p["version"] == version)
                      and (source is None or p.get("source") == source)]
        if len(candidates) != 1:
            raise ValueError(f"ambiguous dependency reference: {value}")
        package = candidates[0]
        result = f'{identity(package)} {package["version"]}'
        if "source" in package:
            result += f' ({package["source"]})'
        return result

    chunks = text.split("[[package]]")
    if len(chunks) - 1 != len(packages):
        raise ValueError("unsupported Cargo.lock structure")
    output = [chunks[0]]
    for chunk, package in zip(chunks[1:], packages, strict=True):
        lines = chunk.splitlines(keepends=True)
        in_dependencies = False
        for index, line in enumerate(lines):
            if line.startswith("name = "):
                lines[index] = f'name = "{identity(package)}"\n'
            elif line == "dependencies = [\n":
                in_dependencies = True
            elif in_dependencies and line.strip() == "]":
                in_dependencies = False
            elif in_dependencies:
                value = json.loads(line.strip().removesuffix(","))
                lines[index] = " " + json.dumps(reference(value)) + ",\n"
        output.append("[[package]]" + "".join(lines))
    result = "".join(output)
    tomllib.loads(result)
    return result


def structure(project: Path, name: str, prefix: str, env: dict[str, str]) -> dict:
    checked_exports = export_hygiene(project)
    meta = json.loads(run([CARGO, "metadata", "--locked", "--format-version", "1"], project, env, True))
    packages = {p["id"]: p for p in meta["packages"]}
    members = [packages[key] for key in meta["workspace_members"]]
    require({p["name"] for p in members} == {f"{prefix}-{member}" for member in MEMBERS}, "workspace members differ from the five-crate contract")
    require({p["id"] for p in packages.values() if p["source"] is None} == set(meta["workspace_members"]),
            "unexpected non-workspace path dependency")
    names = {p["name"]: p["name"][len(prefix) + 1:] for p in members}
    dependency_edges = []
    for package in members:
        source = names[package["name"]]
        normal = set()
        for dep in package["dependencies"]:
            if dep["name"] in names and dep.get("path"):
                destination = names[dep["name"]]
                require(destination in EDGES[source], (source, destination))
                require(dep["rename"] == f"service-{destination}", "incorrect internal dependency alias")
                require(Path(dep["path"]).resolve() == (project / destination).resolve(), "local dependency escaped the generated workspace")
                kind = dep["kind"] or "normal"
                dependency_edges.append(f"{source}:{kind}->{destination}")
                if kind == "normal":
                    normal.add(destination)
            if dep["name"] == "tracing-subscriber" and dep["kind"] != "dev":
                require(source == "app", "only app may depend on the subscriber in production")
            if dep["name"].startswith("sqlx") and not dep.get("path"):
                require(source == "storage", "SQLx dependency leaked outside storage")
        require(normal == EDGES[source], f"production dependency edges changed: {source}: {normal}")
    app = next(p for p in members if names[p["name"]] == "app")
    require(any(t["name"] == name.replace("-", "_") and "bin" in t["kind"] for t in app["targets"]), "incorrect application binary identity")
    require(sum("bin" in target["kind"] for target in app["targets"]) == 1, "only one service executable is expected")
    fixtures = [target for target in app["targets"] if "example" in target["kind"]]
    require(len(fixtures) == 1 and fixtures[0]["name"] == "release-panic-fixture"
            and Path(fixtures[0]["src_path"]) == project / "app/tests/fixtures/release_panic.rs"
            and not fixtures[0]["test"], "release fixture must use a standalone target under tests/fixtures")
    active = {node["id"]: node for node in meta["resolve"]["nodes"]}
    # metadata/Cargo.lock can include optional dependencies that are not compiled.
    # Check the selected workspace build/test graph, not that resolution superset.
    tree = run([CARGO, "tree", "--workspace", "--locked", "--prefix", "none",
                "--edges", "normal,build,dev"], project, env, True)
    for line in tree.splitlines():
        require(not re.match(r"^(sqlx-postgres|sqlx-mysql) v", line), line)
    sqlx = next(p for p in packages.values() if p["name"] == "sqlx" and p["source"])
    features = set(active[sqlx["id"]]["features"])
    require(not features & {"any", "postgres", "mysql", "sqlite-load-extension", "sqlite-deserialize", "sqlite-unlock-notify"}, features)
    require("sqlite-bundled" in features, "SQLite bundled feature missing")
    require((project / "Makefile").read_bytes() == (ROOT / "Makefile.project").read_bytes(), "incorrect generated Makefile")
    # No project CI is shipped. .github is listed below because cargo-generate.toml's
    # ignore list is now the only thing keeping the template's own template-check
    # workflow out of the result, and nothing else would notice if it stopped working.
    for absent in ("hooks", "scripts", "docs", "Makefile.project", "README.project.md", ".github", "cargo-generate.toml", ".genignore",
                   "provider", "runtime", "push", "testkit", "web-ui", "Cross.toml", "Dockerfile", "docker-compose.yml", ".claude"):
        require(not (project / absent).exists(), f"template-only file leaked: {absent}")
    for source in rust_sources():
        generated = project / source.relative_to(ROOT)
        require(generated.read_bytes() == source.read_bytes(), f"Rust source was rendered: {source}")
    require(not list(project.rglob("__pycache__")), "local Python cache leaked into generation")
    unresolved = re.compile(r"\{\{\s*(?:crate_prefix|crate_name|env_prefix|project-name)\s*\}\}")
    for path in project.rglob("*"):
        if path.is_file() and "target" not in path.relative_to(project).parts:
            require(not unresolved.search(path.read_text()), f"unexpanded identity in {path}")
    # Only these files are Liquid input. GitHub expressions live in a byte-copy
    # workflow; Rust braces and shell ${NAME} are not reverse-replacement tokens.
    for path in [project / "Cargo.toml", project / "Cargo.lock", project / "README.md",
                 *(project / member / "Cargo.toml" for member in sorted(MEMBERS))]:
        require(not re.search(r"(?<!\$)\{\{|\{%", path.read_text()), f"unexpanded Liquid in rendered file: {path}")
    require((project / "config/service.toml").is_file(), "default configuration was not shipped")
    require(not list((project / "storage/migrations").glob("*.sql")), "business migrations were pre-shipped")
    attributes = (project / ".gitattributes").read_text()
    require("*.sql text eol=lf" in attributes, "migration LF rule is missing")
    for source in rust_sources():
        require(not re.search(r"\b(?:edge_core|edge_storage|prism_store|starriver|sansitech)\b", source.read_text()), f"legacy production identity: {source}")
    return {"workspace_packages": sorted(names), "dependency_edges": sorted(dependency_edges),
            "sqlx_features": sorted(features), "history_free_exports": checked_exports}



def migration_rebuild(project: Path, prefix: str, env: dict[str, str]) -> list[str]:
    """Mutate only this newly generated scratch project, never source migrations."""
    marker = json.loads((project.parent / MARKER).read_text())
    require(marker["template"] == str(ROOT) and marker["project"] == project.name,
            "migration rebuild probe requires a managed scratch project")
    manifest = project / "storage/Cargo.toml"
    probe = project / "storage/migrations/0001_embedded_migration_probe.sql"
    require(not probe.exists(), "refusing to overwrite a migration")

    def fresh() -> bool:
        messages = run([CARGO, "build", "--locked", "-p", f"{prefix}-storage",
                        "--message-format=json-render-diagnostics"], project, env, True)
        hits = [message for line in messages.splitlines()
                if (message := json.loads(line)).get("reason") == "compiler-artifact"
                and message.get("manifest_path") == str(manifest)
                and "lib" in message["target"]["kind"]]
        require(len(hits) == 1, "could not identify the storage library build artifact")
        return hits[0]["fresh"]

    def baseline() -> None:
        fresh()  # establish the package-only build/feature fingerprint
        require(fresh(), "unchanged storage should be fresh before a mutation")

    def executable() -> Path:
        name = tomllib.loads((project / "app/Cargo.toml").read_text())["bin"][0]["name"]
        messages = run([CARGO, "build", "--locked", "--bin", name,
                        "--message-format=json-render-diagnostics"], project, env, True)
        hits = [message["executable"] for line in messages.splitlines()
                if (message := json.loads(line)).get("reason") == "compiler-artifact"
                and message.get("executable") and message["target"]["name"] == name]
        require(len(hits) == 1, "could not identify the migration-probe executable")
        return Path(hits[0])

    def process(binary: Path, directory: Path, expected: str) -> None:
        run([sys.executable, str(ROOT / "scripts/probe_migrations.py"), "--project", str(project),
             "--binary", str(binary), "--directory", str(directory), "--expect", expected], project, env, timeout=30)

    results = []
    created = False
    with tempfile.TemporaryDirectory(prefix="migration-probe-") as temporary:
        data = Path(temporary)
        try:
            baseline()
            with probe.open("x") as handle:
                created = True
                handle.write("CREATE TABLE embedded_migration_probe (id INTEGER); INSERT INTO embedded_migration_probe VALUES (1);\n")
            require(not fresh(), "adding a migration failed to trigger recompilation")
            binary = executable()
            process(binary, data / "applied", "applied")
            process(binary, data / "applied", "applied")
            results.extend(["add-embedded", "restart-does-not-reapply"])
            baseline()
            probe.write_text("CREATE TABLE embedded_migration_probe (id INTEGER); INVALID_SQL;\n")
            require(not fresh(), "modifying a migration failed to trigger recompilation")
            binary = executable()
            process(binary, data / "applied", "rejected")  # checksum mismatch
            process(binary, data / "invalid-sql", "rejected")
            results.extend(["modify-checksum-rejected", "invalid-sql-fail-fast"])
        finally:
            if created:
                try:
                    baseline()
                finally:
                    probe.unlink(missing_ok=True)
                require(not fresh(), "removing a migration failed to trigger recompilation")
                binary = executable()  # Restore an actual empty-set service artifact.
        process(binary, data / "applied", "rejected")  # missing applied version
        process(binary, data / "empty", "empty")
        results.extend(["remove-missing-version-rejected", "remove-embedded-empty"])
    require(not list((project / "storage/migrations").glob("*.sql")), "probe left a business migration behind")
    print("Migration rebuild: add / modify / remove were observed by actual service binaries")
    return results


def basic_check(project: Path, name: str, prefix: str, env: dict[str, str]) -> dict:
    details = structure(project, name, prefix, env)
    for arguments in (
        ["fmt", "--all", "--", "--check"],
        ["clippy", "--workspace", "--all-targets", "--locked", "--", "-D", "warnings"],
        ["test", "--workspace", "--locked"],
        ["build", "--workspace", "--locked"],
    ):
        run([CARGO, *arguments], project, env)
    output = run([sys.executable, "app/tests/check_service.py", "--identity-only"], project, env, True)
    print(output, end="")
    result = json.loads(output.splitlines()[-1])
    require(result["scope"] == "identity" and result["result"] == "passed", "identity process probe did not pass")
    details["gates"] = ["fmt", "clippy", "test", "build", "identity-process"]
    return details


def check(project: Path, name: str, prefix: str, env: dict[str, str]) -> None:
    details = structure(project, name, prefix, env)
    # Exercise the shipped entry points themselves, rather than a lookalike
    # sequence maintained solely in the template driver. Two separate ordered
    # invocations, exactly as the generated CI runs them: passing both goals to
    # one make would let them mutate the same workspace concurrently.
    make = os.environ.get("MAKE", "make")
    output = ""
    for goal in ("check", "check-process"):
        part = run([make, goal, f"CARGO={CARGO}", f"PYTHON={sys.executable}"], project, env, True)
        print(part, end="")
        output += part
    records = [json.loads(line) for line in output.splitlines() if line.startswith("{")]
    services = [r for r in records if r.get("suite") == "service"]
    require({(r["profile"], r["topology"]) for r in services} == {
        (p, t) for p in ("debug", "release") for t in ("main", "worker", "http", "shared", "split")
    } and len(services) == 10, "generated Makefile omitted service process combinations")
    require(all(r["result"] == "passed" for r in records), "a generated check did not pass")
    details["service_process_checks"] = services
    details["migration_rebuild"] = migration_rebuild(project, prefix, env)
    details.update({"project": str(project), "rustc": run(["rustc", "-V"], project, env, True).strip(),
                    "platform": platform.platform(), "python": platform.python_version(),
                    "generator": PINNED_GENERATOR, "clean_target": env["CARGO_TARGET_DIR"] == str(project / "target"),
                    "lock_sha256": hashlib.sha256((project / "Cargo.lock").read_bytes()).hexdigest(),
                    "template_lock_sha256": hashlib.sha256((ROOT / "Cargo.lock").read_bytes()).hexdigest(),
                    "result": "passed", "scope": "local", "remote_ci": "not_run"})
    report = project.parent / "verification.json"
    report.write_text(json.dumps(details, indent=2) + "\n")
    print(f"VERIFICATION_REPORT={report}")


def matrix(base: Path) -> None:
    results = []
    for label, name, prefix in NAME_MATRIX:
        project = generate(base, name, prefix)
        details = basic_check(project, name, prefix, environment(base, project))
        details.update({"case": label, "project": str(project), "name": name, "prefix": prefix})
        results.append(details)
    # Two actual generators read the same working tree concurrently, including
    # identical requested identities. Each must own a different source directory.
    with ThreadPoolExecutor(max_workers=2) as executor:
        projects = list(executor.map(lambda _: generate(base, "parallel-probe", "parx"), range(2)))
    require(projects[0] != projects[1], "parallel generators shared a source directory")
    for project in projects:
        structure(project, "parallel-probe", "parx", environment(base, project))
    # Project identities are user input, not authored migration history. Prove
    # that the export audit does not reserve otherwise valid names for itself.
    identity_project = generate(base, "m1-service", "m2")
    structure(identity_project, "m1-service", "m2", environment(base, identity_project))
    sentinel = projects[0] / "user-change.txt"
    sentinel.write_text("do not overwrite an earlier generated project\n")
    clean(projects[1], base)
    require(sentinel.read_text() == "do not overwrite an earlier generated project\n", "clean affected another project")
    # cargo-generate otherwise edits an enclosing Cargo workspace automatically.
    # GEN_ROOT may be outside this template but inside a different user project.
    with tempfile.TemporaryDirectory(prefix="parent-workspace-", dir=base) as temporary:
        outer = Path(temporary)
        manifest = outer / "Cargo.toml"
        original = '[workspace]\nresolver = "3"\nmembers = []\n'
        manifest.write_text(original)
        nested = outer / "generated"
        nested.mkdir()
        generate(nested, "nested-probe", "nestx")
        require(manifest.read_text() == original, "generator modified an enclosing workspace")
    # Test direct cargo-generate entry too: the Python wrapper cannot be the only
    # identity guard used by people generating from a git URL. Ordinary direct
    # generation normalizes names before pre-hooks; --force preserves the input
    # (it does NOT authorize overwrite) so this exercises our own rejection.
    for name, prefix in (("Bad_Name", "ok"), ("a" * 65, "ok"), ("ok", "a" * 33)):
        with tempfile.TemporaryDirectory(prefix="invalid-name-", dir=base) as target:
            try:
                run([CARGO, "generate", "--path", str(ROOT), "--destination", target,
                     "--name", name, "--define", f"crate_prefix={prefix}", "--vcs", "none", "--silent",
                     "--force", "--no-workspace"], ROOT, timeout=30)
            except RuntimeError as error:
                require("command failed" in str(error), "invalid generator input hung instead of rejecting")
            else:
                raise RuntimeError(f"direct generator accepted invalid identity: {name}/{prefix}")
    report = projects[0].parent / "matrix.json"
    report.write_text(json.dumps({"names": results, "parallel_generation": "passed", "user_identity_not_filtered": "passed",
                                 "parent_workspace_unchanged": "passed", "direct_invalid_names": "passed",
                                 "platform": platform.platform(), "result": "passed"}, indent=2) + "\n")
    print(f"MATRIX_REPORT={report}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["gen", "check", "verify", "matrix", "fmt", "lock", "clean"])
    parser.add_argument("--name", default="example-service")
    parser.add_argument("--prefix", default="example")
    parser.add_argument("--gen-root", default=str(DEFAULT_GEN_ROOT))
    parser.add_argument("--directory", type=Path)
    args = parser.parse_args()
    base = workspace_base(args.gen_root)
    if args.action == "clean":
        if args.directory is None:
            parser.error("clean requires --directory <managed generated project>")
        clean(args.directory, base)
        return
    if args.action == "matrix":
        matrix(base)
        return
    lock = writer_lock(base) if args.action in ("fmt", "lock") else nullcontext()
    with lock:
        before_format = {path: path.read_bytes() for path in rust_sources()} if args.action == "fmt" else {}
        before_lock = (ROOT / "Cargo.lock").read_bytes()
        project = generate(base, args.name, args.prefix)
        env = environment(base, project, args.action == "verify")
        if args.action == "gen":
            return
        if args.action == "lock":
            run([CARGO, "generate-lockfile"], project, env)
            normalized = normalize_lock((project / "Cargo.lock").read_text(), args.prefix)
            require((ROOT / "Cargo.lock").read_bytes() == before_lock, "Cargo.lock changed during generation; refusing overwrite")
            (ROOT / "Cargo.lock").write_text(normalized)
            print("Updated only workspace identities in template Cargo.lock")
        elif args.action == "fmt":
            run([CARGO, "fmt", "--all"], project, env)
            sync_rust(project, before_format)
        else:
            if args.action == "verify":
                require(not Path(env["CARGO_TARGET_DIR"]).exists(), "clean-room target was not empty")
            check(project, args.name, args.prefix, env)


if __name__ == "__main__":
    try:
        main()
    except (AssertionError, RuntimeError, ValueError, OSError, KeyError) as error:
        print(f"template gate failed: {error}", file=sys.stderr)
        sys.exit(1)
