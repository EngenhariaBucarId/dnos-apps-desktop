#!/bin/bash
# Ícone no formato do macOS 26 (Tahoe), 29/09/2026.
#
# No Tahoe, app que traz só o .icns (com cantos transparentes) aparece encolhido
# dentro de uma placa clara no Dock — era o "<OS>" com borda branca nos Macs dos
# outros. O formato novo é um .icon (Icon Composer) compilado para Assets.car
# pelo actool do Xcode 26, com CFBundleIconName no Info.plist. Quem está no
# macOS 15 ou antes continua lendo o icon.icns, que fica como está.
#
# Roda no CI (o Mac do Rodrigo, no 15.3, não roda o Xcode 26). Sem Xcode 26 ou
# se o actool falhar, NÃO quebra o build: avisa e segue com o .icns de sempre.
set -uo pipefail
cd "$(dirname "$0")/../.."

XC=$(ls -d /Applications/Xcode_26*.app 2>/dev/null | sort -V | tail -1)
[ -n "$XC" ] && export DEVELOPER_DIR="$XC/Contents/Developer"
echo "Xcode: ${DEVELOPER_DIR:-padrão do sistema}"
xcrun actool --version 2>/dev/null | head -5 || true

SAIDA=$(mktemp -d)
if xcrun actool src-tauri/icons/AppIcon.icon --compile "$SAIDA" \
     --output-format human-readable-text --notices --warnings --errors \
     --output-partial-info-plist "$SAIDA/parcial.plist" \
     --app-icon AppIcon --include-all-app-icons \
     --enable-on-demand-resources NO \
     --target-device mac --minimum-deployment-target 11.0 --platform macosx \
   && [ -f "$SAIDA/Assets.car" ]; then
  cp "$SAIDA/Assets.car" src-tauri/icons/Assets.car
  # O Assets.car vai para Contents/Resources; o Info.plist diz o nome do ícone.
  cat > src-tauri/tauri.macos.conf.json <<'JSON'
{
  "bundle": {
    "macOS": {
      "files": { "Resources/Assets.car": "icons/Assets.car" }
    }
  }
}
JSON
  /usr/libexec/PlistBuddy -c "Delete :CFBundleIconName" src-tauri/Info.plist >/dev/null 2>&1 || true
  /usr/libexec/PlistBuddy -c "Add :CFBundleIconName string AppIcon" src-tauri/Info.plist
  echo "Ícone do macOS 26 compilado: $(du -h src-tauri/icons/Assets.car | cut -f1)"
  cat "$SAIDA/parcial.plist" 2>/dev/null || true
else
  echo "::warning::Ícone do macOS 26 não compilado (sem Xcode 26 ou actool falhou) — o app sai com o .icns de sempre"
fi
exit 0
