#!/usr/bin/env bash

# Shared validation for the managed-agent executables listed in Tauri's
# externalBin configuration. This file is sourced by the staging and release
# scripts so both paths enforce the same fail-closed contract.

# Still-unreleased campaign strings. Official v0.38.0 advertises protocol v2
# and contains the durable-ACK error codes; those are no longer denylisted.
# `x0x-dm-thread-v1` is the ADR 0030 deferred DM-threading wire tag — it is
# absent from every released x0x tag through v0.38.0.
X0XD_CAMPAIGN_DENYLIST=(
  x0x-dm-thread-v1
)

# Official x0x v0.38.0 macos-arm64 x0xd, extracted from
# x0x-macos-arm64.tar.gz (sha256
# 85149d21a144d9e4b8cc347cea0f2de44d432fad33c44dd1d4823ca14d7ec8ea).
# 0.38.0: ADR 0030 product durable-by-default + hedged v2 ACK publisher.
PINNED_X0XD_VERSION="0.38.0"
PINNED_X0XD_AARCH64_APPLE_DARWIN_SHA256="daa33950bd00c332e07b34f29b0f996a6786f2747b1b3af16d20fa38986fc691"
PINNED_X0XD_MACOS_ARM64_TARBALL_SHA256="85149d21a144d9e4b8cc347cea0f2de44d432fad33c44dd1d4823ca14d7ec8ea"

# Previous official pin. mixed-version-dm-smoke fetches this as the 0.37.4
# peer so we can assert the documented 409 (product send) vs 200 (opt-out).
LEGACY_X0XD_VERSION="0.37.4"
LEGACY_X0XD_AARCH64_APPLE_DARWIN_SHA256="730b90390c8b08743dc33ab3d72326000a765e6a46d7343d8a5218d9af5a8360"
LEGACY_X0XD_MACOS_ARM64_TARBALL_SHA256="f9f990819e38058c6e46411e3e7d83a4dee219e0cfd90d4bf0ff258cca73f92c"

reject_campaign_x0xd() {
  local binary="$1"
  local hit
  if [[ ! -f "$binary" ]]; then
    echo "FATAL: x0xd sidecar missing: $binary" >&2
    return 5
  fi
  for hit in "${X0XD_CAMPAIGN_DENYLIST[@]}"; do
    # `strings` misses some Mach-O literals in official 0.38.0; search bytes.
    if grep -aF -q -- "$hit" "$binary"; then
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
