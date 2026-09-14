import json
from pathlib import Path
import tempfile
import tomllib
import unittest

import template


def lock_fixture(prefix="demo", collide=False):
    blocks=[]
    for member in sorted(template.MEMBERS):
        dependencies=[]
        if member=="app":
            dependencies=[f"{prefix}-core 0.1.0", f"{prefix}-core 9.0.0"] if collide else [f"{prefix}-core", "tracing-core"]
        block=f'[[package]]\nname = "{prefix}-{member}"\nversion = "0.1.0"\n'
        if dependencies:
            block+='dependencies = [\n'+''.join(' '+json.dumps(d)+',\n' for d in dependencies)+']\n'
        blocks.append(block)
    external=f"{prefix}-core" if collide else "tracing-core"
    blocks.append(f'[[package]]\nname = "{external}"\nversion = "9.0.0"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\nchecksum = "original-checksum"\n')
    return 'version = 4\n\n'+'\n'.join(blocks)


class IdentityTests(unittest.TestCase):
    def test_gate_failures_are_explicit_exceptions_not_optimizable_assertions(self):
        with self.assertRaisesRegex(RuntimeError,"must fail"):
            template.require(False,"must fail")

    def test_supported_identities(self):
        template.valid_identity("a-different-project", "a"*32)
        for name,prefix in [("../other","ok"),("UPPER","ok"),("ok","bad_prefix"),("ok","a"*33)]:
            with self.assertRaises(ValueError): template.valid_identity(name,prefix)

    def test_only_local_package_identities_are_rewritten(self):
        result=tomllib.loads(template.normalize_lock(lock_fixture(),"demo"))["package"]
        self.assertEqual({p["name"] for p in result if "source" not in p},
                         {"{{crate_prefix}}-"+name for name in template.MEMBERS})
        registry=next(p for p in result if "source" in p)
        self.assertEqual(registry["name"],"tracing-core")
        self.assertEqual(registry["checksum"],"original-checksum")

    def test_registry_collision_keeps_source_and_version_disambiguation(self):
        result=tomllib.loads(template.normalize_lock(lock_fixture("sqlx",True),"sqlx"))["package"]
        registry=next(p for p in result if "source" in p)
        self.assertEqual(registry["name"],"sqlx-core")
        app=next(p for p in result if p["name"]=="{{crate_prefix}}-app")
        self.assertEqual(app["dependencies"][0],"{{crate_prefix}}-core 0.1.0")
        self.assertEqual(app["dependencies"][1],"sqlx-core 9.0.0 (registry+https://github.com/rust-lang/crates.io-index)")

    def test_unexpected_local_packages_are_rejected(self):
        extra='\n[[package]]\nname="unrelated"\nversion="1.0.0"\n'
        with self.assertRaises(ValueError): template.normalize_lock(lock_fixture()+extra,"demo")

    def test_ambiguous_unqualified_references_are_rejected(self):
        value=lock_fixture("sqlx",True).replace('"sqlx-core 0.1.0"','"sqlx-core"')
        with self.assertRaises(ValueError): template.normalize_lock(value,"sqlx")


class DirectorySafetyTests(unittest.TestCase):
    def test_workspace_generation_is_rejected(self):
        for path in (template.ROOT, template.ROOT/"generated", Path.home(), Path('/')):
            with self.assertRaises(ValueError): template.workspace_base(str(path))

    def test_unmanaged_clean_does_not_delete_files(self):
        with tempfile.TemporaryDirectory() as temp:
            base=Path(temp).resolve(); project=base/"m1-not-owned"/"project"
            project.mkdir(parents=True); sentinel=project/"keep"; sentinel.write_text("user data")
            with self.assertRaises(OSError): template.clean(project,base)
            self.assertEqual(sentinel.read_text(),"user data")

    def test_managed_clean_only_removes_its_own_parent(self):
        with tempfile.TemporaryDirectory() as temp:
            base=Path(temp).resolve(); parent=base/"m1-owned"; project=parent/"project"
            project.mkdir(parents=True); sibling=base/"keep"; sibling.write_text("keep")
            (parent/template.MARKER).write_text(json.dumps({"template":str(template.ROOT),"project":"project"}))
            template.clean(project,base)
            self.assertFalse(parent.exists()); self.assertEqual(sibling.read_text(),"keep")

    def test_mismatched_owner_is_not_deleted(self):
        with tempfile.TemporaryDirectory() as temp:
            base=Path(temp).resolve(); parent=base/"m1-other"; project=parent/"project"
            project.mkdir(parents=True)
            (parent/template.MARKER).write_text(json.dumps({"template":"/unrelated","project":"project"}))
            with self.assertRaises(ValueError): template.clean(project,base)
            self.assertTrue(project.exists())

    def test_symlink_clean_is_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            base=Path(temp).resolve(); victim=base/"victim"; victim.mkdir()
            link=base/"m1-link"; link.symlink_to(victim,target_is_directory=True)
            with self.assertRaises(ValueError): template.clean(link/"project",base)
            self.assertTrue(victim.exists())




class SyncSafetyTests(unittest.TestCase):
    def test_all_sources_are_validated_before_any_writeback(self):
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "template"
            project = Path(temporary) / "generated"
            (root / "app").mkdir(parents=True)
            (project / "app").mkdir(parents=True)
            a, b = root / "app/a.rs", root / "app/b.rs"
            a.write_text("original a"); b.write_text("original b")
            snapshot = {a: a.read_bytes(), b: b.read_bytes()}
            (project / "app/a.rs").write_text("formatted a")
            (project / "app/b.rs").write_text("formatted b")
            b.write_text("concurrent user edit")
            with patch.object(template, "ROOT", root), patch.object(template, "rust_sources", return_value=[a, b]):
                with self.assertRaisesRegex(RuntimeError, "source changed"):
                    template.sync_rust(project, snapshot)
            self.assertEqual(a.read_text(), "original a")
            self.assertEqual(b.read_text(), "concurrent user edit")

    def test_fmt_copies_bytes_without_reverse_identity_or_brace_substitution(self):
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "template"; root.mkdir()
            project = Path(temporary) / "generated"; project.mkdir()
            source = root / "example.rs"; source.write_text("before formatting")
            text = 'service_core; "${NAME:-fallback}"; "{{literal}}"; "${{ github.ref }}";'
            (project / source.name).write_text(text)
            with patch.object(template, "ROOT", root), patch.object(template, "rust_sources", return_value=[source]):
                template.sync_rust(project, {source: source.read_bytes()})
            self.assertEqual(source.read_text(), text)

    def test_changed_source_set_and_formatted_symlinks_are_rejected(self):
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "template"; root.mkdir()
            project = Path(temporary) / "generated"; project.mkdir()
            source = root / "example.rs"; source.write_text("keep")
            (project / source.name).symlink_to(source)
            with patch.object(template, "ROOT", root), patch.object(template, "rust_sources", return_value=[source]):
                with self.assertRaisesRegex(RuntimeError, "source set changed"):
                    template.sync_rust(project, {})
                with self.assertRaisesRegex(RuntimeError, "invalid formatted source"):
                    template.sync_rust(project, {source: b"keep"})
            self.assertEqual(source.read_text(), "keep")

    def test_source_symlinks_cannot_escape_the_template(self):
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "template"; (root / "app").mkdir(parents=True)
            outside = Path(temporary) / "outside.rs"; outside.write_text("keep")
            (root / "app/escape.rs").symlink_to(outside)
            with patch.object(template, "ROOT", root):
                with self.assertRaisesRegex(RuntimeError, "regular workspace file"):
                    template.rust_sources()
            self.assertEqual(outside.read_text(), "keep")


class ProductizationTests(unittest.TestCase):
    def test_failed_captured_command_preserves_its_diagnostics(self):
        import contextlib
        import io
        import sys
        output = io.StringIO()
        with contextlib.redirect_stdout(output), self.assertRaisesRegex(RuntimeError, "command failed"):
            template.run([sys.executable, "-c", "print('failure-details'); raise SystemExit(1)"],
                         template.ROOT, capture=True)
        self.assertIn("\nfailure-details\n", output.getvalue())

    def test_matrix_reaches_name_limits_and_varies_identities(self):
        for _, name, prefix in template.NAME_MATRIX:
            template.valid_identity(name, prefix)
        self.assertEqual(max(len(n) for _, n, _ in template.NAME_MATRIX), 64)
        self.assertEqual(max(len(p) for _, _, p in template.NAME_MATRIX), 32)
        for name in ("", "-a", "a-", "a--b", "bad_name", "é", "a" * 65):
            with self.assertRaises(ValueError):
                template.valid_identity(name, "ok")

    def test_marker_symlink_cannot_authorize_deletion(self):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary).resolve(); parent = base / "m1-probe"
            project = parent / "project"; project.mkdir(parents=True)
            marker = base / "external-marker"
            marker.write_text(json.dumps({"template": str(template.ROOT), "project": "project"}))
            (parent / template.MARKER).symlink_to(marker)
            with self.assertRaisesRegex(ValueError, "marker"):
                template.clean(project, base)
            self.assertTrue(project.is_dir())

    def test_command_watchdog_is_effective(self):
        import sys
        with self.assertRaisesRegex(RuntimeError, "exceeded"):
            template.run([sys.executable, "-c", "import time; time.sleep(60)"], template.ROOT, timeout=0.1)


if __name__ == "__main__":
    unittest.main()
