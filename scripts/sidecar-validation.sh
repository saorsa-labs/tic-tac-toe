#!/usr/bin/env bash

# Shared validation for the managed-agent executables listed in Tauri's
# externalBin configuration. This file is sourced by the staging and release
# scripts so both paths enforce the same fail-closed contract.

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
