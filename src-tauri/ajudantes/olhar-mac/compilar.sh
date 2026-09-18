#!/usr/bin/env bash
# Protótipo 2C-1 separado: não substitui o gravador que já está em produção.
set -euo pipefail
cd "$(dirname "$0")"
DESTINO_OLHAR="${1:-/tmp/dnos-olhar-mac-universal}"
TMP_OLHAR="$(mktemp -d)"
trap 'rm -rf "$TMP_OLHAR"' EXIT
SDK_OLHAR="$(xcrun --sdk macosx --show-sdk-path)"
for ARQUITETURA_OLHAR in arm64 x86_64; do
  swiftc -O -target "$ARQUITETURA_OLHAR-apple-macos11" -sdk "$SDK_OLHAR" -o "$TMP_OLHAR/$ARQUITETURA_OLHAR" main.swift
done
lipo -create -output "$DESTINO_OLHAR" "$TMP_OLHAR/arm64" "$TMP_OLHAR/x86_64"
codesign --force --sign - --identifier ai.dnia.dnos.olhar-mac "$DESTINO_OLHAR"
