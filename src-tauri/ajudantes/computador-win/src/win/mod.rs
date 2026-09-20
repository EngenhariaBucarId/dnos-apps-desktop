//! Camada do Windows: janelas, captura, entrada, Automação de Interface.
mod captura;
mod entrada;
mod janelas;
mod olhar;
mod roteiro;
mod uia;

use std::io::Read;

use serde_json::{json, Value};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};

fn imprimir(v: Value) -> ! {
    roteiro::linha(v);
    std::process::exit(0);
}

fn valor_de<'a>(args: &'a [String], nome: &str) -> Option<&'a str> {
    args.iter().position(|a| a == nome).and_then(|i| args.get(i + 1)).map(|s| s.as_str())
}

pub fn principal() {
    // Coordenadas em pixels físicos, iguais para captura, cursor e janelas.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let texto: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    match texto.as_slice() {
        [] | ["--permissoes"] | ["permissoes", ..] => imprimir(olhar::permissoes()),
        ["--soltar"] => imprimir(olhar::soltar()),
        ["--apps"] => imprimir(olhar::apps()),
        ["--acao"] => {
            let mut dados = Vec::new();
            let _ = std::io::stdin().take(16_385).read_to_end(&mut dados);
            let pedido = if dados.len() <= 16_384 { serde_json::from_slice::<Value>(&dados).ok() } else { None };
            let Some(pedido) = pedido else { imprimir(json!({ "ok": false, "motivo": "acao_invalida" })) };
            let Some(exe) = pedido["bundle"].as_str().filter(|b| !b.is_empty()).map(|b| b.to_string()) else { imprimir(json!({ "ok": false, "motivo": "acao_invalida" })) };
            imprimir(olhar::olhar(&exe, Some(pedido)));
        }
        ["--explorar", exe] | ["--app", exe] if !exe.is_empty() => imprimir(olhar::olhar(exe, None)),
        ["executar", ..] => {
            let caminho = valor_de(&args, "--roteiro").unwrap_or("");
            let ignorar = valor_de(&args, "--ignorar").unwrap_or("");
            std::process::exit(roteiro::executar(caminho, ignorar));
        }
        ["gravar", ..] => {
            roteiro::linha(json!({ "t": "erro", "motivo": "a gravação da máquina só existe no macOS por enquanto" }));
            std::process::exit(1);
        }
        _ => imprimir(json!({ "ok": false, "motivo": "uso: dnos-computador-win [--permissoes | --apps | --soltar | --explorar <exe> | --acao | executar --roteiro <arquivo>]" })),
    }
}
