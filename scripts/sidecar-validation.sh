#!/usr/bin/env bash

# Shared validation for the managed-agent executables listed in Tauri's
# externalBin configuration. This file is sourced by the staging and release
# scripts so both paths enforce the same fail-closed contract.

# Campaign-only protocol strings. They exist on wip/codex-durable-app-ack
# and were found in the mis-staged v0.5.0/v0.5.1 sidecar (ttt #12). They
# are absent from every released x0x tag through v0.37.2. Bundled x0xd
# must not contain them until ADR 0030 is accepted and a released daemon
# advertises protocol v2.
X0XD_CAMPAIGN_DENYLIST=(
  recipient_ack_semantics_unavailable
  x0x-dm-durable-accepted-binding-v1
  x0x-dm-thread-v1
)

# Official x0x v0.37.2 macos-arm64 x0xd, extracted from
# x0x-macos-arm64.tar.gz (sha256
# b3e3c49f60154cd7e7c589964cd7fd5d0aa9cdc65cb9a8f124cdcc23f36bcaa9).
PINNED_X0XD_VERSION="0.37.2"
PINNED_X0XD_AARCH64_APPLE_DARWIN_SHA256="50f7f0b17567ef1153639849ddce1f8fec4a83831cabd692e3a219c93f219742"
PINNED_X0XD_MACOS_ARM64_TARBALL_SHA256="b3e3c49f60154cd7e7c589964cd7fd5d0aa9cdc65cb9a8f124cdcc23f36bcaa9"

reject_campaign_x0xd() {
  local binary="$1"
  local hit
  if [[ ! -f "$binary" ]]; then
    echo "FATAL: x0xd sidecar missing: $binary" >&2
    return 5
  fi
  for hit in "${X0XD_CAMPAIGN_DENYLIST[@]}"; do
    if strings "$binary" | grep -F -q -- "$hit"; then
      echo "FATAL: $binary contains unreleased campaign string '$hit' (ttt #12)" >&2
      return 5
    fi
  done
}

# Version + campaign-string identity. Used on both the unsigned official
# asset (plus sha256 pin) and the Developer-ID-signed copy inside the app
# bundle (sha256 cannot survive codesign; --remove-signature does not
# restore the original bytes either).
assert_official_x0xd_identity() {
  local binary="$1"
  reject_campaign_x0xd "$binary" || return $?
  local ver
  ver="$("$binary" --version 2>/dev/null || true)"
  if [[ "$ver" != *"$PINNED_X0XD_VERSION"* ]]; then
    echo "FATAL: $binary reports '$ver', expected $PINNED_X0XD_VERSION" >&2
    return 5
  fi
}

# Unsigned official GitHub asset. Stage / fetch paths only.
assert_official_x0xd_pin() {
  local binary="$1"
  assert_official_x0xd_identity "$binary" || return $?
  local actual
  actual="$(shasum -a 256 "$binary" | awk '{print $1}')"
  if [[ "$actual" != "$PINNED_X0XD_AARCH64_APPLE_DARWIN_SHA256" ]]; then
    echo "FATAL: $binary sha256 $actual != pinned official v${PINNED_X0XD_VERSION} $PINNED_X0XD_AARCH64_APPLE_DARWIN_SHA256" >&2
    return 5
  fi
}

# Post-codesign copy inside tic-tac-toe.app. Hash is expected to differ
# from PINNED_X0XD_AARCH64_APPLE_DARWIN_SHA256.
assert_official_x0xd_bundle() {
  local binary="$1"
  assert_official_x0xd_identity "$binary" || return $?
  if [[ "$(uname -s)" == "Darwin" ]]; then
    if ! codesign --verify --verbose=2 "$binary" >/dev/null 2>&1; then
      echo "FATAL: bundled x0xd is not codesigned: $binary" >&2
      return 5
    fi
  fi
}

validate_managed_agent_sidecars() {
  if [[ $# -lt 2 || $# -gt 3 ]]; then
    echo "FATAL: validate_managed_agent_sidecars requires DIRECTORY TARGET_TRIPLE [NAME_SUFFIX]" >&2
    return 2
  fi

  local directory="$1"
  local target_triple="$2"
  local name_suffix
  if [[ $# -eq 3 ]]; then
    name_suffix="$3"
  else
    name_suffix="-$target_triple"
  fi

  local extension=""
  case "$target_triple" in
    *-windows-*) extension=".exe" ;;
  esac

  local -a required_names=(buzz-acp buzz-agent buzz-x0x-mcp)
  local name candidate size description
  for name in "${required_names[@]}"; do
    candidate="$directory/$name$name_suffix$extension"

    if [[ ! -f "$candidate" ]]; then
      echo "FATAL: required external binary missing: $candidate" >&2
      return 5
    fi
    if [[ ! -x "$candidate" ]]; then
      echo "FATAL: required external binary is not executable: $candidate" >&2
      return 5
    fi

    size="$(wc -c < "$candidate")"
    if (( size == 17 )) && cmp -s "$candidate" <(printf '#!/bin/sh\nexit 0\n'); then
      echo "FATAL: refusing inert 17-byte external-bin placeholder: $candidate" >&2
      return 5
    fi

    if ! description="$(file -b "$candidate")"; then
      echo "FATAL: could not inspect external binary: $candidate" >&2
      return 5
    fi

    case "$target_triple" in
      aarch64-apple-darwin)
        [[ "$description" == *Mach-O* && "$description" == *arm64* ]] || {
          echo "FATAL: external binary is not target-native for $target_triple: $candidate ($description)" >&2
          return 5
        }
        ;;
      x86_64-apple-darwin)
        [[ "$description" == *Mach-O* && "$description" == *x86_64* ]] || {
          echo "FATAL: external binary is not target-native for $target_triple: $candidate ($description)" >&2
          return 5
        }
        ;;
      aarch64-*-linux-*)
        [[ "$description" == *ELF* && "$description" == *aarch64* ]] || {
          echo "FATAL: external binary is not target-native for $target_triple: $candidate ($description)" >&2
          return 5
        }
        ;;
      x86_64-*-linux-*)
        [[ "$description" == *ELF* && "$description" == *x86-64* ]] || {
          echo "FATAL: external binary is not target-native for $target_triple: $candidate ($description)" >&2
          return 5
        }
        ;;
      aarch64-*-windows-*)
        [[ "$description" == *PE32+* && "$description" == *Aarch64* ]] || {
          echo "FATAL: external binary is not target-native for $target_triple: $candidate ($description)" >&2
          return 5
        }
        ;;
      x86_64-*-windows-*)
        [[ "$description" == *PE32+* && "$description" == *x86-64* ]] || {
          echo "FATAL: external binary is not target-native for $target_triple: $candidate ($description)" >&2
          return 5
        }
        ;;
      *)
        echo "FATAL: unsupported target triple for external-binary validation: $target_triple" >&2
        return 5
        ;;
    esac

    printf '[sidecars] validated %s (%s)\n' "$candidate" "$description" >&2
  done
}
