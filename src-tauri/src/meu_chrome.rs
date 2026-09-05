//! Meu Chrome — a ponte entre o Chrome da pessoa e a VPS do dn.os.
//!
//! A página pede (evento `dnos://meu-chrome/ligar` com `{endereco, token}`);
//! a casca conecta em `wss://tela.<domínio>/node`, autentica com o JWT do
//! Supabase e recebe `{t:"pronto", porta, perfil}`. Aí abre um Chrome com
//! perfil próprio ("dn.os", não o pessoal) e depuração remota nessa porta.
//! Cada fluxo que a VPS abre vira uma conexão TCP local no Chrome; os bytes
//! vão e voltam dentro do WebSocket. A VPS enxerga o Chrome da pessoa em
//! `127.0.0.1:<porta>` e o OpenClaw usa o perfil de sempre — sem túnel SSH.
//!
//! Quadro binário: `[tipo u8][id u32 BE][dados]`, tipo 1=abrir 2=dados 3=fechar.
//! Fechar o Chrome, `dnos://meu-chrome/parar` ou a queda do WebSocket desligam
//! tudo. Estado vai para a página em `dnos://meu-chrome` como
//! `{estado: "ligando"|"ligado"|"desligado"|"erro", porta?, perfil?, motivo?}`.

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Listener, Manager};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

#[derive(Deserialize)]
struct PedidoLigar {
    endereco: String,
    token: String,
}

#[derive(Serialize, Clone)]
pub struct Estado {
    pub estado: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub porta: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub perfil: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub motivo: Option<String>,
}

#[derive(Default)]
pub struct MeuChrome {
    ligado: Option<Sessao>,
}

struct Sessao {
    porta: u16,
    perfil: String,
    cancelar: tokio::sync::watch::Sender<bool>,
    chrome: Arc<Mutex<Option<Child>>>,
}

pub type Compartilhado = Arc<Mutex<MeuChrome>>;

pub fn instalar(app: &AppHandle) {
    let estado: Compartilhado = Arc::new(Mutex::new(MeuChrome::default()));
    app.manage(estado.clone());

    let h = app.clone();
    let e = estado.clone();
    app.listen_any("dnos://meu-chrome/ligar", move |evento| {
        let pedido: Option<PedidoLigar> = serde_json::from_str(evento.payload()).ok();
        match pedido {
            Some(p) if !p.endereco.is_empty() && !p.token.is_empty() => {
                let h2 = h.clone();
                let e2 = e.clone();
                tauri::async_runtime::spawn(async move { ligar(h2, e2, p).await });
            }
            _ => emitir(&h, Estado { estado: "erro", porta: None, perfil: None, motivo: Some("pedido sem endereço ou token".into()) }),
        }
    });

    let h = app.clone();
    let e = estado.clone();
    app.listen_any("dnos://meu-chrome/parar", move |_| {
        parar(&h, &e, "você desligou");
    });

    // A página recém-carregada pergunta o estado atual.
    let h = app.clone();
    let e = estado;
    app.listen_any("dnos://meu-chrome/estado", move |_| {
        let atual = e.lock().ok().and_then(|m| m.ligado.as_ref().map(|s| (s.porta, s.perfil.clone())));
        match atual {
            Some((porta, perfil)) => emitir(&h, Estado { estado: "ligado", porta: Some(porta), perfil: Some(perfil), motivo: None }),
            None => emitir(&h, Estado { estado: "desligado", porta: None, perfil: None, motivo: None }),
        }
    });
}

fn emitir(app: &AppHandle, e: Estado) {
    let _ = app.emit("dnos://meu-chrome", e);
}

pub fn parar(app: &AppHandle, estado: &Compartilhado, motivo: &str) {
    let sessao = estado.lock().ok().and_then(|mut m| m.ligado.take());
    if let Some(s) = sessao {
        let _ = s.cancelar.send(true);
        if let Ok(mut c) = s.chrome.lock() {
            if let Some(ch) = c.as_mut() {
                let _ = ch.kill();
                let _ = ch.wait();
            }
        }
        emitir(app, Estado { estado: "desligado", porta: None, perfil: None, motivo: Some(motivo.into()) });
    }
}

/// Binário do Chrome (ou parente) na máquina, na ordem em que a pessoa mais
/// provavelmente o tem. Chromium-based basta: o OpenClaw fala CDP.
fn binario_do_chrome() -> Option<std::path::PathBuf> {
    let candidatos: Vec<std::path::PathBuf> = if cfg!(target_os = "macos") {
        vec![
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".into(),
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge".into(),
            "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser".into(),
            "/Applications/Chromium.app/Contents/MacOS/Chromium".into(),
        ]
    } else if cfg!(target_os = "windows") {
        let mut v = Vec::new();
        for base in ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"] {
            if let Ok(b) = std::env::var(base) {
                v.push(std::path::PathBuf::from(&b).join("Google/Chrome/Application/chrome.exe"));
                v.push(std::path::PathBuf::from(&b).join("Microsoft/Edge/Application/msedge.exe"));
                v.push(std::path::PathBuf::from(&b).join("BraveSoftware/Brave-Browser/Application/brave.exe"));
            }
        }
        v
    } else {
        vec!["/usr/bin/google-chrome".into(), "/usr/bin/google-chrome-stable".into(), "/usr/bin/chromium".into(), "/usr/bin/chromium-browser".into(), "/usr/bin/microsoft-edge".into()]
    };
    candidatos.into_iter().find(|p| p.exists())
}

fn abrir_chrome(app: &AppHandle, porta: u16) -> Result<Child, String> {
    let bin = binario_do_chrome().ok_or("não achei o Google Chrome (nem Edge/Brave) neste computador")?;
    let dados = app.path().app_data_dir().map_err(|e| e.to_string())?.join("chrome-dnos");
    std::fs::create_dir_all(&dados).map_err(|e| e.to_string())?;
    let instancia = std::env::var("DNOS_URL").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "https://dnos.dnia.ai".into());
    Command::new(bin)
        .arg(format!("--remote-debugging-port={porta}"))
        .arg(format!("--user-data-dir={}", dados.display()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--new-window")
        .arg(format!("{instancia}/meu-chrome"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("não consegui abrir o Chrome: {e}"))
}

async fn esperar_chrome(porta: u16) -> bool {
    for _ in 0..60 {
        if TcpStream::connect(("127.0.0.1", porta)).await.is_ok() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    false
}

fn quadro(tipo: u8, id: u32, dados: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(5 + dados.len());
    v.push(tipo);
    v.extend_from_slice(&id.to_be_bytes());
    v.extend_from_slice(dados);
    v
}

fn erro(app: &AppHandle, motivo: String) {
    emitir(app, Estado { estado: "erro", porta: None, perfil: None, motivo: Some(motivo) });
}

async fn ligar(app: AppHandle, estado: Compartilhado, pedido: PedidoLigar) {
    // Já ligado? Derruba e liga de novo (a página pode ter recarregado).
    parar(&app, &estado, "religando");
    emitir(&app, Estado { estado: "ligando", porta: None, perfil: None, motivo: None });

    let url = format!("{}/node", pedido.endereco.trim_end_matches('/'));
    let (ws, _) = match tokio_tungstenite::connect_async(&url).await {
        Ok(x) => x,
        Err(e) => return erro(&app, format!("não conectei na VPS ({url}): {e}")),
    };
    let (mut tx, mut rx) = ws.split();
    let auth = serde_json::json!({ "t": "auth", "token": pedido.token }).to_string();
    if tx.send(Message::Text(auth.into())).await.is_err() {
        return erro(&app, "a VPS fechou antes do login".into());
    }

    // Espera o {t:"pronto", porta, perfil}
    let (porta, perfil) = loop {
        match tokio::time::timeout(std::time::Duration::from_secs(10), rx.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => {
                let v: serde_json::Value = serde_json::from_str(&t).unwrap_or_default();
                if v["t"] == "pronto" {
                    break (v["porta"].as_u64().unwrap_or(0) as u16, v["perfil"].as_str().unwrap_or("").to_string());
                }
            }
            Ok(Some(Ok(Message::Close(f)))) => {
                let motivo = f.map(|f| f.reason.to_string()).unwrap_or_default();
                return erro(&app, if motivo.is_empty() { "a VPS recusou".into() } else { motivo });
            }
            Ok(Some(Ok(_))) => continue,
            _ => return erro(&app, "a VPS não respondeu ao login".into()),
        }
    };
    if porta == 0 {
        return erro(&app, "a VPS não informou a porta".into());
    }

    let chrome = match abrir_chrome(&app, porta) {
        Ok(c) => c,
        Err(e) => return erro(&app, e),
    };
    let chrome = Arc::new(Mutex::new(Some(chrome)));
    if !esperar_chrome(porta).await {
        if let Ok(mut c) = chrome.lock() {
            if let Some(ch) = c.as_mut() { let _ = ch.kill(); }
        }
        return erro(&app, format!("o Chrome abriu mas a porta {porta} não respondeu"));
    }

    let (cancelar, mut cancelado) = tokio::sync::watch::channel(false);
    if let Ok(mut m) = estado.lock() {
        m.ligado = Some(Sessao { porta, perfil: perfil.clone(), cancelar, chrome: chrome.clone() });
    }
    emitir(&app, Estado { estado: "ligado", porta: Some(porta), perfil: Some(perfil.clone()), motivo: None });

    // Saída única para o WebSocket: os fluxos mandam quadros por este canal.
    let (para_ws, mut da_fila) = mpsc::channel::<Vec<u8>>(1024);
    let escritor = tokio::spawn(async move {
        while let Some(q) = da_fila.recv().await {
            if tx.send(Message::Binary(q.into())).await.is_err() { break; }
        }
        let _ = tx.close().await;
    });

    let mut fluxos: HashMap<u32, mpsc::Sender<Vec<u8>>> = HashMap::new();
    let motivo: String = loop {
        tokio::select! {
            _ = cancelado.changed() => break "desligado".into(),
            m = rx.next() => match m {
                Some(Ok(Message::Binary(b))) if b.len() >= 5 => {
                    let tipo = b[0];
                    let id = u32::from_be_bytes([b[1], b[2], b[3], b[4]]);
                    match tipo {
                        1 => {
                            let (para_tcp, mut da_ws) = mpsc::channel::<Vec<u8>>(256);
                            fluxos.insert(id, para_tcp);
                            let para_ws = para_ws.clone();
                            tokio::spawn(async move {
                                let Ok(tcp) = TcpStream::connect(("127.0.0.1", porta)).await else {
                                    let _ = para_ws.send(quadro(3, id, &[])).await;
                                    return;
                                };
                                let (mut ler, mut escrever) = tcp.into_split();
                                let para_ws2 = para_ws.clone();
                                let leitor = tokio::spawn(async move {
                                    let mut buf = vec![0u8; 64 * 1024];
                                    loop {
                                        match ler.read(&mut buf).await {
                                            Ok(0) | Err(_) => break,
                                            Ok(n) => { if para_ws2.send(quadro(2, id, &buf[..n])).await.is_err() { break; } }
                                        }
                                    }
                                    let _ = para_ws2.send(quadro(3, id, &[])).await;
                                });
                                while let Some(d) = da_ws.recv().await {
                                    if d.is_empty() { break; } // fechar
                                    if escrever.write_all(&d).await.is_err() { break; }
                                }
                                let _ = escrever.shutdown().await;
                                leitor.abort();
                            });
                        }
                        2 => { if let Some(f) = fluxos.get(&id) { let _ = f.send(b[5..].to_vec()).await; } }
                        3 => { if let Some(f) = fluxos.remove(&id) { let _ = f.send(Vec::new()).await; } }
                        _ => {}
                    }
                }
                Some(Ok(Message::Close(f))) => break f.map(|f| f.reason.to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| "a VPS encerrou".into()),
                Some(Ok(_)) => {}
                Some(Err(e)) => break format!("conexão com a VPS caiu: {e}"),
                None => break "conexão com a VPS caiu".into(),
            },
            // O Chrome fechou? (a pessoa fechou a janela)
            _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {
                let saiu = chrome.lock().ok()
                    .map(|mut c| c.as_mut().map(|ch| ch.try_wait().ok().flatten().is_some()).unwrap_or(true))
                    .unwrap_or(true);
                if saiu { break "você fechou o Chrome".into(); }
            }
        }
    };
    drop(fluxos);
    drop(para_ws);
    escritor.abort();
    // Se ainda somos a sessão registrada, limpa e avisa.
    let ainda = estado.lock().ok().map(|m| m.ligado.as_ref().map(|s| s.porta == porta).unwrap_or(false)).unwrap_or(false);
    if ainda {
        parar(&app, &estado, &motivo);
    }
}
