# dn.os Desktop

A casca nativa do dn.os para Mac e Windows. Ela abre a instância web
(`https://dnos.dnia.ai` por padrão) numa janela própria, com ícone,
notificação nativa, deep link `dnos://` e atualização automática.

O **conteúdo** é o app web: quando o dn.os publica, a casca mostra a versão
nova sem reinstalar. A casca só é redistribuída quando ela mesma muda.
Plano completo em `docs/plano-app-desktop.md` no repositório do dn.os.

## Instalar

Baixe em **Releases**: `dn.os_x.y.z_universal.dmg` (Mac, Intel e Apple
Silicon) ou `dn.os_x.y.z_x64-setup.exe` (Windows).

### Primeira abertura sem assinatura (enquanto não há conta Apple/Windows)

- **Mac:** o sistema diz "não pode ser aberto porque o desenvolvedor não pode
  ser verificado". Abra **Ajustes → Privacidade e Segurança**, role até o
  aviso do dn.os e clique **Abrir Mesmo Assim**. Só na primeira vez.
- **Windows:** o SmartScreen mostra "o Windows protegeu o computador".
  Clique **Mais informações → Executar assim mesmo**. Só na primeira vez.

## Para um remix

A casca aponta para a instância definida em tempo de execução pela variável
`DNOS_URL` (ex.: `DNOS_URL=https://os.suaempresa.com`). Sem ela, usa a dn.ia.
Para distribuir uma casca com outra instância fixa, mude `URL_PADRAO` em
`src-tauri/src/lib.rs`, o `identifier` e as URLs em
`src-tauri/capabilities/default.json`, e gere os ícones com `npm run icons`.

## Desenvolver

Requisitos: Node 22, Rust estável (`rustup`), e no Mac o Xcode Command Line
Tools; no Windows, Visual Studio Build Tools + WebView2.

```sh
npm ci
npm run dev          # abre a casca apontando para a instância
npm run build:mac    # .dmg universal em src-tauri/target/…/bundle
cargo test --manifest-path src-tauri/Cargo.toml
```

## Publicar uma versão

1. Ajuste `version` em `package.json`, `src-tauri/tauri.conf.json` e
   `src-tauri/Cargo.toml`.
2. `git tag vX.Y.Z && git push --tags`.
3. O workflow `release` compila Mac e Windows, cria a Release com os
   instaladores e o `latest.json`. As cascas instaladas atualizam sozinhas.

Secrets necessários no repositório: `TAURI_SIGNING_PRIVATE_KEY` e
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (assinatura do updater; a chave pública
está em `tauri.conf.json`). Opcionais, quando houver conta: `APPLE_*` para
assinar e notarizar no Mac.

## O que a casca faz e o que não faz

- Faz: janela, ícone, instância única, notificação, badge, deep link,
  links externos no navegador do sistema, tela de sem conexão, updater.
- Não faz (fases 2 e 3, por vir): usar o Chrome da pessoa, capturar tela,
  mexer no mouse e teclado. Isso exige a ponte `dnos-node` na VPS e regras
  de segurança próprias.
