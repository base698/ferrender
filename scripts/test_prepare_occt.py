#!/usr/bin/env python3
"""Offline failure tests for the release dependency verifier."""
import hashlib
import importlib.util
import io
from pathlib import Path
import tarfile
import tempfile
import sys
sys.dont_write_bytecode = True
import unittest

spec = importlib.util.spec_from_file_location("prepare_occt", Path(__file__).with_name("prepare-occt.py"))
prepare = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prepare)


class VerifyOcct(unittest.TestCase):
    def fixture(self, directory, extra=None):
        archive = directory / "fixture.tar.gz"
        with tarfile.open(archive, "w:gz") as out:
            for name in ["include/opencascade/Standard.hxx", "lib/libTKBRep.a", "share/doc/opencascade/LICENSE_LGPL_21.txt", "share/doc/opencascade/OCCT_LGPL_EXCEPTION.txt"]:
                member = tarfile.TarInfo("occt/" + name)
                member.size = 2
                out.addfile(member, io.BytesIO(b"ok"))
            if extra:
                out.addfile(extra)
        return archive, archive.stat().st_size, hashlib.sha256(archive.read_bytes()).hexdigest()

    def test_valid_archive_is_verified_and_reusable(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            archive, size, digest = self.fixture(directory)
            out = directory / "prepared"
            self.assertEqual(prepare.prepare_from_archive(archive, size, digest, "occt", out), out)
            self.assertEqual(prepare.prepare_from_archive(archive, size, digest, "occt", out), out)

    def test_digest_failure_never_extracts(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            archive, size, digest = self.fixture(directory)
            out = directory / "prepared"
            with self.assertRaisesRegex(ValueError, "SHA256"):
                prepare.prepare_from_archive(archive, size, "0" * 64, "occt", out)
            self.assertFalse(out.exists())
            with self.assertRaisesRegex(ValueError, "size"):
                prepare.prepare_from_archive(archive, size + 1, digest, "occt", out)
            self.assertFalse(out.exists())

    def test_traversal_and_symlinks_cannot_write_outside_extraction(self):
        for kind in ["traversal", "symlink"]:
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as directory:
                directory = Path(directory)
                bad = tarfile.TarInfo("occt/../../escape" if kind == "traversal" else "occt/link")
                if kind == "symlink":
                    bad.type = tarfile.SYMTYPE
                    bad.linkname = "../../escape"
                archive, size, digest = self.fixture(directory, bad)
                out = directory / "prepared"
                with self.assertRaises((ValueError, tarfile.TarError)):
                    prepare.prepare_from_archive(archive, size, digest, "occt", out)
                self.assertFalse(out.exists())
                self.assertFalse((directory / "escape").exists())

    def test_unverified_existing_directory_is_not_reused(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            archive, size, digest = self.fixture(directory)
            out = directory / "prepared"
            out.mkdir()
            with self.assertRaisesRegex(ValueError, "unverified"):
                prepare.prepare_from_archive(archive, size, digest, "occt", out)


if __name__ == "__main__":
    unittest.main()
