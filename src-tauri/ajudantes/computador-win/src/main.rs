//! dn.os Desktop · ajudante do computador no Windows.
//!
//! Faz no Windows o que `olhar-mac` (explorar) e `gravador-mac executar`
//! (roteiro) fazem no Mac, com a MESMA linha de comando e o MESMO JSON de
//! saída — a casca, o relay, a skill e a página não sabem a diferença:
//!
//!   dnos-computador-win --permissoes
//!   dnos-computador-win --apps
//!   dnos-computador-win --soltar
//!   dnos-computador-win --explorar <executavel>          (observação)
//!   dnos-computador-win --acao   < {passo…}              (ação + observação)
//!   dnos-computador-win executar --roteiro <arq> [--ignorar <id>]
//!
//! O "bundle" do Mac aqui é o nome do executável (`capcut.exe`). Sem
//! permissões a pedir: o Windows não tem o modelo de Acessibilidade/Tela do
//! macOS. Somente para apps de janela comuns (Win32, WPF, WinForms, Electron,
//! Chromium); apps da Loja (UWP) e programas rodando como administrador ficam
//! de fora nesta versão.
#![cfg_attr(not(windows), allow(dead_code))]

mod logica;

#[cfg(windows)]
mod win;

#[cfg(windows)]
fn main() {
    win::principal();
}

#[cfg(not(windows))]
fn main() {
    println!("{}", serde_json::json!({ "ok": false, "motivo": "sistema_nao_suportado", "detalhe": "este ajudante só roda no Windows" }));
    std::process::exit(1);
}
