//! Volta do login OAuth dos conectores MCP (0.8.17, 13/10).
//!
//! O login de um MCP (Higgsfield, por exemplo) é feito pelo OpenClaw na VPS, mas o provedor manda o navegador da
//! pessoa de volta para `http://127.0.0.1:8989/oauth/callback?code=…` — o computador DELA, onde nada escuta, e a
//! página dava erro; a pessoa copiava o endereço e colava no dn.os. Agora, enquanto a tela de conexão espera, a casca
//! escuta essa porta, mostra "pronto, pode voltar", traz a janela do dn.os para a frente e entrega o código à página
//! (evento `dnos://oauth/codigo`). Escuta só 127.0.0.1, só esse caminho, um código por vez, e para em 10 minutos.

use serde::Serialize;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

static ESCUTANDO: AtomicBool = AtomicBool::new(false);
const PORTA: u16 = 8989;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Retorno {
    pub code: Option<String>,
    pub state: Option<String>,
    pub erro: Option<String>,
}

/// "GET /oauth/callback?code=a&state=b HTTP/1.1" → Some(Retorno); outro caminho → None.
pub fn ler_pedido(linha: &str) -> Option<Retorno> {
    let mut partes = linha.split_whitespace();
    if partes.next()? != "GET" { return None; }
    let alvo = partes.next()?;
    let (caminho, consulta) = alvo.split_once('?').unwrap_or((alvo, ""));
    if caminho != "/oauth/callback" { return None; }
    let mut r = Retorno { code: None, state: None, erro: None };
    for par in consulta.split('&') {
        let (k, v) = par.split_once('=').unwrap_or((par, ""));
        let v = percent_encoding::percent_decode_str(&v.replace('+', " ")).decode_utf8_lossy().to_string();
        match k {
            "code" if !v.is_empty() => r.code = Some(v),
            "state" => r.state = Some(v),
            "error" => r.erro = Some(v),
            "error_description" if !v.is_empty() => r.erro = Some(v),
            _ => {}
        }
    }
    if r.code.is_none() && r.erro.is_none() { r.erro = Some("o provedor voltou sem o código".into()); }
    Some(r)
}

fn pagina(ok: bool) -> String {
    let (titulo, texto) = if ok {
        ("Pronto!", "O login foi concluído. Pode voltar ao dn.os, ele já está terminando a conexão.")
    } else {
        ("Não deu certo", "O login não foi concluído. Volte ao dn.os e tente de novo.")
    };
    format!("<!doctype html><meta charset=utf-8><meta name=viewport content='width=device-width'><title>dn.os</title>\
<body style=\"margin:0;min-height:100vh;display:grid;place-items:center;background:#0A0A0A;color:#f4f4f5;font:16px/1.5 Inter,system-ui,sans-serif\">\
<div style=\"text-align:center;max-width:420px;padding:24px\"><div style=\"font:600 28px Rajdhani,system-ui,sans-serif;margin-bottom:8px\">{titulo}</div>\
<p style=\"color:#a1a1aa;margin:0\">{texto}</p><p style=\"color:#52525b;font-size:12px;margin-top:24px\">dn.ia</p></div></body>")
}

/// Começa a escutar a volta do login (se já estiver escutando, só confirma). Erro "porta_ocupada" = a página
/// mostra o campo para colar o endereço, como antes.
#[tauri::command]
pub fn oauth_ouvir(app: AppHandle, window: tauri::WebviewWindow) -> Result<(), String> {
    if window.label() != "main" { return Err("janela_nao_autorizada".into()); }
    if ESCUTANDO.swap(true, Ordering::SeqCst) { return Ok(()); }
    let ouvinte = match TcpListener::bind(("127.0.0.1", PORTA)) {
        Ok(l) => l,
        Err(_) => { ESCUTANDO.store(false, Ordering::SeqCst); return Err("porta_ocupada".into()); }
    };
    let _ = ouvinte.set_nonblocking(true);
    std::thread::spawn(move || {
        let fim = Instant::now() + Duration::from_secs(600);
        while Instant::now() < fim {
            match ouvinte.accept() {
                Ok((mut s, _)) => {
                    let _ = s.set_nonblocking(false);
                    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
                    let mut linha = String::new();
                    let _ = BufReader::new(match s.try_clone() { Ok(c) => c, Err(_) => continue }).read_line(&mut linha);
                    match ler_pedido(&linha) {
                        Some(r) => {
                            let corpo = pagina(r.code.is_some());
                            let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", corpo.len(), corpo);
                            let _ = app.emit("dnos://oauth/codigo", r);
                            if let Some(w) = app.get_webview_window("main") { let _ = w.show(); let _ = w.unminimize(); let _ = w.set_focus(); }
                            break;
                        }
                        None => { let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"); }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(150)),
                Err(_) => std::thread::sleep(Duration::from_millis(300)),
            }
        }
        ESCUTANDO.store(false, Ordering::SeqCst);
    });
    Ok(())
}

#[cfg(test)]
mod testes {
    use super::*;
    #[test]
    fn le_o_codigo_da_volta() {
        let r = ler_pedido("GET /oauth/callback?code=abc%2F123&state=xyz HTTP/1.1").unwrap();
        assert_eq!(r.code.as_deref(), Some("abc/123"));
        assert_eq!(r.state.as_deref(), Some("xyz"));
        assert_eq!(r.erro, None);
    }
    #[test]
    fn erro_do_provedor() {
        let r = ler_pedido("GET /oauth/callback?error=access_denied&error_description=Usu%C3%A1rio+negou HTTP/1.1").unwrap();
        assert_eq!(r.code, None);
        assert_eq!(r.erro.as_deref(), Some("Usuário negou"));
    }
    #[test]
    fn ignora_outros_caminhos() {
        assert!(ler_pedido("GET /favicon.ico HTTP/1.1").is_none());
        assert!(ler_pedido("POST /oauth/callback?code=a HTTP/1.1").is_none());
        assert!(ler_pedido("").is_none());
        assert_eq!(ler_pedido("GET /oauth/callback HTTP/1.1").unwrap().erro.as_deref(), Some("o provedor voltou sem o código"));
    }
}
