#!/usr/bin/env bash
# Compila o ajudante do gravador da máquina como binário universal (arm64 + x86_64)
# em src-tauri/ajudantes/dnos-gravador-mac. Roda no CI (macOS) antes do cargo build.
set -euo pipefail
cd "$(dirname "$0")"
SDK="$(xcrun --sdk macosx --show-sdk-path)"
swiftc -O -target arm64-apple-macos11 -sdk "$SDK" -o dnos-gravador-mac-arm64 gravador-mac.swift
swiftc -O -target x86_64-apple-macos11 -sdk "$SDK" -o dnos-gravador-mac-x86_64 gravador-mac.swift
lipo -create -output dnos-gravador-mac dnos-gravador-mac-arm64 dnos-gravador-mac-x86_64
rm -f dnos-gravador-mac-arm64 dnos-gravador-mac-x86_64
chmod +x dnos-gravador-mac
# Identificador estável para o TCC (o linker deixava "dnos-gravador-mac-arm64").
codesign --force --sign - --identifier ai.dnia.dnos.gravador-mac dnos-gravador-mac
ls -la dnos-gravador-mac

# Olhar é um processo separado e somente de leitura.
bash olhar-mac/compilar.sh "$(pwd)/dnos-olhar-mac"
