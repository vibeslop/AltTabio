#!/usr/bin/env python3
"""Exercise the installer's real bounded ZIP streams. Requires POSIX resource limits."""

import os
from pathlib import Path
import shlex
import stat
import struct
import subprocess
import sys
import tempfile
import unittest
import warnings
import zipfile


ROOT = Path(__file__).resolve().parents[2]
INSTALLER = ROOT / "scripts/mac/install.sh"
EXECUTABLE = "AltTabio.app/Contents/MacOS/AltTabio"
SCRIPT = INSTALLER.read_text().split("main() {", 1)[0]


class ArchiveTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="alttabio-archive-test-")
        self.addCleanup(self.temporary.cleanup)
        self.work = Path(self.temporary.name)
        self.archive = self.work / "update.zip"
        self.output = self.work / "extracted"
        self.environment = os.environ.copy()
        self.injected_tools = False

    def inject_tools(self, script):
        if self.injected_tools:
            # Fault-injection tools replace the production helper's fixed system PATH.
            return script.replace("PATH=/usr/bin:/bin:/usr/sbin:/sbin",
                                  "PATH=" + shlex.quote(self.environment["PATH"]))
        return script

    def write_zip(self, entries):
        with warnings.catch_warnings(), zipfile.ZipFile(
            self.archive, "w", compression=zipfile.ZIP_DEFLATED
        ) as archive:
            warnings.simplefilter("ignore", UserWarning)
            for name, content in entries:
                archive.writestr(name, content)

    def extract(self, script=SCRIPT):
        return subprocess.run(
            ["/bin/sh", "-c", self.inject_tools(script) + '\nextract_archive "$1" "$2"',
             "sh", str(self.archive), str(self.output)],
            env=self.environment, stdin=subprocess.DEVNULL, capture_output=True, timeout=20,
        )

    def mock(self, name, body):
        folder = self.work / "bin"
        folder.mkdir(exist_ok=True)
        command = folder / name
        command.write_text("#!/bin/sh\nset -eu\n" + body)
        command.chmod(0o755)
        self.environment["PATH"] = str(folder) + os.pathsep + os.environ["PATH"]
        self.injected_tools = True

    def assert_rejected(self, script=SCRIPT):
        result = self.extract(script)
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_regular_bundle_and_ignored_appledouble(self):
        self.write_zip([
            ("AltTabio.app/", b""), (EXECUTABLE, b"inert executable"),
            ("AltTabio.app/Contents/Info.plist", b"plist"),
            ("__MACOSX/AltTabio.app/._Contents", b"ignored"),
        ])
        result = self.extract()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.output / EXECUTABLE).read_bytes(), b"inert executable")
        self.assertTrue((self.output / EXECUTABLE).stat().st_mode & stat.S_IXUSR)
        self.assertFalse((self.output / "__MACOSX").exists())

    def test_bad_archive_and_crc_fail_closed(self):
        self.archive.write_bytes(b"not a ZIP")
        self.assert_rejected()
        self.output = self.work / "crc-output"
        self.write_zip([(EXECUTABLE, b"executable")])
        data = bytearray(self.archive.read_bytes())
        central = data.index(b"PK\x01\x02")
        struct.pack_into("<I", data, central + 16, 0)
        struct.pack_into("<I", data, 14, 0)
        self.archive.write_bytes(data)
        self.assert_rejected()

    def test_embedded_entry_point_only_extracts(self):
        self.write_zip([(EXECUTABLE, b"inert executable")])
        result = subprocess.run(
            ["/bin/sh", "-c", INSTALLER.read_text(), "sh", "--extract-update-archive",
             str(self.archive), str(self.output)],
            stdin=subprocess.DEVNULL, capture_output=True, timeout=20,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.output / EXECUTABLE).read_bytes(), b"inert executable")

    def test_encrypted_entry_fails_without_prompting(self):
        self.write_zip([(EXECUTABLE, b"not actually encrypted")])
        data = bytearray(self.archive.read_bytes())
        central = data.index(b"PK\x01\x02")
        struct.pack_into("<H", data, 6, 1)
        struct.pack_into("<H", data, central + 8, 1)
        self.archive.write_bytes(data)
        self.assert_rejected()

    def test_entry_and_listing_limits(self):
        self.write_zip([(f"AltTabio.app/{i}", b"") for i in range(257)])
        self.assert_rejected()
        self.assertFalse((self.output / "AltTabio.app").exists())
        (self.output / ".archive-members").unlink()
        self.output.rmdir()
        self.write_zip([(f"AltTabio.app/{i}-" + "x" * 200, b"") for i in range(200)])
        self.assert_rejected()
        self.assertLessEqual((self.output / ".archive-members").stat().st_size, 32768)

    def test_names_duplicates_and_non_bundle_payloads(self):
        for name in ["../escape", "/escape", "Other.app/file", "AltTabio.app/../escape",
                     "AltTabio.app/a/../../escape", "AltTabio.app/a*", "AltTabio.app/a\nb",
                     "AltTabio.app/" + "a/" * 17 + "file"]:
            with self.subTest(name=name):
                self.write_zip([(EXECUTABLE, b"exe"), (name, b"payload")])
                self.assert_rejected()
                self.assertFalse((self.work / "escape").exists())
                self.output = self.work / ("out-" + str(len(list(self.work.iterdir()))))
        self.write_zip([(EXECUTABLE, b"first"), (EXECUTABLE, b"second")])
        self.assert_rejected()

    def test_archive_symlinks_cannot_create_links(self):
        link = zipfile.ZipInfo("AltTabio.app/Contents/Resources/link")
        link.create_system = 3
        link.external_attr = (stat.S_IFLNK | 0o777) << 16
        self.write_zip([(EXECUTABLE, b"exe"), (link, b"../../../../escape")])
        result = self.extract()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse((self.output / link.filename).is_symlink())
        self.assertFalse((self.work / "escape").exists())

    def test_actual_decoder_output_is_bounded_even_with_forged_zip_sizes(self):
        for forged in [False, True]:
            with self.subTest(forged=forged):
                # Small compressed fixture with output above the real per-file ceiling.
                self.write_zip([(EXECUTABLE, bytes(65 * 1024 * 1024))])
                if forged:
                    data = bytearray(self.archive.read_bytes())
                    central = data.index(b"PK\x01\x02")
                    struct.pack_into("<I", data, central + 24, 1)
                    struct.pack_into("<I", data, 22, 1)
                    self.archive.write_bytes(data)
                self.assert_rejected()
                self.assertLessEqual((self.output / EXECUTABLE).stat().st_size, 64 * 1024 * 1024)
                self.output = self.work / "forged-output"

    def test_actual_aggregate_output_is_bounded(self):
        # Scale only the byte budgets to keep this aggregate test quick. Each file fits
        # even with a shell that uses 512-byte -f blocks, but their sum exceeds the cap.
        script = SCRIPT.replace("remaining=268435456", "remaining=196608").replace(
            "limit=67108864", "limit=65536"
        )
        self.write_zip([(EXECUTABLE, bytes(32768))] + [
            (f"AltTabio.app/Contents/Resources/{i}", bytes(32768)) for i in range(7)
        ])
        self.assert_rejected(script)
        written = sum(p.stat().st_size for p in self.output.rglob("*") if p.is_file())
        self.assertLessEqual(written, 196608 + 32768)  # Includes the bounded listing.

    def test_compressed_input_limit(self):
        with self.archive.open("wb") as archive:
            archive.truncate(104857601)
        self.assert_rejected()
        self.assertFalse(self.output.exists())

    def test_cpu_limit_stops_a_stuck_decoder(self):
        self.archive.write_bytes(b"mock")
        self.mock("unzip", 'if [ "$1" = -Z ]; then printf "%s\\n" "' + EXECUTABLE +
                  '"; else while :; do :; done; fi\n')
        self.assert_rejected(SCRIPT.replace("ulimit -t 10", "ulimit -t 1"))

    def test_extraction_deadline(self):
        self.write_zip([(EXECUTABLE, b"exe")])
        counter = self.work / "clock"
        self.mock("date", f'if [ -e "{counter}" ]; then echo 1000; '
                  f'else touch "{counter}"; echo 0; fi\n')
        self.assert_rejected()
        self.assertFalse((self.output / EXECUTABLE).exists())

    def test_download_has_a_hard_cap_even_if_curl_ignores_its_byte_option(self):
        self.mock("curl", 'while [ "$1" != --output ]; do shift; done\n'
                  'dd if=/dev/zero of="$2" bs=1024 count=100 2>/dev/null\n')
        download = self.work / "download.zip"
        result = subprocess.run(
            ["/bin/sh", "-c", self.inject_tools(SCRIPT.replace("ulimit -f 102400", "ulimit -f 4")) +
             '\ndownload_archive https://example.invalid "$1"', "sh", str(download)],
            env=self.environment, capture_output=True, timeout=10,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertLessEqual(download.stat().st_size, 4096)

    @unittest.skipUnless(sys.platform == "darwin", "requires macOS ditto, codesign and CI bundle")
    def test_native_bundle_round_trip(self):
        bundle = ROOT / "target/mac/AltTabio.app"
        self.assertTrue(bundle.is_dir(), "run scripts/mac/build-app.sh first")
        subprocess.run(["ditto", "-c", "-k", "--keepParent", str(bundle), str(self.archive)], check=True)
        result = self.extract()
        self.assertEqual(result.returncode, 0, result.stderr)
        extracted = self.output / "AltTabio.app"
        subprocess.run(["codesign", "--verify", "--deep", "--strict", str(extracted)], check=True)
        for original in bundle.rglob("*"):
            if original.is_file():
                self.assertEqual(original.read_bytes(), (extracted / original.relative_to(bundle)).read_bytes())


if __name__ == "__main__":
    unittest.main()
