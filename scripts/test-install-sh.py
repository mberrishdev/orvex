"""Exercise release selection and fail-closed downloads without installing an app."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

INSTALLER = Path(__file__).resolve().parents[1] / "install.sh"


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.version = "0.1.0-beta.3"
        self.url = f"https://github.com/mberrishdev/orvex/releases/download/v{self.version}/Orvex.app.tar.gz"
        self.payload = b"download fixture, never executed"
        (self.root / "Orvex.app.tar.gz").write_bytes(self.payload)
        self.manifest = {"version": self.version, "platforms": {"darwin-aarch64": {"url": self.url}}}
        self.write_manifest()
        self.checksum = hashlib.sha256(self.payload).hexdigest()
        (self.root / "SHA256SUMS.txt").write_text(f"{self.checksum}  Orvex.app.tar.gz\n")
        self.tool("uname", 'import sys; print("Darwin" if sys.argv[1]=="-s" else "arm64")')
        self.tool("curl", '''import os, pathlib, shutil, sys
args=sys.argv[1:]; url=next(a for a in args if a.startswith('https://'))
root=pathlib.Path(os.environ['INSTALL_FIXTURE'])
with (root/'urls').open('a') as log: log.write(url+'\\n')
shutil.copyfile(root/url.rsplit('/',1)[1], args[args.index('--output')+1])
''')
        self.tool("plutil", '''import json, sys
value=json.load(open(sys.argv[-1]))
for key in sys.argv[sys.argv.index('-extract')+1].split('.'): value=value[key]
print(value)
''')
        self.env = {**os.environ, "PATH": f"{self.bin}:{os.environ['PATH']}", "INSTALL_FIXTURE": str(self.root)}

    def tool(self, name, source):
        p = self.bin / name
        p.write_text("#!/usr/bin/env python3\n" + source + "\n")
        p.chmod(0o755)

    def write_manifest(self):
        (self.root / "latest.json").write_text(json.dumps(self.manifest))

    def run_installer(self, *args):
        return subprocess.run(["bash", str(INSTALLER), "--download-only", "--output-dir", str(self.root / "downloads"), *args], env=self.env, text=True, capture_output=True)

    def test_beta_download_and_checksum(self):
        result = self.run_installer()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("download/updater-beta/latest.json", (self.root / "urls").read_text())
        self.assertEqual((self.root / "downloads/Orvex.app.tar.gz").read_bytes(), self.payload)

    def test_pinned_version_and_stable_feed(self):
        result = self.run_installer("--version", "v" + self.version)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(f"download/v{self.version}/latest.json", (self.root / "urls").read_text())
        (self.root / "downloads/Orvex.app.tar.gz").unlink()
        result = self.run_installer("--channel", "stable")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("releases/latest/download/latest.json", (self.root / "urls").read_text())

    def test_corruption_or_missing_checksum_never_installs(self):
        for checksum in ["0" * 64 + "  Orvex.app.tar.gz\n", "", f"{self.checksum}  Orvex.app.tar.gz\n" * 2]:
            (self.root / "SHA256SUMS.txt").write_text(checksum)
            result = self.run_installer()
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse((self.root / "downloads").exists())

    def test_manifest_mismatch_and_untrusted_url_fail(self):
        result = self.run_installer("--version", "1.0.0")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("does not match", result.stderr)
        self.manifest["platforms"]["darwin-aarch64"]["url"] = "https://example.com/Orvex.app.tar.gz"
        self.write_manifest()
        result = self.run_installer()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unexpected", result.stderr)

    def test_unsupported_arch_and_invalid_arguments_fail_before_download(self):
        self.tool("uname", 'import sys; print("Darwin" if sys.argv[1]=="-s" else "i386")')
        for args in [(), ("--version", "../unsafe"), ("--channel", "wrong"), ("--version",)]:
            self.assertNotEqual(self.run_installer(*args).returncode, 0)
        self.assertFalse((self.root / "urls").exists())


if __name__ == "__main__":
    unittest.main()
