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
  Desde a 0.7.7 o app é assinado com um certificado próprio (auto-assinado,
  `src-tauri/assinatura/dn-os-desktop.pem`), o mesmo em toda versão: as
  permissões de Acessibilidade, Gravação de Tela e Microfone ficam guardadas
  entre atualizações. A chave privada (`.p12`) vive só nos secrets do GitHub
  (`DNOS_MAC_CERT_P12`, `DNOS_MAC_CERT_SENHA`); se ela se perder, gera-se outra
  e as pessoas religam as permissões uma vez.
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

## Olhar meu computador (0.7.13, fase 2C-1)

No macOS, o menu do chat permite autorizar um agente a consultar uma janela do
CapCut ou Finder por cinco minutos (até 30 leituras). Barra `olhando` identifica
o agente; Parar cancela captura e revoga o acesso. Desconectar/trocar conta também
revoga. Não clica, não edita vídeos, não grava demonstração nem gera habilidades.

O ajudante `olhar-mac/main.swift` é embutido no build junto do gravador, mas é um
processo separado somente de leitura (deadline 10 s, saída até 3 MB). Fotos não
passam por eventos da página; seguem direto ao relay autenticado. Permissão nativa
fica em memória e é exclusiva com gravação/execução. Windows/Linux não oferecem
Olhar nesta fatia. A nova web exige a bridge `maquina_contexto` e relay atualizado.

Validar a release assinada em CapCut e Finder com as permissões reais do dn.os.
O teste sintético de visão na VPS não substitui esse teste no computador.

### Correção 0.7.14 · menu Olhar não aparecia

A 0.7.13 registrava o comando nativo, mas não concedia a capability exigida pelo
Tauri para a origem remota `https://dnos.dnia.ai`. A consulta de estado era
recusada antes de chegar ao handler, e o frontend escondia o item ao receber erro.
A 0.7.14 concede somente esse comando às janelas main/barra-mac. O handler
continua restringindo autorização ao chat e Parar à barra.

Regressão em `src-tauri/src/olhar_acl.rs`: usa o roteador IPC do Tauri com o
contexto real de produção; verifica acesso pelo chat e barra e recusa outra
origem/janela. Esse teste falha na 0.7.13 e passa com a capability. Não captura
a tela nem concede uma sessão no computador.

Para corrigir o menu já publicado, basta atualizar o Desktop para 0.7.14; não
precisa repetir o deploy da executor-bridge.

### Correção 0.7.15 · avatar da barra flutuante

Foto e iniciais respeitam o atributo `hidden`. O `display` explícito dos dois
elementos mantinha ambos no layout, empurrando a foto abaixo da barra e cortando
o avatar. Conferido visualmente com e sem foto nos modos olhando, assistindo e
atuando. É uma correção da casca: não exige Publish da web, edge ou migration.


### Aprenda comigo integrado · 0.7.16

O destino “No meu computador” do Aprenda comigo passa a transmitir os passos e
quadros da demonstração para a página assim que são capturados. A página usa a
função de visão já existente, guarda as interpretações com a gravação e junta
as imagens às ações e explicações para gerar o rascunho da habilidade. Não
precisa pedir no chat para o agente assistir. A web retira o item separado
“Olhar meu computador”.

- Flag `aprendaVisao` (somente macOS); eventos `gravador/passo` e
  `gravador/visao` correlacionados pelo id da gravação.
- Quadros nos passos e observação da janela ativa a cada 5 s quando muda;
  até 80 imagens. Ações e falas continuam após o limite; a revisão informa a
  cobertura parcial. Observações não são ações do roteiro.
- Finalizar encerra a captura; interpretações já pedidas podem terminar na
  revisão. Não há exploração ou edição autônoma nesta entrega.
- Áudio inclui o id da gravação. O último trecho de fala é enviado ao
  finalizar; transcrições tardias atualizam apenas a gravação de origem,
  inclusive se uma nova sessão começou. Não recriam gravações apagadas.
- Foto/iniciais corrigidas na 0.7.15; botão Finalizar no modo assistindo e
  indicação do microfone acompanha a detecção real de fala.

Validação: helper universal compilado, dez testes Rust e teste web do fluxo
completo com ponte simulada (inclui fala tardia). Teste assinado com uma
nova demonstração real do CapCut precisa ser feito após instalar esta versão
**e publicar o frontend correspondente**. Nenhuma edge ou migration nova.


## Exploração e rodadas · 0.7.17

No macOS, Aprenda comigo → No meu computador oferece Explore e aprenda.
A sessão escolhe app aberto, agente, rodada e duração (5/10/15 min). Modo
acompanhado aprova cada ação; autônomo permite cliques/rolagens/arrastos comuns,
com aprovação para texto/teclas e ações sensíveis. Até 40 ações/80 capturas.
Somente apps autorizados; terminais, cofres e Ajustes ficam fora. Botão Parar,
queda/troca de conexão e expiração revogam a sessão. Cada ação consome uma
observação e confere a impressão da janela antes de executar, seguida de nova
captura. Não se trata de sandbox semântico do software nem de edição validada
em todos os apps. Pausar animações pode ser necessário para uma imagem estável.

O protocolo `explorar-v1` usa a conexão autenticada existente. O comando IPC
`explorar_computador` é restrito à origem da instância; barra só consulta,
aprova/recusa a ação pelo id exato e para. Resultados aguardam consulta, nunca
reexecução automática. Ajuda Swift recebe entrada JSON limitada, sem shell
ou caminhos escolhidos pelo modelo. Cancelar libera entradas pressionadas.

O histórico e aprovação ficam na web/banco: migration de rodadas, deploy de
skill-manage/executor-bridge, relay exploracao.mjs e Publish são necessários.
A release isoladamente não habilita toda a função. Teste real no CapCut após
instalação/deploy ainda é necessário; testes locais usam transporte simulado.
