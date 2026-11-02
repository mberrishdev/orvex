import contextlib
import importlib.util
import io
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("setup", Path(__file__).with_name("setup_dependencies.py"))
setup = importlib.util.module_from_spec(spec)
spec.loader.exec_module(setup)


class SetupTests(unittest.TestCase):
    def provision(self, files, mode="auto", fail=None):
        calls = []

        def run(*args, cwd=None):
            calls.append((args, str(cwd)))
            if fail and args[0] == fail:
                raise subprocess.CalledProcessError(1, args)

        with tempfile.TemporaryDirectory() as root:
            previous = Path.cwd()
            try:
                os.chdir(root)
                for name, content in files.items():
                    path = Path(name)
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_text(content)
                with patch.object(setup, "run", side_effect=run), patch.object(
                    setup.subprocess, "check_output", return_value="\0".join(files).encode()
                ), patch.object(setup.sys, "argv", ["setup", mode]), patch.object(
                    setup.Path, "home", return_value=Path(root)
                ), patch.dict(os.environ), contextlib.redirect_stdout(io.StringIO()):
                    setup.main()
            finally:
                os.chdir(previous)
        return calls

    def test_dotnet_installs_pinned_sdk_and_restores_nested_project(self):
        calls = self.provision({"global.json": '{"sdk":{"version":"8.0.405"}}',
                                "src/App.csproj": "<TargetFramework>net8.0</TargetFramework>"})
        self.assertTrue(any("--version" in args and "8.0.405" in args for args, _ in calls))
        self.assertIn((("dotnet", "restore", "src/App.csproj"), "None"), calls)

    def test_node_workspace_installs_once_with_pinned_manager(self):
        calls = self.provision({"package.json": '{"packageManager":"pnpm@10.32.1", "workspaces":["apps/*"]}',
                                "pnpm-lock.yaml": "", "apps/web/package.json": "{}"})
        self.assertIn((("sudo", "-n", "npm", "install", "--global", "pnpm@10.32.1"), "None"), calls)
        self.assertEqual([c for c in calls if c[0][0] == "pnpm"],
                         [(("pnpm", "install", "--frozen-lockfile"), ".")])

    def test_custom_setup_installs_runtime_without_duplicate_restore(self):
        calls = self.provision({"App.csproj": "<TargetFramework>net8.0</TargetFramework>"}, "custom")
        self.assertTrue(any("--channel" in args and "8.0" in args for args, _ in calls))
        self.assertFalse(any(args[0] == "dotnet" for args, _ in calls))

    def test_independent_nested_node_packages_are_installed(self):
        calls = self.provision({"package.json": "{}", "examples/demo/package.json": "{}"})
        self.assertEqual([cwd for args, cwd in calls if args[0] == "npm"], [".", "examples/demo"])

    def test_failed_install_propagates(self):
        with self.assertRaises(subprocess.CalledProcessError):
            self.provision({"package.json": "{}"}, fail="npm")

    def test_unknown_project_requires_explicit_setup(self):
        with self.assertRaisesRegex(RuntimeError, "No supported dependency manifest"):
            self.provision({"README.md": "project"})
        self.provision({"README.md": "project"}, "custom")

    def test_container_only_postgres_project_installs_client(self):
        calls = self.provision({"compose.yaml": "services:\n  db:\n    image: postgres:17"})
        self.assertIn((("sudo", "-n", "apt-get", "install", "-y", "postgresql-client"), "None"), calls)


if __name__ == "__main__":
    unittest.main()
