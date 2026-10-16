#!/usr/bin/env python3
"""Offline release checks for pinned dependency notices."""
import hashlib
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("dependency_notices", Path(__file__).with_name("bundle-dependency-notices.py"))
notices = importlib.util.module_from_spec(spec)
spec.loader.exec_module(notices)


class DependencyNotices(unittest.TestCase):
    def fixture(self, root):
        source = root / "assets/dependency-notices"
        source.mkdir(parents=True)
        text = b"License and upstream notice\r\n"
        (source / "LICENSE").write_bytes(text)
        manifest = {"format": 1, "packages": [{"name": "fixture", "version": "1.0.0", "crate_sha256": "1234", "files": [{"path": "LICENSE", "sha256": hashlib.sha256(text).hexdigest()}]}]}
        (source / "manifest.json").write_text(json.dumps(manifest))
        (root / "Cargo.lock").write_text('[[package]]\nname = "fixture"\nversion = "1.0.0"\nchecksum = "1234"\n')
        return source, text

    def test_bundles_exact_text_and_manifest_without_network(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, text = self.fixture(root)
            output = root / "bundle"
            self.assertEqual(notices.bundle(root, output), (1, 1))
            self.assertEqual((output / "LICENSE").read_bytes(), text)
            self.assertEqual((output / "manifest.json").read_bytes(), (source / "manifest.json").read_bytes())

    def test_changed_missing_notice_or_dependency_refuses_before_output(self):
        for kind in ["changed", "missing", "version", "checksum"]:
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                source, _ = self.fixture(root)
                if kind == "changed": (source / "LICENSE").write_text("different")
                elif kind == "missing": (source / "LICENSE").unlink()
                elif kind == "version": (root / "Cargo.lock").write_text((root / "Cargo.lock").read_text().replace("1.0.0", "2.0.0"))
                else: (root / "Cargo.lock").write_text((root / "Cargo.lock").read_text().replace("1234", "abcd"))
                with self.assertRaises(ValueError): notices.bundle(root, root / "bundle")
                self.assertFalse((root / "bundle").exists())


if __name__ == "__main__":
    unittest.main()
