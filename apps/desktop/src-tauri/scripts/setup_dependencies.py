"""Provision dependencies inside a newly created, disposable Ubuntu guest only."""
import json
import os
from pathlib import Path
import re
import subprocess
import sys


def run(*args, cwd=None):
    print("+", " ".join(map(str, args)), flush=True)
    subprocess.run(args, cwd=cwd, check=True)


def apt(*packages):
    run("sudo", "-n", "apt-get", "install", "-y", *packages)


def main():
    # Only consider committed manifests; never walk generated/vendor trees.
    files = [Path(p) for p in subprocess.check_output(
        ["git", "ls-tree", "-r", "--name-only", "-z", "HEAD"]
    ).decode().split("\0") if p and not set(Path(p).parts) & {
        "node_modules", "vendor", "bin", "obj", ".venv"
    }]
    named = lambda name: [p for p in files if p.name == name]
    manifests = [p for p in files if p.name in {
        "package.json", "Cargo.toml", "go.mod", "requirements.txt",
        "pyproject.toml", "Gemfile"
    } or p.suffix in {".csproj", ".fsproj", ".vbproj"}]
    if not manifests and sys.argv[1] != "custom":
        if files and not any(p.name in {"Dockerfile", "compose.yaml", "compose.yml", "docker-compose.yaml", "docker-compose.yml"} for p in files):
            raise RuntimeError("No supported dependency manifest found. Declare setup.commands in .orvex/workspace.json.")
        if not files:
            return
    run("sudo", "-n", "apt-get", "update")
    apt("ca-certificates", "curl", "build-essential", "pkg-config")
    if any("postgres" in p.read_text().lower() or "npgsql" in p.read_text().lower()
           for p in files if p.suffix in {".csproj", ".json", ".yaml", ".yml"}
           and p.stat().st_size < 65536):
        apt("postgresql-client")
    if named("package.json"):
        apt("nodejs", "npm")
        run("sudo", "-n", "npm", "install", "--global", "n")
        node_hint = next(iter(named(".nvmrc") + named(".node-version")), None)
        run("sudo", "-n", "n", node_hint.read_text().strip() if node_hint else "lts")
        for manifest in named("package.json"):
            directory = manifest.parent
            # A workspace root installs packages for its children.
            if any((parent / "pnpm-workspace.yaml").exists() or
                   ((parent / "package.json").exists() and
                    json.loads((parent / "package.json").read_text()).get("workspaces"))
                   for parent in directory.parents):
                continue
            package = json.loads(manifest.read_text())
            manager = package.get("packageManager", "")
            if manager.startswith(("pnpm@", "yarn@")):
                apt_manager = manager.split("+", 1)[0]
            elif (directory / "pnpm-lock.yaml").exists():
                apt_manager = "pnpm"
            elif (directory / "yarn.lock").exists():
                apt_manager = "yarn"
            elif (directory / "bun.lock").exists() or manager.startswith("bun@"):
                if sys.argv[1] == "custom":
                    continue
                raise RuntimeError("Bun projects require explicit setup.commands.")
            else:
                apt_manager = "npm"
            if apt_manager.startswith("yarn"):
                run("sudo", "-n", "npm", "install", "--global", "corepack")
                run("sudo", "-n", "corepack", "enable")
                run("corepack", "install", "--global", apt_manager if "@" in apt_manager else "yarn@stable")
            elif apt_manager != "npm":
                run("sudo", "-n", "npm", "install", "--global", apt_manager)
            if sys.argv[1] != "custom":
                tool = apt_manager.split("@", 1)[0]
                args = [tool, "ci" if tool == "npm" and (directory / "package-lock.json").exists() else "install"]
                if tool == "pnpm" and (directory / "pnpm-lock.yaml").exists():
                    args.append("--frozen-lockfile")
                run(*args, cwd=directory)
    projects = [p for p in files if p.suffix in {".csproj", ".fsproj", ".vbproj"}]
    if projects:
        apt("libicu-dev", "libssl-dev", "zlib1g-dev")
        installer = Path.home() / ".cache/orvex/dotnet-install.sh"
        installer.parent.mkdir(parents=True, exist_ok=True)
        run("curl", "-fsSL", "https://dot.net/v1/dotnet-install.sh", "-o", str(installer))
        versions = {json.loads(p.read_text())["sdk"]["version"] for p in named("global.json")}
        channels = set()
        if not versions:
            for project in projects + named("Directory.Build.props"):
                channels.update(re.findall(r"net(?:coreapp)?(\d+\.\d+)", project.read_text()))
        for option, value in [("--version", v) for v in sorted(versions)] + [("--channel", c) for c in sorted(channels or ({"LTS"} if not versions else set()))]:
            run("bash", str(installer), option, value, "--install-dir", str(Path.home() / ".dotnet"))
        run("sudo", "-n", "ln", "-sf", str(Path.home() / ".dotnet/dotnet"), "/usr/local/bin/dotnet")
        if sys.argv[1] != "custom":
            for project in projects:
                run("dotnet", "restore", str(project))
    if named("Cargo.toml"):
        run("curl", "-fsSL", "https://sh.rustup.rs", "-o", "/tmp/orvex-rustup.sh")
        run("sh", "/tmp/orvex-rustup.sh", "-y", "--profile", "minimal")
        os.environ["PATH"] = str(Path.home() / ".cargo/bin") + ":" + os.environ["PATH"]
        if sys.argv[1] != "custom":
            for manifest in named("Cargo.toml"):
                run("cargo", "fetch", "--manifest-path", str(manifest))
    if named("go.mod"):
        apt("golang-go")
        if sys.argv[1] != "custom":
            for manifest in named("go.mod"):
                run("go", "mod", "download", cwd=manifest.parent)
    if named("requirements.txt") or named("pyproject.toml"):
        apt("python3-venv", "python3-dev")
        for directory in sorted({p.parent for p in named("requirements.txt") + named("pyproject.toml")}):
            run("python3", "-m", "venv", str(directory / ".venv"))
            if sys.argv[1] != "custom":
                pip = str((directory / ".venv/bin/pip").resolve())
                if (directory / "requirements.txt").exists():
                    run(pip, "install", "-r", "requirements.txt", cwd=directory)
                else:
                    run(pip, "install", "-e", ".", cwd=directory)
    if named("Gemfile"):
        apt("ruby-full", "ruby-bundler")
        if sys.argv[1] != "custom":
            for manifest in named("Gemfile"):
                run("bundle", "config", "set", "--local", "path", "vendor/bundle", cwd=manifest.parent)
                run("bundle", "install", cwd=manifest.parent)


if __name__ == "__main__":
    main()
