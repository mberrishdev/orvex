#!/bin/bash
# Install the released desktop app. Compatible with macOS's Bash 3.2.
set -euo pipefail

fail() { printf 'Orvex: %s\n' "$*" >&2; exit 1; }
usage() {
  cat <<'HELP'
Usage: bash install.sh [options]
  --version VERSION      Install a particular release (optional v prefix).
  --channel beta|stable  Release feed; defaults to beta.
  --install-dir PATH     Application directory; defaults to /Applications when writable,
                         otherwise ~/Applications. Existing installations take priority.
  --download-only        Download and verify without installing.
  --output-dir PATH      Download destination (default: ./orvex-downloads).
  --help                 Show this help.
Windows: use install.ps1 in PowerShell.
HELP
}

channel=beta
version=
install_dir=
download_only=false
output_dir="$PWD/orvex-downloads"
while [ "$#" -gt 0 ]; do
  case "$1" in
    --version|--channel|--install-dir|--output-dir)
      [ "$#" -ge 2 ] && [ -n "$2" ] || fail "$1 requires a value."
      case "$1" in
        --version) version="${2#v}" ;;
        --channel) channel="$2" ;;
        --install-dir) install_dir="$2" ;;
        --output-dir) output_dir="$2" ;;
      esac
      shift 2 ;;
    --download-only) download_only=true; shift ;;
    --help|-h) usage; exit 0 ;;
    *) fail "Unknown option: $1. Use --help." ;;
  esac
done
case "$channel" in beta|stable) ;; *) fail 'Channel must be beta or stable.' ;; esac
valid_version() { [[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-beta\.[0-9]+)?$ ]]; }
[ -z "$version" ] || valid_version "$version" || fail 'Expected a version such as 0.1.0-beta.3.'
[ "$(uname -s)" = Darwin ] || fail 'This script supports macOS. On Windows, use install.ps1 in PowerShell.'
arch="$(uname -m)"
if [ "$arch" = x86_64 ] && [ "$(sysctl -n hw.optional.arm64 2>/dev/null || printf 0)" = 1 ]; then arch=arm64; fi
[ "$arch" = arm64 ] || fail 'There is no Intel Mac release yet; Apple Silicon is required.'
for tool in curl plutil shasum awk mktemp; do command -v "$tool" >/dev/null || fail "Required command missing: $tool"; done

work="$(mktemp -d "${TMPDIR:-/tmp}/orvex-install.XXXXXXXX")"
stage=
destination=
cleanup() {
  status=$?
  if [ -n "$stage" ] && [ -e "$stage/previous.app" ] && [ ! -e "$destination" ]; then
    if ! mv "$stage/previous.app" "$destination"; then
      printf 'Orvex: could not restore the previous app. It is preserved at %s/previous.app\n' "$stage" >&2
      rm -rf "$work"
      return 1
    fi
  fi
  rm -rf "$work"
  if [ -n "$stage" ]; then rm -rf "$stage"; fi
  return "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
download() { curl --fail --location --show-error --silent --retry 3 --connect-timeout 20 --max-time 600 --proto '=https' --proto-redir '=https' "$1" --output "$2"; }
base=https://github.com/mberrishdev/orvex/releases
if [ -n "$version" ]; then
  feed="$base/download/v$version/latest.json"
elif [ "$channel" = beta ]; then
  feed="$base/download/updater-beta/latest.json"
else
  feed="$base/latest/download/latest.json"
fi
printf 'Finding Orvex release…\n'
download "$feed" "$work/latest.json" || fail 'Could not download the release manifest. The selected channel or version may not exist.'
resolved="$(plutil -extract version raw -o - "$work/latest.json")"
valid_version "$resolved" || fail 'Release manifest contains an invalid version.'
[ -z "$version" ] || [ "$resolved" = "$version" ] || fail 'Release manifest does not match the requested version.'
asset=Orvex.app.tar.gz
expected_url="$base/download/v$resolved/$asset"
url="$(plutil -extract platforms.darwin-aarch64.url raw -o - "$work/latest.json")"
[ "$url" = "$expected_url" ] || fail 'Release manifest contains an unexpected macOS download URL.'
printf 'Downloading Orvex %s for Apple Silicon…\n' "$resolved"
download "$expected_url" "$work/$asset"
download "$base/download/v$resolved/SHA256SUMS.txt" "$work/SHA256SUMS.txt"
expected="$(awk -v name="$asset" '$2 == name && NF == 2 { print tolower($1) }' "$work/SHA256SUMS.txt")"
[[ "$expected" =~ ^[0-9a-f]{64}$ ]] || fail 'Missing or ambiguous SHA-256 checksum for the app archive.'
actual="$(shasum -a 256 "$work/$asset" | awk '{print $1}')"
[ "$actual" = "$expected" ] || fail 'Checksum mismatch. Nothing was installed.'
printf 'SHA-256 checksum verified.\n'
if [ "$download_only" = true ]; then
  mkdir -p "$output_dir"
  [ ! -e "$output_dir/$asset" ] || fail "Download already exists: $output_dir/$asset"
  cp "$work/$asset" "$output_dir/$asset"
  printf '%s  %s\n' "$expected" "$asset" > "$output_dir/$asset.sha256"
  printf 'Downloaded %s\n' "$output_dir/$asset"
  exit 0
fi

if [ -z "$install_dir" ]; then
  if [ -d "$HOME/Applications/Orvex.app" ]; then install_dir="$HOME/Applications"
  elif [ -d /Applications/Orvex.app ] || [ -w /Applications ]; then install_dir=/Applications
  else install_dir="$HOME/Applications"; fi
fi
mkdir -p "$install_dir"
[ -w "$install_dir" ] || fail "Cannot write to $install_dir. Try --install-dir \"$HOME/Applications\"."
destination="$install_dir/Orvex.app"
if ps -u "$(id -u)" -o comm= | awk -v target="$destination/Contents/MacOS/orvex-desktop" '{ sub(/^[ \t]+/, ""); if ($0 == target) found=1 } END { exit !found }'; then
  fail 'Quit the Orvex app at this installation path before updating it.'
fi
[ ! -L "$destination" ] || fail 'Refusing to replace a symlink at the application destination.'
stage="$(mktemp -d "$install_dir/.orvex-install.XXXXXXXX")"
tar -tzf "$work/$asset" > "$work/entries"
awk '/^\// || /(^|\/)\.\.(\/|$)/ || $0 !~ /^(\.\/)?Orvex\.app(\/|$)/ { bad=1 } END { exit bad }' "$work/entries" || fail 'Unexpected paths in the application archive.'
mkdir "$stage/unpacked"
tar -xzf "$work/$asset" -C "$stage/unpacked"
app="$stage/unpacked/Orvex.app"
[ -f "$app/Contents/MacOS/orvex-desktop" ] || fail 'The archive does not contain the Orvex executable.'
[ "$(plutil -extract CFBundleIdentifier raw -o - "$app/Contents/Info.plist")" = com.orvex.desktop ] || fail 'Unexpected application identifier.'
[ "$(plutil -extract CFBundleShortVersionString raw -o - "$app/Contents/Info.plist")" = "$resolved" ] || fail 'The app version does not match the release.'
if ! codesign --verify --deep --strict "$app" 2>"$work/codesign-error"; then
  # Early betas carried only the linker's ad-hoc executable signature, without
  # sealing the bundle resources. Repair that specific legacy case after the
  # release checksum/identity checks; never replace a Developer ID signature.
  details="$(codesign -d --verbose=4 "$app" 2>&1)" || fail 'Could not inspect the application code signature.'
  if [[ "$details" = *'Signature=adhoc'* ]] && [[ "$details" = *'linker-signed'* ]] && [[ "$details" = *'Sealed Resources=none'* ]]; then
    printf 'Sealing the verified beta application bundle with an ad-hoc signature…\n'
    codesign --force --deep --sign - "$app"
    codesign --verify --deep --strict "$app" || fail 'Application code signature verification failed.'
  else
    cat "$work/codesign-error" >&2
    fail 'Application code signature verification failed.'
  fi
fi
if [ -e "$destination" ]; then mv "$destination" "$stage/previous.app"; fi
mv "$app" "$destination"
printf 'Installed Orvex %s at %s\nOpen Orvex from that location.\n' "$resolved" "$destination"
