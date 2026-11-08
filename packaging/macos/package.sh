#!/usr/bin/env bash
# Build, sign and (optionally) notarize the macOS release artifacts:
#
#   $DIST/filmcraft-<version>-macos-<arch>.dmg          FilmCraft.app on a drag-to-Applications DMG
#   $DIST/filmcraft-cli-<version>-macos-<arch>.zip      the headless CLI
#
# Usage: packaging/macos/package.sh [--arch universal|aarch64|x86_64] [--skip-build] [--ai-alpha]
# --ai-alpha requires --arch aarch64 and builds FilmCraft AI with optional voice features.
#
# Signing (env):
#   MACOS_SIGN_IDENTITY   codesign identity (name or SHA-1). Default "-" = ad-hoc (local testing;
#                         Gatekeeper will reject the result on other Macs). In CI, import-cert.sh sets it.
#   MACOS_KEYCHAIN        keychain holding the identity (optional)
# Notarization (env; all three needed, and a real identity):
#   APPLE_ID, APPLE_PASSWORD (app-specific password), APPLE_TEAM_ID
set -euo pipefail
# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"
HERE="$ROOT/packaging/macos"

ARCH=universal
SKIP_BUILD=0
AI_ALPHA=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --arch) ARCH="$2"; shift 2 ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    --ai-alpha) AI_ALPHA=1; shift ;;
    -h | --help) sed -n '2,16p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done
case "$ARCH" in
  universal) TARGETS=(aarch64-apple-darwin x86_64-apple-darwin) ;;
  aarch64) TARGETS=(aarch64-apple-darwin) ;;
  x86_64) TARGETS=(x86_64-apple-darwin) ;;
  *) echo "unknown --arch $ARCH" >&2; exit 2 ;;
esac

APP_TITLE=FilmCraft
ARTIFACT_NAME=filmcraft
if [[ "$AI_ALPHA" = 1 ]]; then
  if [[ "$SKIP_BUILD" = 1 ]]; then
    echo "error: --ai-alpha cannot use --skip-build; the alpha feature configuration must be built" >&2
    exit 2
  fi
  if [[ "$ARCH" != aarch64 ]]; then
    echo "error: --ai-alpha requires --arch aarch64" >&2
    exit 2
  fi
  APP_TITLE="FilmCraft AI"
  ARTIFACT_NAME=filmcraft-ai
fi

# Reuse the native release cache on Apple Silicon instead of creating a second identical
# target cache. Non-native and standard release builds keep their explicit target layout.
AI_NATIVE=0
if [[ "$AI_ALPHA" = 1 ]] && [[ "$(rustc -vV | sed -n 's/^host: //p')" = aarch64-apple-darwin ]]; then
  AI_NATIVE=1
fi

# Keep in sync with LSMinimumSystemVersion in Info.plist.in.
export MACOSX_DEPLOYMENT_TARGET=11.0
IDENTITY="${MACOS_SIGN_IDENTITY:--}"
SHORT_VERSION="${VERSION%%-*}"
WORK="$CARGO_TARGET_DIR/macos-package"
APP="$WORK/$APP_TITLE.app"
DMG="$DIST/$ARTIFACT_NAME-$VERSION-macos-$ARCH.dmg"
CLI_ZIP="$DIST/filmcraft-cli-$VERSION-macos-$ARCH.zip"

NOTARIZE=0
if [[ "$IDENTITY" = "-" ]]; then
  warn "macOS: ad-hoc signing (no MACOS_SIGN_IDENTITY); artifacts are not notarized"
elif [[ -n "${APPLE_ID:-}" ]] && [[ -n "${APPLE_PASSWORD:-}" ]] && [[ -n "${APPLE_TEAM_ID:-}" ]]; then
  NOTARIZE=1
else
  warn "macOS: APPLE_ID / APPLE_PASSWORD / APPLE_TEAM_ID incomplete; signed but not notarized"
fi

echo "==> FilmCraft $VERSION for macOS ($ARCH), identity: $IDENTITY, notarize: $NOTARIZE"

# ---- build -------------------------------------------------------------------------------------
if [[ "$SKIP_BUILD" = 0 ]]; then
  args=()
  if [[ "$AI_ALPHA" = 1 ]]; then args+=(--features filmcraft/ai-alpha,filmcraft-cli/whisper); fi
  if [[ "$AI_NATIVE" = 0 ]]; then
    for t in "${TARGETS[@]}"; do args+=(--target "$t"); done
  fi
  (cd "$ROOT" && cargo build --release --locked -p filmcraft -p filmcraft-cli "${args[@]}")
fi

rm -rf "$WORK"
mkdir -p "$WORK/bin"
for bin in filmcraft filmcraft-cli; do
  inputs=()
  if [[ "$AI_NATIVE" = 1 ]]; then
    inputs+=("$CARGO_TARGET_DIR/release/$bin")
  else
    for t in "${TARGETS[@]}"; do inputs+=("$CARGO_TARGET_DIR/$t/release/$bin"); done
  fi
  lipo -create -output "$WORK/bin/$bin" "${inputs[@]}"
  lipo -info "$WORK/bin/$bin"
done

# ---- signing helpers ---------------------------------------------------------------------------
sign() {
  # Hardened runtime + secure timestamp for a real identity; ad-hoc can't be timestamped.
  local ts=(--timestamp)
  [[ "$IDENTITY" = "-" ]] && ts=(--timestamp=none)
  local kc=()
  [[ -n "${MACOS_KEYCHAIN:-}" ]] && kc=(--keychain "$MACOS_KEYCHAIN")
  codesign --force --sign "$IDENTITY" ${kc[@]+"${kc[@]}"} "${ts[@]}" "$@"
}

notarize() {
  local file="$1" out id status
  echo "==> notarizing $(basename "$file") (this can take a few minutes)"
  out="$(xcrun notarytool submit "$file" --apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" \
    --team-id "$APPLE_TEAM_ID" --wait --timeout 1h --output-format json)" || true
  echo "$out"
  id="$(printf '%s' "$out" | plutil -extract id raw -o - - 2>/dev/null || true)"
  status="$(printf '%s' "$out" | plutil -extract status raw -o - - 2>/dev/null || true)"
  if [[ "$status" != "Accepted" ]]; then
    if [[ -n "$id" ]]; then
      xcrun notarytool log "$id" --apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID" || true
    fi
    echo "error: notarization of $(basename "$file") failed (status: ${status:-unknown})" >&2
    exit 1
  fi
}

# ---- FilmCraft.app ----------------------------------------------------------------------------
echo "==> assembling $APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
# Executable and icon carry the display name (CFBundleExecutable / CFBundleIconFile).
cp "$WORK/bin/filmcraft" "$APP/Contents/MacOS/FilmCraft"
# The Console spawns this matching sibling for authenticated MCP, without a checkout.
cp "$WORK/bin/filmcraft-cli" "$APP/Contents/MacOS/filmcraft-cli"
cp "$ROOT/assets/app-icon/filmcraft.icns" "$APP/Contents/Resources/FilmCraft.icns"
cp "$ROOT/NOTICE" "$APP/Contents/Resources/"
copy_docs "$APP/Contents/Resources"
cp "$ROOT/ATTRIBUTION.md" "$APP/Contents/Resources/"
cp "$ROOT"/assets/fonts/OFL-*.txt "$APP/Contents/Resources/"
cp "$ROOT/docs/brand/LICENSE-brand.txt" "$APP/Contents/Resources/"
if [[ "$AI_ALPHA" = 1 ]]; then
  cp "$ROOT/docs/apple-silicon-alpha.md" "$APP/Contents/Resources/APPLE-SILICON-ALPHA.md"
  cp "$ROOT/docs/apple-silicon-alpha-install.md" "$APP/Contents/Resources/INSTALL.md"
  # The upstream README includes official branding; this independent distribution has its
  # own install document while retaining copyright, licence and source attribution notices.
  cp "$ROOT/docs/apple-silicon-alpha-install.md" "$APP/Contents/Resources/README.md"
fi
sed -e "s/@VERSION@/$VERSION/g" -e "s/@SHORT_VERSION@/$SHORT_VERSION/g" \
  -e "s/@BUILD_SHA@/${FILMCRAFT_BUILD_SHA:-unknown}/g" \
  "$HERE/Info.plist.in" >"$APP/Contents/Info.plist"
if [[ "$AI_ALPHA" = 1 ]]; then
  /usr/libexec/PlistBuddy -c "Set :CFBundleDisplayName FilmCraft AI" "$APP/Contents/Info.plist"
  /usr/libexec/PlistBuddy -c "Set :CFBundleName FilmCraft AI" "$APP/Contents/Info.plist"
  /usr/libexec/PlistBuddy -c "Add :FilmCraftDistribution string apple-silicon-ai-alpha" "$APP/Contents/Info.plist"
fi
plutil -lint "$APP/Contents/Info.plist"
printf 'APPL????' >"$APP/Contents/PkgInfo"
# copy_docs above includes licences for embedded craft-fonts (CRAFT_FONTS_DIR).

# Sign inside-out: nested code first, then the bundle itself (no --deep on the final signature).
# Sign the matching MCP helper before the enclosing app.
sign --options runtime --entitlements "$HERE/entitlements.plist" "$APP/Contents/MacOS/filmcraft-cli"
sign --options runtime --entitlements "$HERE/entitlements.plist" "$APP/Contents/MacOS/FilmCraft"
sign --options runtime --entitlements "$HERE/entitlements.plist" "$APP"
codesign --verify --strict --deep --verbose=2 "$APP"

if [[ "$NOTARIZE" = 1 ]]; then
  ditto -c -k --keepParent "$APP" "$WORK/FilmCraft-notarize.zip"
  notarize "$WORK/FilmCraft-notarize.zip"
  xcrun stapler staple "$APP"
  xcrun stapler validate "$APP"
  spctl --assess --type execute -vvv "$APP"
fi

# ---- DMG ---------------------------------------------------------------------------------------
echo "==> building $DMG"
STAGE="$WORK/dmg"
mkdir -p "$STAGE"
ditto "$APP" "$STAGE/$APP_TITLE.app"
ln -s /Applications "$STAGE/Applications"
# Finder window layout: background, icon size and positions (packaging/macos/dmg/README.md).
mkdir -p "$STAGE/.background"
cp "$HERE/dmg/background.tiff" "$STAGE/.background/background.tiff"
# The fixed layout names FilmCraft.app. Let Finder arrange the differently named alpha
# instead of shipping a stale icon record for an app that is not on this disk image.
if [[ "$AI_ALPHA" = 0 ]]; then cp "$HERE/dmg/dmg-layout.DS_Store" "$STAGE/.DS_Store"; fi
rm -f "$DMG"
# Preserve the signed bundle's filesystem metadata. makehybrid synthesizes FinderInfo
# on Mach-O files, so a byte-identical app can fail strict signature verification after
# mounting. The folder-to-image path preserves the valid bundle, verified on the
# mounted installer as well as on the staging app. The volume name remains unversioned
# for the Finder layout's background alias.
hdiutil create -srcfolder "$STAGE" -volname "FilmCraft" -fs HFS+ -format UDZO -imagekey zlib-level=9 "$DMG"
sign "$DMG"
codesign --verify --strict --verbose=2 "$DMG"
if [[ "$NOTARIZE" = 1 ]]; then
  notarize "$DMG"
  xcrun stapler staple "$DMG"
  xcrun stapler validate "$DMG"
  spctl --assess --type open --context context:primary-signature -vvv "$DMG"
fi

# ---- CLI ---------------------------------------------------------------------------------------
echo "==> building $CLI_ZIP"
CLI_DIR="$WORK/filmcraft-cli-$VERSION-macos-$ARCH"
mkdir -p "$CLI_DIR"
cp "$WORK/bin/filmcraft-cli" "$CLI_DIR/"
copy_docs "$CLI_DIR"
if [[ "$AI_ALPHA" = 1 ]]; then
  cp "$ROOT/docs/apple-silicon-alpha-install.md" "$CLI_DIR/README.md"
fi
cp "$ROOT/ATTRIBUTION.md" "$CLI_DIR/"
cp "$ROOT"/assets/fonts/OFL-*.txt "$CLI_DIR/"
cp "$ROOT/docs/brand/LICENSE-brand.txt" "$CLI_DIR/"
sign --options runtime "$CLI_DIR/filmcraft-cli"
codesign --verify --strict --verbose=2 "$CLI_DIR/filmcraft-cli"
rm -f "$CLI_ZIP"
ditto -c -k --keepParent "$CLI_DIR" "$CLI_ZIP"
# A bare Mach-O can't carry a stapled ticket; Gatekeeper looks the notarization up online.
if [[ "$NOTARIZE" = 1 ]]; then notarize "$CLI_ZIP"; fi

"$WORK/bin/filmcraft-cli" --version
echo "==> done"
ls -lh "$DMG" "$CLI_ZIP"
