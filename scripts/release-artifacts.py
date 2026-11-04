"""Validate release versions and collect only signed, installable Tauri artifacts."""
import argparse
import datetime
import hashlib
import json
from pathlib import Path
import re
import shutil
from urllib.parse import quote

REPOSITORY = "mberrishdev/orvex"
PLATFORMS = {"macos": "darwin-aarch64", "windows": "windows-x86_64"}


def prepare(root, tag):
    if not re.fullmatch(r"v\d+\.\d+\.\d+(?:-beta\.\d+)?", tag):
        raise ValueError("Expected vX.Y.Z or vX.Y.Z-beta.N")
    config_path = root / "apps/desktop/src-tauri/tauri.conf.json"
    config = json.loads(config_path.read_text())
    if tag != "v" + config["version"]:
        raise ValueError("Release tag must match the application version")
    endpoint = ("download/updater-beta" if "-beta." in tag else "latest/download")
    config["plugins"]["updater"]["endpoints"] = [
        f"https://github.com/{REPOSITORY}/releases/{endpoint}/latest.json"
    ]
    config_path.write_text(json.dumps(config, indent=2) + "\n")


def exactly_one(directory, pattern):
    files = list(directory.glob(pattern))
    if len(files) != 1:
        raise ValueError(f"Expected one {pattern} in {directory}, found {len(files)}")
    return files[0]


def collect(root, bundle, output, platform, tag):
    config = json.loads((root / "apps/desktop/src-tauri/tauri.conf.json").read_text())
    if tag != "v" + config["version"]:
        raise ValueError("Release tag must match the application version")
    if platform == "windows":
        archive = exactly_one(bundle / "nsis", "*-setup.exe")
        installers = []
    else:
        archive = exactly_one(bundle / "macos", "*.app.tar.gz")
        installers = [exactly_one(bundle / "dmg", "*.dmg")]
    signature = Path(str(archive) + ".sig")
    signature_text = signature.read_text().strip()  # Fail before publishing unsigned files.
    if not signature_text:
        raise ValueError("Empty updater signature")
    output.mkdir(parents=True, exist_ok=True)
    for source in [archive, signature, *installers]:
        shutil.copyfile(source, output / source.name)
    manifest = {
        "version": config["version"],
        "notes": "Orvex desktop update. Save your work before installing.",
        "pub_date": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "platforms": {PLATFORMS[platform]: {
            "signature": signature_text,
            "url": f"https://github.com/{REPOSITORY}/releases/download/{tag}/{quote(archive.name)}",
        }},
    }
    (output / f"latest-{platform}.json").write_text(json.dumps(manifest, indent=2) + "\n")


def merge(artifacts, output):
    manifests = list(artifacts.glob("**/latest-*.json"))
    combined = None
    platforms = {}
    for path in manifests:
        item = json.loads(path.read_text())
        if combined and item["version"] != combined["version"]:
            raise ValueError("Mixed application versions")
        combined = item
        for platform, entry in item["platforms"].items():
            if platform in platforms:
                raise ValueError("Duplicate platform")
            platforms[platform] = entry
    if set(platforms) != set(PLATFORMS.values()):
        raise ValueError("Both macOS and Windows signed artifacts are required")
    # Validate every manifest URL maps to an artifact before creating the output.
    for entry in platforms.values():
        from urllib.parse import unquote
        name = unquote(entry["url"].rsplit("/", 1)[1])
        files = list(artifacts.glob(f"**/{name}"))
        if len(files) != 1 or not Path(str(files[0]) + ".sig").is_file():
            raise ValueError(f"Missing or ambiguous signed artifact: {name}")
        if Path(str(files[0]) + ".sig").read_text().strip() != entry["signature"]:
            raise ValueError("Signature does not match the manifest")
    output.mkdir(parents=True, exist_ok=True)
    for source in artifacts.glob("**/*"):
        if source.is_file() and not source.name.startswith("latest-"):
            destination = output / source.name
            if destination.exists():
                raise ValueError(f"Duplicate release filename: {source.name}")
            shutil.copyfile(source, destination)
    combined["platforms"] = platforms
    (output / "latest.json").write_text(json.dumps(combined, indent=2) + "\n")
    checksums = [f"{hashlib.sha256(p.read_bytes()).hexdigest()}  {p.name}" for p in sorted(output.iterdir()) if p.is_file()]
    (output / "SHA256SUMS.txt").write_text("\n".join(checksums) + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=["prepare", "collect", "merge"])
    parser.add_argument("--root", type=Path, default=Path("."))
    parser.add_argument("--bundle", type=Path, default=Path("target/release/bundle"))
    parser.add_argument("--output", type=Path, default=Path("release"))
    parser.add_argument("--artifacts", type=Path, default=Path("artifacts"))
    parser.add_argument("--platform", choices=PLATFORMS)
    parser.add_argument("--tag")
    args = parser.parse_args()
    if args.operation == "prepare":
        prepare(args.root, args.tag)
    elif args.operation == "collect":
        collect(args.root, args.bundle, args.output, args.platform, args.tag)
    else:
        merge(args.artifacts, args.output)
