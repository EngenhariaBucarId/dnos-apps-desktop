# Ajudante do computador no Windows

Faz no Windows o que o `olhar-mac` (explorar) e o `gravador-mac executar` (roteiro)
fazem no Mac, com a **mesma linha de comando e o mesmo JSON**. A casca, o relay, a
skill `olhar-computador` e a página não sabem a diferença.

| Comando | O que faz |
|---|---|
| `--permissoes` | `{acessibilidade, tela, sistema:"windows"}`. O Windows não pede permissão a apps de área de trabalho. |
| `--apps` | Apps com janela visível (um por executável), sem terminais, cofres de senha e shell. |
| `--explorar <exe>` | Traz o app para a frente e devolve foto da **tela inteira** (JPEG ≤1600 px) + árvore de controles. |
| `--acao` (stdin) | Uma ação (`clique`, `passar`, `arrastar`, `rolar`, `texto`, `tecla`, `menu`) e, depois, a observação. |
| `--soltar` | Solta botão do mouse e modificadores presos. |
| `executar --roteiro <arq> [--ignorar <id>]` | Roteiro v2, uma linha JSON por passo; "parar" no stdin ou Esc duas vezes. |

O "bundle" do Mac aqui é o **nome do executável** (`capcut.exe`).

## Como funciona

- **Ver:** `BitBlt` do desktop (GDI) do monitor onde está a janela; JPEG por `image`.
- **Ler controles:** UI Automation (`IUIAutomation`), em largura: 150 controles, 8 níveis, prazo de 2 s.
  Campo de senha entra sem texto e a foto é omitida.
- **Agir:** `SendInput` (mouse e teclado de verdade) com a cadência do Mac. Cada ponto é conferido:
  tem de ser do app autorizado e não pode ser campo de senha.
- **Menu:** procura a barra de menus do app por UI Automation e aciona por nome (`Arquivo › Exportar`).
  Apps com menu próprio (Electron, hambúrguer) devolvem `menu_indisponivel`; o agente usa atalho ou clique medido.
- **Atalhos:** `cmd` é tratado como `Ctrl` (convenção dos apps multiplataforma). Bloqueados: Alt+F4, Alt+Tab,
  Ctrl+Alt+Del, Ctrl+Shift+Esc, e os do Mac (cmd+q/tab/espaço/h).

## Fora desta versão

Apps da Loja (UWP, hospedados por `ApplicationFrameHost`), programas rodando como administrador (o Windows
bloqueia entrada vinda de um processo comum) e a **gravação por demonstração** da máquina toda (só Mac).

## Testar

Lógica pura (roda em qualquer sistema): `cargo test`.

No Windows, compilar e rodar a fumaça com o Bloco de Notas:

```
cargo build --release
python teste-fumaca.py
```

Checar a compilação para Windows a partir de um Mac: `rustup target add x86_64-pc-windows-msvc` e
`cargo check --target x86_64-pc-windows-msvc`. O CI (`verificar.yml`, job `windows`) faz tudo isso.
