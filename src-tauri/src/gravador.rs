//! Gravador — "Aprenda comigo", fase 3a.
//!
//! Enquanto o Meu Chrome está ligado, a casca se conecta ao mesmo Chrome pelo
//! CDP (127.0.0.1:<porta>) e observa o que a pessoa faz: cliques, digitação,
//! teclas, envios de formulário, navegação e abas. Cada passo guarda o alvo
//! *semântico* (texto do botão, rótulo do campo, papel, URL) — não coordenadas —
//! e um quadro da tela. Senhas nunca entram (campos type=password viram "••••").
//! A pessoa pode anotar durante a gravação ("aqui escolho 1080p porque…") e,
//! ao parar, dizer o critério de sucesso. O resultado é um JSON em
//! `<app_data>/gravacoes/<id>.json`, entregue à página pelo evento
//! `dnos://gravador/pronta` — dali a página manda compilar em habilidade.
//!
//! Eventos que a página emite: `dnos://gravador/iniciar {nome?}`,
//! `dnos://gravador/nota {texto}`, `dnos://gravador/parar {criterio?}`,
//! `dnos://gravador/estado`, `dnos://gravador/listar`.
//! A casca responde em `dnos://gravador` `{estado: "gravando"|"parado"|"erro", passos, id?, motivo?}`
//! e em `dnos://gravador/pronta {gravacao}` / `dnos://gravador/lista {gravacoes}`.

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Listener, Manager};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::meu_chrome;

/// Roda dentro de cada página do Chrome do dn.os. Descreve o alvo de cada ação
/// pelo que uma pessoa (e um agente) reconhece: texto, rótulo, papel, campo.
const SCRIPT_DA_PAGINA: &str = r#"
(() => {
  if (window.__dnosGravadorOk) return; window.__dnosGravadorOk = true;
  const texto = (el) => (el && (el.innerText || el.textContent || "") || "").replace(/\s+/g, " ").trim().slice(0, 80);
  const descrever = (el) => {
    if (!el || !el.tagName) return null;
    const alvo = el.closest ? (el.closest("button,a,input,select,textarea,summary,[role='button'],[role='link'],[role='tab'],[role='menuitem'],[role='option'],[role='checkbox'],[role='switch'],label") || el) : el;
    const rotulo = alvo.getAttribute && (alvo.getAttribute("aria-label") || alvo.getAttribute("title") || alvo.getAttribute("placeholder") || alvo.getAttribute("name") || "");
    let label = "";
    try { if (alvo.labels && alvo.labels[0]) label = texto(alvo.labels[0]); else if (alvo.id) { const l = document.querySelector(`label[for="${CSS.escape(alvo.id)}"]`); if (l) label = texto(l); } } catch {}
    return {
      tag: alvo.tagName.toLowerCase(),
      tipo: alvo.getAttribute ? (alvo.getAttribute("type") || "") : "",
      papel: alvo.getAttribute ? (alvo.getAttribute("role") || "") : "",
      texto: texto(alvo),
      rotulo: String(rotulo || label || "").slice(0, 80),
      id: alvo.id || "",
      href: alvo.href ? String(alvo.href).slice(0, 200) : "",
      testid: alvo.getAttribute ? (alvo.getAttribute("data-testid") || "") : "",
    };
  };
  const enviar = (o) => { try { window.__dnosGravador(JSON.stringify(Object.assign({ url: location.href, titulo: document.title }, o))); } catch {} };
  document.addEventListener("click", (e) => enviar({ t: "clique", alvo: descrever(e.target), x: e.clientX, y: e.clientY, vw: innerWidth, vh: innerHeight }), true);
  document.addEventListener("change", (e) => {
    const el = e.target; if (!el || !el.tagName) return;
    const senha = String(el.type || "").toLowerCase() === "password";
    let valor = "";
    if (el.tagName === "SELECT") { const o = el.options && el.options[el.selectedIndex]; valor = o ? texto(o) : String(el.value); }
    else if (el.type === "checkbox" || el.type === "radio") valor = el.checked ? "marcado" : "desmarcado";
    else valor = String(el.value || "");
    enviar({ t: "digitar", alvo: descrever(el), valor: senha ? "••••" : valor.slice(0, 200), senha });
  }, true);
  document.addEventListener("keydown", (e) => { if (["Enter", "Tab", "Escape"].includes(e.key)) enviar({ t: "tecla", tecla: e.key, alvo: descrever(e.target) }); }, true);
  document.addEventListener("submit", (e) => enviar({ t: "enviar", alvo: descrever(e.target) }), true);
})();
"#;

#[derive(Default)]
pub struct Gravador {
    ativa: Option<Ativa>,
}

struct Ativa {
    id: String,
    passos: usize,
    notas: mpsc::UnboundedSender<Value>,
    parar: mpsc::UnboundedSender<(Option<String>, Option<String>)>, // (critério, nome)
}

pub type Compartilhado = Arc<Mutex<Gravador>>;

fn agora_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn emitir(app: &AppHandle, estado: &str, passos: usize, id: Option<&str>, motivo: Option<String>) {
    let mut v = json!({ "estado": estado, "passos": passos });
    if let Some(i) = id { v["id"] = json!(i); }
    if let Some(m) = motivo { v["motivo"] = json!(m); }
    meu_chrome::registrar(app, &format!("gravador: {v}"));
    let _ = app.emit("dnos://gravador", v);
}

fn pasta(app: &AppHandle) -> Option<std::path::PathBuf> {
    let p = app.path().app_data_dir().ok()?.join("gravacoes");
    std::fs::create_dir_all(&p).ok()?;
    Some(p)
}

pub fn instalar(app: &AppHandle) {
    let estado: Compartilhado = Arc::new(Mutex::new(Gravador::default()));
    app.manage(estado.clone());

    let h = app.clone();
    let e = estado.clone();
    app.listen_any("dnos://gravador/iniciar", move |evento| {
        let v: Value = serde_json::from_str(evento.payload()).unwrap_or(json!({}));
        let nome = v["nome"].as_str().unwrap_or("").to_string();
        let h2 = h.clone();
        let e2 = e.clone();
        tauri::async_runtime::spawn(async move { iniciar(h2, e2, nome).await });
    });

    let e = estado.clone();
    app.listen_any("dnos://gravador/nota", move |evento| {
        let v: Value = serde_json::from_str(evento.payload()).unwrap_or(json!({}));
        if let Some(t) = v["texto"].as_str() {
            if let Ok(g) = e.lock() {
                if let Some(a) = g.ativa.as_ref() {
                    let _ = a.notas.send(json!({ "hora": agora_ms(), "texto": t, "apos_passo": a.passos }));
                }
            }
        }
    });

    let e = estado.clone();
    app.listen_any("dnos://gravador/parar", move |evento| {
        let v: Value = serde_json::from_str(evento.payload()).unwrap_or(json!({}));
        let criterio = v["criterio"].as_str().map(|s| s.to_string());
        let nome = v["nome"].as_str().filter(|s| !s.trim().is_empty()).map(|s| s.trim().to_string());
        if let Ok(g) = e.lock() {
            if let Some(a) = g.ativa.as_ref() { let _ = a.parar.send((criterio, nome)); }
        }
    });

    let h = app.clone();
    let e = estado.clone();
    app.listen_any("dnos://gravador/estado", move |_| {
        let atual = e.lock().ok().and_then(|g| g.ativa.as_ref().map(|a| (a.id.clone(), a.passos)));
        match atual {
            Some((id, n)) => emitir(&h, "gravando", n, Some(&id), None),
            None => emitir(&h, "parado", 0, None, None),
        }
    });

    let h = app.clone();
    app.listen_any("dnos://gravador/listar", move |_| {
        let mut lista = Vec::new();
        if let Some(p) = pasta(&h) {
            if let Ok(rd) = std::fs::read_dir(&p) {
                for ent in rd.flatten() {
                    if let Ok(txt) = std::fs::read_to_string(ent.path()) {
                        if let Ok(v) = serde_json::from_str::<Value>(&txt) {
                            lista.push(json!({ "id": v["id"], "nome": v["nome"], "inicio": v["inicio"], "fim": v["fim"], "passos": v["passos"].as_array().map(|a| a.len()).unwrap_or(0), "criterio": v["criterio"] }));
                        }
                    }
                }
            }
        }
        lista.sort_by(|a, b| b["inicio"].as_u64().unwrap_or(0).cmp(&a["inicio"].as_u64().unwrap_or(0)));
        let _ = h.emit("dnos://gravador/lista", json!({ "gravacoes": lista }));
    });

    // Apagar uma gravação desta máquina (só o arquivo dela; a lista volta atualizada).
    let h = app.clone();
    app.listen_any("dnos://gravador/apagar", move |evento| {
        let v: Value = serde_json::from_str(evento.payload()).unwrap_or(json!({}));
        let id = v["id"].as_str().unwrap_or("").replace(|c: char| !c.is_ascii_alphanumeric() && c != '-', "");
        if id.is_empty() { return; }
        if let Some(p) = pasta(&h) {
            let _ = std::fs::remove_file(p.join(format!("{id}.json")));
            meu_chrome::registrar(&h, &format!("gravador: apagada {id}"));
        }
        let _ = h.emit("dnos://gravador/listar-de-novo", json!({}));
    });

    // A página pede uma gravação inteira (com quadros) pelo id.
    let h = app.clone();
    app.listen_any("dnos://gravador/abrir", move |evento| {
        let v: Value = serde_json::from_str(evento.payload()).unwrap_or(json!({}));
        let id = v["id"].as_str().unwrap_or("").replace(|c: char| !c.is_ascii_alphanumeric() && c != '-', "");
        if id.is_empty() { return; }
        if let Some(p) = pasta(&h) {
            if let Ok(txt) = std::fs::read_to_string(p.join(format!("{id}.json"))) {
                if let Ok(g) = serde_json::from_str::<Value>(&txt) {
                    let _ = h.emit("dnos://gravador/pronta", json!({ "gravacao": g, "reaberta": true }));
                }
            }
        }
    });
}

/// GET http://127.0.0.1:<porta>/json/version sem dependência de HTTP: uma
/// requisição crua basta para o Chrome. Lê pelo Content-Length: o servidor
/// do DevTools NÃO fecha a conexão (ignora `Connection: close`), e esperar
/// EOF travava até o Chrome morrer — foi o "reset by peer" de 06/09.
async fn url_do_browser(porta: u16) -> Result<String, String> {
    let mut tcp = TcpStream::connect(("127.0.0.1", porta)).await.map_err(|e| format!("Chrome não respondeu na porta {porta}: {e}"))?;
    tcp.write_all(format!("GET /json/version HTTP/1.1\r\nHost: 127.0.0.1:{porta}\r\nConnection: close\r\n\r\n").as_bytes()).await.map_err(|e| e.to_string())?;
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 4096];
    let limite = std::time::Duration::from_secs(5);
    let corpo = loop {
        let n = match tokio::time::timeout(limite, tcp.read(&mut tmp)).await {
            Ok(Ok(0)) | Err(_) => 0,
            Ok(Ok(n)) => n,
            Ok(Err(e)) => return Err(format!("lendo o Chrome: {e}")),
        };
        if n > 0 { buf.extend_from_slice(&tmp[..n]); }
        let txt = String::from_utf8_lossy(&buf).to_string();
        if let Some(pos) = txt.find("\r\n\r\n") {
            let cab = &txt[..pos];
            let tamanho = cab.lines().find_map(|l| {
                let (k, v) = l.split_once(':')?;
                if k.trim().eq_ignore_ascii_case("content-length") { v.trim().parse::<usize>().ok() } else { None }
            });
            let corpo_bytes = &buf[pos + 4..];
            match tamanho {
                Some(t) if corpo_bytes.len() >= t => break String::from_utf8_lossy(&corpo_bytes[..t]).to_string(),
                Some(_) if n > 0 => continue,
                _ => break String::from_utf8_lossy(corpo_bytes).to_string(),
            }
        }
        if n == 0 { break String::new(); }
    };
    let v: Value = serde_json::from_str(corpo.trim()).map_err(|_| "resposta do Chrome sem JSON".to_string())?;
    v["webSocketDebuggerUrl"].as_str().map(|s| s.to_string()).ok_or_else(|| "Chrome sem webSocketDebuggerUrl".into())
}

async fn iniciar(app: AppHandle, estado: Compartilhado, nome: String) {
    // Precisa do Meu Chrome ligado: é nele que a pessoa demonstra.
    let porta = app.try_state::<meu_chrome::Compartilhado>().and_then(|m| m.lock().ok().and_then(|g| g.porta_ligada()));
    let Some(porta) = porta else {
        return emitir(&app, "erro", 0, None, Some("ligue o Meu Chrome antes de gravar".into()));
    };
    if estado.lock().map(|g| g.ativa.is_some()).unwrap_or(false) {
        return emitir(&app, "erro", 0, None, Some("já existe uma gravação em andamento".into()));
    }
    // Chrome recém-aberto às vezes aceita TCP antes de o DevTools estar pronto: tenta por até ~8 s.
    let mut ws = None;
    let mut ultimo_erro = String::new();
    for _ in 0..8 {
        match url_do_browser(porta).await {
            Ok(url) => match tokio_tungstenite::connect_async(&url).await {
                Ok((w, _)) => { ws = Some(w); break; }
                Err(e) => ultimo_erro = format!("não conectei ao Chrome pelo CDP: {e}"),
            },
            Err(e) => ultimo_erro = e,
        }
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
    }
    let Some(ws) = ws else { return emitir(&app, "erro", 0, None, Some(ultimo_erro)); };
    let (mut tx, mut rx) = ws.split();

    let id = format!("{}-{:x}", agora_ms(), std::process::id());
    let mut nome = if nome.trim().is_empty() { format!("Gravação de {}", chrono_curto()) } else { nome.trim().to_string() };
    let (tx_notas, mut rx_notas) = mpsc::unbounded_channel::<Value>();
    let (tx_parar, mut rx_parar) = mpsc::unbounded_channel::<(Option<String>, Option<String>)>();
    if let Ok(mut g) = estado.lock() {
        g.ativa = Some(Ativa { id: id.clone(), passos: 0, notas: tx_notas, parar: tx_parar });
    }
    emitir(&app, "gravando", 0, Some(&id), None);

    // Saída única para o CDP.
    let (para_cdp, mut fila) = mpsc::unbounded_channel::<Value>();
    let escritor = tokio::spawn(async move {
        while let Some(m) = fila.recv().await {
            if tx.send(Message::Text(m.to_string().into())).await.is_err() { break; }
        }
    });
    let mut prox_id: u64 = 1;
    let mut mandar = |metodo: &str, params: Value, sessao: Option<&str>| -> u64 {
        let id = prox_id; prox_id += 1;
        let mut m = json!({ "id": id, "method": metodo, "params": params });
        if let Some(s) = sessao { m["sessionId"] = json!(s); }
        let _ = para_cdp.send(m);
        id
    };
    mandar("Target.setAutoAttach", json!({ "autoAttach": true, "waitForDebuggerOnStart": false, "flatten": true }), None);

    let inicio = agora_ms();
    let mut passos: Vec<Value> = Vec::new();
    let mut notas: Vec<Value> = Vec::new();
    let mut ultima_url: HashMap<String, String> = HashMap::new(); // sessionId -> url
    let mut fotos_pendentes: HashMap<u64, usize> = HashMap::new(); // id da requisição -> índice do passo
    let mut ultima_foto_ms: u64 = 0;
    let mut criterio: Option<String> = None;
    let mut motivo_fim = "parado".to_string();
    let mut sessoes_iniciais: usize = 0;

    loop {
        tokio::select! {
            Some((c, n)) = rx_parar.recv() => { criterio = c; if let Some(n) = n { nome = n; } break; }
            Some(n) = rx_notas.recv() => { notas.push(n); }
            m = rx.next() => {
                let Some(Ok(Message::Text(txt))) = m else {
                    if matches!(m, Some(Ok(_))) { continue; }
                    motivo_fim = "o Chrome fechou".into(); break;
                };
                let Ok(v) = serde_json::from_str::<Value>(&txt) else { continue };
                // Resposta de foto: cola o quadro no passo.
                if let Some(rid) = v["id"].as_u64() {
                    if let Some(idx) = fotos_pendentes.remove(&rid) {
                        if let Some(dados) = v["result"]["data"].as_str() {
                            if let Some(p) = passos.get_mut(idx) { p["quadro"] = json!(format!("data:image/jpeg;base64,{dados}")); }
                        }
                    }
                    continue;
                }
                let metodo = v["method"].as_str().unwrap_or("");
                let sessao = v["sessionId"].as_str().map(|s| s.to_string());
                match metodo {
                    "Target.attachedToTarget" => {
                        let info = &v["params"]["targetInfo"];
                        if info["type"].as_str() != Some("page") { continue; }
                        let Some(sid) = v["params"]["sessionId"].as_str() else { continue };
                        mandar("Page.enable", json!({}), Some(sid));
                        mandar("Runtime.enable", json!({}), Some(sid));
                        mandar("Runtime.addBinding", json!({ "name": "__dnosGravador" }), Some(sid));
                        mandar("Page.addScriptToEvaluateOnNewDocument", json!({ "source": SCRIPT_DA_PAGINA }), Some(sid));
                        mandar("Runtime.evaluate", json!({ "expression": SCRIPT_DA_PAGINA }), Some(sid));
                        let url = info["url"].as_str().unwrap_or("").to_string();
                        ultima_url.insert(sid.to_string(), url.clone());
                        // As abas que já estavam abertas não são passos; aba nova durante a gravação é.
                        // Aba em branco (about:blank, sem URL) não vira passo: o navegar dela chega logo depois.
                        if agora_ms() - inicio < 1500 { sessoes_iniciais += 1; }
                        else if !url.is_empty() && url != "about:blank" {
                            passos.push(json!({ "n": passos.len() + 1, "t": "aba", "hora": agora_ms(), "url": url, "titulo": info["title"] }));
                        }
                    }
                    "Page.frameNavigated" => {
                        let frame = &v["params"]["frame"];
                        if frame.get("parentId").is_some() { continue; }
                        let url = frame["url"].as_str().unwrap_or("").to_string();
                        let sid = sessao.clone().unwrap_or_default();
                        if ultima_url.get(&sid).map(|u| u == &url).unwrap_or(false) { continue; }
                        ultima_url.insert(sid, url.clone());
                        if agora_ms() - inicio < 1500 { continue; }
                        if url.is_empty() || url == "about:blank" { continue; }
                        // Navegação logo depois de "aba nova" na mesma URL: é a mesma coisa, funde.
                        if let Some(ult) = passos.last_mut() {
                            if ult["t"] == "aba" && (ult["url"] == json!(url) || agora_ms() - ult["hora"].as_u64().unwrap_or(0) < 3000) {
                                ult["url"] = json!(url); continue;
                            }
                        }
                        passos.push(json!({ "n": passos.len() + 1, "t": "navegar", "hora": agora_ms(), "url": url }));
                        // Um quadro da página nova, quando a última foto já ficou para trás.
                        if let Some(sid) = sessao.as_deref() {
                            if agora_ms() - ultima_foto_ms > 1200 && passos.len() <= 80 {
                                ultima_foto_ms = agora_ms();
                                let idx = passos.len() - 1;
                                let rid = mandar("Page.captureScreenshot", json!({ "format": "jpeg", "quality": 45 }), Some(sid));
                                fotos_pendentes.insert(rid, idx);
                            }
                        }
                    }
                    "Runtime.bindingCalled" => {
                        if v["params"]["name"].as_str() != Some("__dnosGravador") { continue; }
                        let Ok(mut passo) = serde_json::from_str::<Value>(v["params"]["payload"].as_str().unwrap_or("{}")) else { continue };
                        passo["n"] = json!(passos.len() + 1);
                        passo["hora"] = json!(agora_ms());
                        passos.push(passo);
                        let idx = passos.len() - 1;
                        // Um quadro por passo, no máximo um a cada 1,2 s (o Chrome da pessoa não pode engasgar).
                        if let Some(sid) = sessao.as_deref() {
                            if agora_ms() - ultima_foto_ms > 1200 && passos.len() <= 80 {
                                ultima_foto_ms = agora_ms();
                                let rid = mandar("Page.captureScreenshot", json!({ "format": "jpeg", "quality": 45 }), Some(sid));
                                fotos_pendentes.insert(rid, idx);
                            }
                        }
                    }
                    _ => {}
                }
                if metodo == "Runtime.bindingCalled" || metodo == "Page.frameNavigated" || metodo == "Target.attachedToTarget" {
                    if let Ok(mut g) = estado.lock() { if let Some(a) = g.ativa.as_mut() { a.passos = passos.len(); } }
                    if metodo != "Target.attachedToTarget" || sessoes_iniciais == 0 { emitir(&app, "gravando", passos.len(), Some(&id), None); }
                }
            }
        }
    }
    // Dá 400 ms para a última foto chegar.
    let fim_espera = agora_ms() + 400;
    while agora_ms() < fim_espera && !fotos_pendentes.is_empty() {
        match tokio::time::timeout(std::time::Duration::from_millis(100), rx.next()).await {
            Ok(Some(Ok(Message::Text(txt)))) => {
                if let Ok(v) = serde_json::from_str::<Value>(&txt) {
                    if let Some(rid) = v["id"].as_u64() {
                        if let Some(idx) = fotos_pendentes.remove(&rid) {
                            if let Some(dados) = v["result"]["data"].as_str() {
                                if let Some(p) = passos.get_mut(idx) { p["quadro"] = json!(format!("data:image/jpeg;base64,{dados}")); }
                            }
                        }
                    }
                }
            }
            _ => break,
        }
    }
    escritor.abort();

    let gravacao = json!({
        "id": id, "nome": nome, "inicio": inicio, "fim": agora_ms(),
        "instancia": std::env::var("DNOS_URL").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "https://dnos.dnia.ai".into()),
        "criterio": criterio, "passos": passos, "notas": notas, "encerrada_por": motivo_fim,
    });
    if let Some(p) = pasta(&app) {
        let _ = std::fs::write(p.join(format!("{id}.json")), gravacao.to_string());
    }
    if let Ok(mut g) = estado.lock() { g.ativa = None; }
    let n = gravacao["passos"].as_array().map(|a| a.len()).unwrap_or(0);
    emitir(&app, "parado", n, Some(&id), Some(motivo_fim.clone()));
    let _ = app.emit("dnos://gravador/pronta", json!({ "gravacao": gravacao }));
}

/// "06/09 08:41" sem puxar a crate chrono: só para o nome padrão.
fn chrono_curto() -> String {
    let s = agora_ms() / 1000;
    let dias = s / 86400; let resto = s % 86400;
    // Dias desde 1970 → data civil (algoritmo de Howard Hinnant).
    let z = dias as i64 + 719468; let era = z.div_euclid(146097); let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1; let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:02}/{:02} {:02}:{:02}", d, m, resto / 3600, (resto % 3600) / 60)
}
