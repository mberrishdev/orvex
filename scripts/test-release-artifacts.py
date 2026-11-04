import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("release", Path(__file__).with_name("release-artifacts.py"))
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.config = self.root / "apps/desktop/src-tauri/tauri.conf.json"
        self.config.parent.mkdir(parents=True)
        self.config.write_text(json.dumps({"version": "1.2.3-beta.1", "plugins": {"updater": {}}}))
        self.bundle = self.root / "bundle"
        for folder in ["macos/Orvex.app/Contents", "nsis", "dmg"]:
            (self.bundle / folder).mkdir(parents=True)
        for name in ["macos/Orvex.app.tar.gz", "nsis/Orvex_1.2.3-beta.1_x64-setup.exe", "dmg/Orvex.dmg", "dmg/bundle_dmg.sh", "macos/Orvex.app/Contents/Info.plist"]:
            (self.bundle / name).write_bytes(b"fixture")
        for name in ["macos/Orvex.app.tar.gz", "nsis/Orvex_1.2.3-beta.1_x64-setup.exe"]:
            Path(str(self.bundle / name) + ".sig").write_text("fixture-signature")
        self.artifacts = self.root / "artifacts"

    def collect(self, platform):
        release.collect(self.root, self.bundle, self.artifacts / platform, platform, "v1.2.3-beta.1")

    def test_channel_and_version_validation(self):
        release.prepare(self.root, "v1.2.3-beta.1")
        self.assertIn("download/updater-beta/latest.json", self.config.read_text())
        with self.assertRaises(ValueError):
            release.prepare(self.root, "v1.2.3-beta.2")
        config = json.loads(self.config.read_text())
        config["version"] = "1.2.3"
        self.config.write_text(json.dumps(config))
        release.prepare(self.root, "v1.2.3")
        self.assertIn("releases/latest/download/latest.json", self.config.read_text())

    def test_collect_and_merge_complete_release(self):
        self.collect("macos")
        self.collect("windows")
        output = self.root / "release"
        release.merge(self.artifacts, output)
        manifest = json.loads((output / "latest.json").read_text())
        self.assertEqual(set(manifest["platforms"]), {"darwin-aarch64", "windows-x86_64"})
        self.assertTrue(manifest["platforms"]["windows-x86_64"]["url"].endswith("-setup.exe"))
        self.assertFalse((output / "bundle_dmg.sh").exists())
        self.assertFalse((output / "Info.plist").exists())
        self.assertIn("  Orvex.app.tar.gz", (output / "SHA256SUMS.txt").read_text())

    def test_missing_signature_fails(self):
        (self.bundle / "macos/Orvex.app.tar.gz.sig").unlink()
        with self.assertRaises(FileNotFoundError):
            self.collect("macos")

    def test_missing_platform_fails(self):
        self.collect("windows")
        with self.assertRaises(ValueError):
            release.merge(self.artifacts, self.root / "release")

    def test_mixed_versions_fail(self):
        self.collect("macos")
        self.collect("windows")
        path = self.artifacts / "windows/latest-windows.json"
        manifest = json.loads(path.read_text())
        manifest["version"] = "9.0.0"
        path.write_text(json.dumps(manifest))
        with self.assertRaises(ValueError):
            release.merge(self.artifacts, self.root / "release")

    def test_missing_payload_fails(self):
        self.collect("macos")
        self.collect("windows")
        (self.artifacts / "macos/Orvex.app.tar.gz").unlink()
        with self.assertRaises(ValueError):
            release.merge(self.artifacts, self.root / "release")


if __name__ == "__main__":
    unittest.main()
