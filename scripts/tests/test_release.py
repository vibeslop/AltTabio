"""Exercise release authorization against real Git histories and installer failures."""

import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
PIN = (ROOT / "scripts/mac/release-certificate.sha1").read_text().strip()
spec = importlib.util.spec_from_file_location("release_policy", ROOT / "scripts/validate-release.py")
policy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(policy)


class ReleaseAuthorization(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.previous = Path.cwd()
        os.chdir(self.temporary.name)
        self.addCleanup(self.temporary.cleanup)
        self.addCleanup(os.chdir, self.previous)
        policy.git("init", "--quiet")
        policy.git("config", "commit.gpgsign", "false")
        policy.git("config", "tag.gpgsign", "false")
        policy.git("config", "user.name", "Release test")
        policy.git("config", "user.email", "test@example.invalid")
        Path("Cargo.toml").write_text('[package]\nversion = "1.2.3"\n')
        pin = Path("scripts/mac/release-certificate.sha1")
        pin.parent.mkdir(parents=True)
        pin.write_text(PIN + "\n")
        policy.git("add", ".")
        policy.git("commit", "--quiet", "-m", "Trusted release")
        self.sha = policy.git("rev-parse", "HEAD")
        policy.git("update-ref", "refs/remotes/origin/main", self.sha)
        policy.git("tag", "v1.2.3")
        self.metadata = dict(tag="v1.2.3", version="1.2.3", sha=self.sha,
                             windows_digest="a" * 64, macos_digest="b" * 64)

    def test_accepts_source_on_main(self):
        self.assertEqual(policy.validate(self.metadata, self.sha), self.metadata)

    def test_rejects_tag_code_outside_main(self):
        Path("unreviewed.txt").write_text("tag-only code")
        policy.git("add", ".")
        policy.git("commit", "--quiet", "-m", "Unreviewed")
        sha = policy.git("rev-parse", "HEAD")
        policy.git("tag", "--force", "v1.2.3")
        self.metadata["sha"] = sha
        with self.assertRaises(subprocess.CalledProcessError):
            policy.validate(self.metadata, sha)

    def test_rejects_moved_tag(self):
        policy.git("commit", "--allow-empty", "--quiet", "-m", "Move")
        policy.git("tag", "--force", "v1.2.3")
        with self.assertRaisesRegex(ValueError, "Tag moved"):
            policy.validate(self.metadata, self.sha)

    def test_rejects_a_different_build_run(self):
        with self.assertRaisesRegex(ValueError, "completed build run"):
            policy.validate(self.metadata, "0" * 40)

    def test_rejects_output_injection_and_extra_fields(self):
        for field, value in [("tag", "v1.2.3\nevil=1"), ("windows_digest", "a" * 64 + "\nevil=1"),
                             ("sha", "--help"), ("extra", "anything")]:
            with self.subTest(field=field), self.assertRaises(ValueError):
                policy.validate({**self.metadata, field: value}, self.sha)

    def test_rejects_replacing_the_publisher_identity(self):
        pin = Path("scripts/mac/release-certificate.sha1")
        pin.write_text("0" * 40)
        with self.assertRaisesRegex(ValueError, "different publisher identity"):
            policy.validate(self.metadata, self.sha)


class ReleaseConfiguration(unittest.TestCase):
    def test_actions_are_pinned(self):
        for path in (ROOT / ".github/workflows").glob("*.yml"):
            for action in re.findall(r"uses:\s+(\S+)", path.read_text()):
                self.assertRegex(action, r"^[\w.-]+/[\w./-]+@[0-9a-f]{40}$", str(path))

    def test_signer_and_installer_keep_the_existing_identity(self):
        workflow = (ROOT / ".github/workflows/release.yml").read_text()
        installer = (ROOT / "docs/install.sh").read_text()
        self.assertIn("RELEASE_CERTIFICATE: " + PIN, workflow)
        self.assertIn("release_certificate=" + PIN, installer)
        self.assertIn(PIN, (ROOT / "docs/mac/index.html").read_text())
        self.assertIn(PIN, (ROOT / "README.md").read_text())
        signing_job = workflow.split("  macos:\n", 1)[1]
        self.assertNotIn("actions/checkout", signing_job)
        self.assertNotIn("scripts/", signing_job)
        self.assertNotIn("--clobber", workflow)
        self.assertLess(signing_job.index("shasum -a 256"), signing_job.index("security import"))
        self.assertLess(signing_job.index("security delete-keychain"),
                        signing_job.index("actions/attest-build-provenance"))


@unittest.skipIf(os.name == "nt", "Exercises the macOS installer through a POSIX shell")
class InstallerAuthentication(unittest.TestCase):
    def reject_bundle(self, certificate, identifier="com.vibeslop.AltTabio", executable="AltTabio"):
        with tempfile.TemporaryDirectory() as temporary:
            base = Path(temporary)
            bin_dir = base / "bin"
            bin_dir.mkdir()
            home = base / "home"
            installed = home / "Applications/AltTabio.app"
            installed.mkdir(parents=True)
            sentinel = installed / "existing"
            sentinel.write_text("original installation")
            log = base / "commands.log"
            dispatcher = bin_dir / "mock"
            # Every command capable of changing the installation or stopping/starting an app
            # is intercepted. Only scratch extraction and the installer's trap use real files.
            dispatcher.write_text("""#!/usr/bin/env python3
import os, pathlib, sys
name = pathlib.Path(sys.argv[0]).name
args = sys.argv[1:]
with open(os.environ['MOCK_LOG'], 'a') as log: log.write(name + '\\n')
if name == 'sw_vers': print('26.0')
elif name == 'curl':
    if '-o' in args: pathlib.Path(args[args.index('-o') + 1]).write_text('mock zip')
    else: print('[]')
elif name == 'osascript': print('v1.2.3 https://example.invalid/release.zip')
elif name == 'ditto':
    app = pathlib.Path(args[-1]) / 'AltTabio.app/Contents/MacOS'
    app.mkdir(parents=True)
    (app / 'AltTabio').write_text('mock executable')
elif name == 'codesign':
    for arg in args:
        if arg.startswith('--extract-certificates='):
            pathlib.Path(arg.split('=', 1)[1] + '0').write_text('certificate')
elif name == 'shasum': print(os.environ['MOCK_CERTIFICATE'] + '  -')
elif name == 'plutil':
    print(os.environ['MOCK_IDENTIFIER'] if args[1] == 'CFBundleIdentifier' else os.environ['MOCK_EXECUTABLE'])
else: sys.exit('Forbidden installation mutation: ' + name)
""")
            dispatcher.chmod(0o755)
            for name in ("sw_vers", "curl", "osascript", "ditto", "codesign", "shasum", "plutil",
                         "pkill", "pgrep", "open", "mv", "sleep"):
                (bin_dir / name).symlink_to(dispatcher)
            result = subprocess.run(["sh", str(ROOT / "docs/install.sh")], capture_output=True,
                                    text=True, env={**os.environ, "PATH": str(bin_dir) + os.pathsep + os.environ["PATH"],
                                                   "HOME": str(home), "TMPDIR": str(base), "MOCK_LOG": str(log),
                                                   "MOCK_CERTIFICATE": certificate, "MOCK_IDENTIFIER": identifier,
                                                   "MOCK_EXECUTABLE": executable})
            self.assertNotEqual(result.returncode, 0, result.stdout)
            self.assertEqual(sentinel.read_text(), "original installation")
            commands = log.read_text().splitlines()
            self.assertTrue({"pkill", "pgrep", "open", "mv", "sleep"}.isdisjoint(commands))
            return result

    def test_rejects_wrong_signer_before_mutating_installation(self):
        self.assertIn("pinned release certificate", self.reject_bundle("0" * 40).stderr)

    def test_rejects_ad_hoc_signing_before_mutating_installation(self):
        self.assertIn("pinned release certificate", self.reject_bundle("").stderr)

    def test_rejects_wrong_identifier_before_mutating_installation(self):
        self.assertIn("wrong identifier", self.reject_bundle(PIN, identifier="com.attacker.App").stderr)

    def test_rejects_wrong_executable_before_mutating_installation(self):
        self.assertIn("wrong executable", self.reject_bundle(PIN, executable="Payload").stderr)


if __name__ == "__main__":
    unittest.main()
