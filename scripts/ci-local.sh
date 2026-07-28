#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="quality"
RUN_QUALITY=1
WINDOWS_TARGET="x86_64-pc-windows-gnu"
BOOTSTRAP_MINGW=0

usage() {
  cat <<'EOF'
Usage: bash scripts/ci-local.sh [mode] [--skip-quality]

Modes:
  --desktop-check   Check the current platform desktop target
  --desktop-build   Build the current platform release executable
  --desktop-package Build the current platform release executable and desktop packages
  --windows-check   Strictly lint the Windows GNU target
  --windows-build   Cross-build and verify the Windows x64 release executable
  --bootstrap-mingw Download a rootless Debian MinGW toolchain into target/toolchains

For an extracted MinGW toolchain, set CAMELLIA_NEXUS_MINGW_BIN to its bin directory.
PFX Authenticode signing is intentionally handled by ci-local.ps1 on Windows.
EOF
}

while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --desktop-check) MODE="desktop-check" ;;
    --desktop-build) MODE="desktop-build" ;;
    --desktop-package) MODE="desktop-package" ;;
    --windows-check) MODE="windows-check" ;;
    --windows-build) MODE="windows-build" ;;
    --bootstrap-mingw) BOOTSTRAP_MINGW=1 ;;
    --skip-quality) RUN_QUALITY=0 ;;
    --help|-h) usage; exit 0 ;;
    *) echo "Unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Required command is unavailable: $1" >&2
    exit 127
  fi
}

bootstrap_mingw() {
  require_command apt-get
  require_command dpkg-deb
  local cache_dir="$ROOT_DIR/target/toolchains/mingw-debian"
  local package_dir="$cache_dir/packages"
  local toolchain_dir="$cache_dir/root"
  local marker="$cache_dir/.complete"
  if [[ ! -f "$marker" || \
        ( ! -x "$toolchain_dir/usr/bin/x86_64-w64-mingw32-gcc" && \
          ! -x "$toolchain_dir/usr/bin/x86_64-w64-mingw32-gcc-posix" ) ]]; then
    echo "==> Download rootless MinGW toolchain"
    rm -rf "$cache_dir"
    mkdir -p "$package_dir" "$toolchain_dir"
    (
      cd "$package_dir"
      apt-get download \
        binutils-mingw-w64-x86-64 \
        mingw-w64-common \
        mingw-w64-x86-64-dev \
        gcc-mingw-w64-base \
        gcc-mingw-w64-x86-64-posix-runtime \
        gcc-mingw-w64-x86-64-posix
    )
    local package
    for package in "$package_dir"/*.deb; do
      dpkg-deb --extract "$package" "$toolchain_dir"
    done
    if [[ ! -x "$toolchain_dir/usr/bin/x86_64-w64-mingw32-gcc" && \
          -x "$toolchain_dir/usr/bin/x86_64-w64-mingw32-gcc-posix" ]]; then
      ln -sf x86_64-w64-mingw32-gcc-posix \
        "$toolchain_dir/usr/bin/x86_64-w64-mingw32-gcc"
    fi
    touch "$marker"
  fi
  export CAMELLIA_NEXUS_MINGW_BIN="$toolchain_dir/usr/bin"
}

prepare_windows_cross() {
  if [[ "$BOOTSTRAP_MINGW" -eq 1 ]]; then
    bootstrap_mingw
  fi
  if [[ -n "${CAMELLIA_NEXUS_MINGW_BIN:-}" ]]; then
    if [[ ! -d "$CAMELLIA_NEXUS_MINGW_BIN" ]]; then
      echo "CAMELLIA_NEXUS_MINGW_BIN is not a directory: $CAMELLIA_NEXUS_MINGW_BIN" >&2
      exit 2
    fi
    export PATH="$CAMELLIA_NEXUS_MINGW_BIN:$PATH"
  fi

  local target_dir
  target_dir="$(rustc --print target-libdir --target "$WINDOWS_TARGET" 2>/dev/null || true)"
  if [[ -z "$target_dir" || ! -d "$target_dir" ]]; then
    echo "Rust target is unavailable: $WINDOWS_TARGET" >&2
    echo "Install it with: rustup target add $WINDOWS_TARGET" >&2
    exit 127
  fi

  local tool
  for tool in gcc windres dlltool ar ranlib; do
    require_command "x86_64-w64-mingw32-$tool"
  done

  CC_x86_64_pc_windows_gnu="$(command -v x86_64-w64-mingw32-gcc)"
  AR_x86_64_pc_windows_gnu="$(command -v x86_64-w64-mingw32-ar)"
  RANLIB_x86_64_pc_windows_gnu="$(command -v x86_64-w64-mingw32-ranlib)"
  RC_x86_64_pc_windows_gnu="$(command -v x86_64-w64-mingw32-windres)"
  export CC_x86_64_pc_windows_gnu AR_x86_64_pc_windows_gnu
  export RANLIB_x86_64_pc_windows_gnu RC_x86_64_pc_windows_gnu
  export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="$CC_x86_64_pc_windows_gnu"
}

verify_windows_executable() {
  local executable="$1"
  if [[ ! -f "$executable" ]]; then
    echo "Windows executable was not produced: $executable" >&2
    exit 1
  fi
  require_command file
  require_command strings
  if ! file "$executable" | grep -F "PE32+" >/dev/null; then
    echo "Build output is not a Windows x64 PE executable: $executable" >&2
    exit 1
  fi

  local marker
  for marker in \
    "asInvoker" \
    "WinVerifyTrust" \
    "Camellia Computing" \
    "ShellExecuteW"; do
    if ! strings "$executable" | grep -F -- "$marker" >/dev/null; then
      echo "Windows executable is missing required startup linkage marker: $marker" >&2
      exit 1
    fi
  done

  for marker in "--startup-bridge" "Failed to create the elevated startup task" "schtasks.exe" "runas"; do
    if strings "$executable" | grep -F -- "$marker" >/dev/null; then
      echo "Windows executable contains forbidden whole-application elevation marker: $marker" >&2
      exit 1
    fi
  done

  echo "Verified normal-user Windows startup linkage: $executable"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$executable"
  fi
}

run_step() {
  local label="$1"
  shift
  echo "==> $label"
  "$@"
}

prepare_privilege_broker() {
  local target="$1"
  local debug="$2"
  export TAURI_ENV_TARGET_TRIPLE="$target"
  export TAURI_ENV_DEBUG="$debug"
  run_step "Prepare the $target privilege broker" \
    node scripts/prepare-privilege-broker.mjs
  export CAMELLIA_NEXUS_PRIVILEGE_BROKER_PREPARED=1
}

macos_dmg_requested() {
  local bundles="${CAMELLIA_NEXUS_TAURI_BUNDLES:-}"
  [[ "$(uname -s)" == "Darwin" ]] || return 1
  bundles="${bundles//[[:space:]]/}"
  case ",$bundles," in
    *,dmg,*) return 0 ;;
    *) return 1 ;;
  esac
}

remove_failed_macos_dmg_outputs() {
  local bundle_dir="$CARGO_TARGET_DIR/release/bundle"
  local artifact
  [[ -d "$bundle_dir" ]] || return 0
  while IFS= read -r -d '' artifact; do
    rm -f -- "$artifact"
  done < <(find "$bundle_dir" -type f -name '*.dmg' -print0)
}

run_desktop_package() {
  local label="$1"
  shift
  if ! macos_dmg_requested; then
    run_step "$label" "$@"
    return
  fi

  local attempt log_path status
  local -a pipeline_status
  log_path="$(mktemp "${TMPDIR:-/tmp}/camellia-nexus-dmg.XXXXXX")"
  for attempt in 1 2; do
    echo "==> $label"
    if "$@" 2>&1 | tee "$log_path"; then
      rm -f "$log_path"
      return 0
    else
      pipeline_status=("${PIPESTATUS[@]}")
      status="${pipeline_status[0]}"
      if [[ "$status" -eq 0 ]]; then
        status="${pipeline_status[1]}"
      fi
    fi

    if [[ "$attempt" -eq 2 ]] ||
      ! grep -Fq 'error running bundle_dmg.sh' "$log_path"; then
      rm -f "$log_path"
      return "$status"
    fi

    echo "Tauri DMG creation failed; retrying once after a short backoff." >&2
    remove_failed_macos_dmg_outputs
    sleep 5
    : > "$log_path"
  done
}

require_command cargo
require_command node
require_command pnpm
cd "$ROOT_DIR"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT_DIR/target}"

clean_tauri_build_cache() {
  local profile
  for profile in debug release; do
    rm -rf "$CARGO_TARGET_DIR/$WINDOWS_TARGET/$profile/build/tauri-"*
    rm -rf "$CARGO_TARGET_DIR/$profile/build/tauri-"*
  done
}

run_step "Install locked frontend dependencies" pnpm --dir ui install --frozen-lockfile

if [[ "$RUN_QUALITY" -eq 1 ]]; then
  run_step "Audit release authorization boundaries" node scripts/audit-release-security.mjs
  run_step "Validate embedded entitlement trust" node scripts/validate-entitlement-keys.mjs
  if [[ -f "$ROOT_DIR/../nexus-management-server/src/contracts.rs" ]]; then
    run_step "Check the sibling license-service contract" \
      node scripts/check-cross-repo-contract.mjs
    run_step "Test sibling contract drift detection" \
      node scripts/test-cross-repo-contract.mjs
  else
    echo "==> Sibling license-service checkout unavailable; cross-repository contract check deferred"
  fi
  run_step "Check Rust formatting" cargo fmt --all -- --check
  run_step "Lint platform-independent Rust targets" cargo clippy --workspace --locked --no-default-features --all-targets -- -D warnings
  run_step "Run platform-independent and native process tests" cargo test --workspace --locked --no-default-features
  run_step "Check Svelte and TypeScript" pnpm --dir ui check
  run_step "Test frontend utilities" pnpm --dir ui test
  run_step "Build frontend" pnpm --dir ui build
fi

case "$MODE" in
  quality) ;;
  desktop-check)
    prepare_privilege_broker "$(rustc --print host-tuple)" true
    run_step "Check the native desktop target" cargo check --locked -p camellia-nexus
    ;;
  desktop-build)
    prepare_privilege_broker "$(rustc --print host-tuple)" false
    run_step "Build the native release executable without an installer" \
      node ui/node_modules/@tauri-apps/cli/tauri.js build --ci --no-bundle -- --locked
    ;;
  desktop-package)
    prepare_privilege_broker "$(rustc --print host-tuple)" false
    package_args=(ui/node_modules/@tauri-apps/cli/tauri.js build --ci)
    package_args+=(--config src-tauri/tauri.privilege-broker.conf.json)
    if [[ -n "${CAMELLIA_NEXUS_TAURI_BUNDLES:-}" ]]; then
      package_args+=(--bundles "$CAMELLIA_NEXUS_TAURI_BUNDLES")
    fi
    if [[ "$(uname -s)" == "Darwin" && "${CAMELLIA_NEXUS_MACOS_SIGN:-disabled}" != "required" ]]; then
      package_args+=(--no-sign)
    fi
    if macos_dmg_requested; then
      package_args+=(--verbose)
    fi
    package_args+=(-- --locked)
    run_desktop_package "Build the native release executable and desktop packages" \
      node "${package_args[@]}"
    ;;
  windows-check)
    prepare_windows_cross
    prepare_privilege_broker "$WINDOWS_TARGET" true
    clean_tauri_build_cache
    run_step "Strictly lint the Windows x64 desktop target" \
      cargo clippy --workspace --locked --target "$WINDOWS_TARGET" --all-targets --all-features -- -D warnings
    ;;
  windows-build)
    prepare_windows_cross
    prepare_privilege_broker "$WINDOWS_TARGET" false
    clean_tauri_build_cache
    run_step "Strictly lint the Windows x64 desktop target" \
      cargo clippy --workspace --locked --target "$WINDOWS_TARGET" --all-targets --all-features -- -D warnings
    run_step "Build the Windows x64 release executable without an installer" \
      node ui/node_modules/@tauri-apps/cli/tauri.js build --ci --no-bundle --target "$WINDOWS_TARGET" -- --locked
    verify_windows_executable "$ROOT_DIR/target/$WINDOWS_TARGET/release/camellia-nexus.exe"
    ;;
esac

echo "Camellia Nexus local CI completed successfully."
