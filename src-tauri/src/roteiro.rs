//! Roteiro — executa os passos mecânicos de uma habilidade direto no Chrome
//! da pessoa, pela casca, sem passar pela VPS nem pelo modelo (fase 3a, passo 2).
//!
//! A página manda `dnos://roteiro/executar {nome, criterio, passos:[...]}`; os
//! passos são os da gravação (alvo em palavras, URL, valor, tecla). Para cada um:
//!   navegar  → Page.navigate na aba do roteiro e espera carregar
//!   clique   → acha o elemento pelo alvo (testid, id, papel+texto, rótulo, texto,
//!              href), rola até ele e clica com evento de mouse de verdade
//!   digitar  → foca o elemento e insere o texto (Input.insertText); senha nunca
//!   tecla    → Enter/Tab/Escape por Input.dispatchKeyEvent
//!   aba      → abre aba nova na URL
//! Depois de cada passo confere a tela: a URL/título do passo seguinte, ou o
//! próximo alvo presente. Passo que não bate (10 s) → para e devolve ao
//! agente com o estado da tela. Estado vai para a página em `dnos://roteiro`
//! `{estado: "rodando"|"parou"|"concluido"|"erro", passo, total, texto, motivo, tela}`
//! e para a barra do Chrome.

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Listener, Manager};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::meu_chrome;

/// Acha um elemento pelo alvo gravado e devolve o centro dele (já rolado para a vista).
const SCRIPT_ACHAR: &str = r#"
(alvo) => {
  const norm = (s) => String(s || "").replace(/\s+/g, " ").trim().toLowerCase();
  const txt = (el) => norm(el.innerText || el.textContent || "");
  const visivel = (el) => { const r = el.getBoundingClientRect(); const cs = getComputedStyle(el); return r.width > 0 && r.height > 0 && cs.visibility !== "hidden" && cs.display !== "none"; };
  const candidatos = [];
  const add = (el, peso) => { if (el && el.nodeType === 1 && visivel(el)) candidatos.push([el, peso]); };
  if (alvo.testid) document.querySelectorAll(`[data-testid="${CSS.escape(alvo.testid)}"]`).forEach((e) => add(e, 100));
  if (alvo.id) { const e = document.getElementById(alvo.id); if (e) add(e, 90); }
  const t = norm(alvo.texto), r = norm(alvo.rotulo);
  const sel = "button,a,input,select,textarea,summary,label,[role='button'],[role='link'],[role='tab'],[role='menuitem'],[role='option'],[role='checkbox'],[role='switch'],[role='textbox'],[contenteditable],div,span,li,p,h1,h2,h3,h4";
  const todos = document.querySelectorAll(sel);
  for (const el of todos) {
    const tag = el.tagName.toLowerCase();
    const papel = el.getAttribute("role") || "";
    let peso = 0;
    const et = txt(el), er = norm(el.getAttribute("aria-label") || el.getAttribute("title") || el.getAttribute("placeholder") || el.getAttribute("name") || "");
    if (t && et === t) peso = 80; else if (t && et.includes(t) && t.length >= 3 && et.length < t.length * 4) peso = 50;
    if (r && er === r) peso = Math.max(peso, 75); else if (r && er.includes(r) && r.length >= 3) peso = Math.max(peso, 45);
    if (!t && !r) continue;
    if (peso === 0) continue;
    if (alvo.papel && papel === alvo.papel) peso += 8;
    if (alvo.tag && tag === alvo.tag) peso += 5;
    if (alvo.href && el.href && String(el.href).split("?")[0] === String(alvo.href).split("?")[0]) peso += 15;
    const interativo = ["button","a","input","select","textarea","summary","label"].includes(tag) || papel;
    if (!interativo) peso -= 10;
    add(el, peso);
  }
  if (!candidatos.length && alvo.href) { const e = document.querySelector(`a[href="${CSS.escape(alvo.href)}"]`); if (e) add(e, 40); }
  if (!candidatos.length) return null;
  candidatos.sort((a, b) => b[1] - a[1]);
  const el = candidatos[0][0];
  el.scrollIntoView({ block: "center", inline: "center" });
  const b = el.getBoundingClientRect();
  return { x: b.x + b.width / 2, y: b.y + b.height / 2, peso: candidatos[0][1], desc: (el.tagName + " " + txt(el).slice(0, 40)) };
}
"#;

pub struct Roteiro { cancelar: Option<tokio::sync::watch::Sender<bool>>, rodando: bool }
pub type Compartilhado = Arc<Mutex<Roteiro>>;

pub fn instalar(app: &AppHandle) {
    app.manage::<Compartilhado>(Arc::new(Mutex::new(Roteiro { cancelar: None, rodando: false })));
    let h = app.clone();
    app.listen_any("dnos://roteiro/executar", move |evento| {
        let v: Value = serde_json::from_str(evento.payload()).unwrap_or(json!({}));
        let h2 = h.clone();
        tauri::async_runtime::spawn(async move { executar(h2, v).await });
    });
    let h = app.clone();
    app.listen_any("dnos://roteiro/parar", move |_| {
        if let Some(r) = h.try_state::<Compartilhado>() { if let Ok(g) = r.lock() { if let Some(c) = g.cancelar.as_ref() { let _ = c.send(true); } } }
    });
}

fn emitir(app: &AppHandle, v: Value) {
    meu_chrome::registrar(app, &format!("roteiro: {}", v.to_string().chars().take(220).collect::<String>()));
    let _ = app.emit("dnos://roteiro", v);
}

fn agora_ms() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0) }

struct Cdp {
    tx: mpsc::UnboundedSender<Value>,
    pend: Arc<Mutex<std::collections::HashMap<u64, tokio::sync::oneshot::Sender<Value>>>>,
    prox: Arc<Mutex<u64>>,
    sessao: String,
}
impl Cdp {
    async fn mandar(&self, metodo: &str, params: Value) -> Value {
        let id = { let mut p = self.prox.lock().unwrap(); *p += 1; *p };
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pend.lock().unwrap().insert(id, tx);
        let _ = self.tx.send(json!({ "id": id, "method": metodo, "params": params, "sessionId": self.sessao }));
        match tokio::time::timeout(std::time::Duration::from_secs(20), rx).await { Ok(Ok(v)) => v, _ => json!({ "error": { "message": "sem resposta do Chrome" } }) }
    }
    async fn avaliar(&self, expr: String) -> Value {
        let r = self.mandar("Runtime.evaluate", json!({ "expression": expr, "returnByValue": true, "awaitPromise": true })).await;
        r["result"]["result"]["value"].clone()
    }
}

fn descreve(p: &Value) -> String {
    let alvo = &p["alvo"];
    let nome = ["texto", "rotulo", "id", "tag"].iter().map(|k| alvo[k].as_str().unwrap_or("")).find(|s| !s.is_empty()).unwrap_or("");
    match p["t"].as_str().unwrap_or("") {
        "navegar" => format!("abrindo {}", p["url"].as_str().unwrap_or("")),
        "aba" => format!("abrindo aba em {}", p["url"].as_str().unwrap_or("")),
        "clique" => format!("clicando em \"{nome}\""),
        "digitar" => format!("preenchendo \"{nome}\""),
        "tecla" => format!("pressionando {}", p["tecla"].as_str().unwrap_or("")),
        "enviar" => "enviando o formulário".into(),
        t => t.to_string(),
    }
}

async fn executar(app: AppHandle, pedido: Value) {
    let Some(estado) = app.try_state::<Compartilhado>() else { return };
    if estado.lock().map(|g| g.rodando).unwrap_or(false) { return emitir(&app, json!({ "estado": "erro", "motivo": "já há um roteiro rodando" })); }
    let porta = app.try_state::<meu_chrome::Compartilhado>().and_then(|m| m.lock().ok().and_then(|g| g.porta_ligada()));
    let Some(porta) = porta else { return emitir(&app, json!({ "estado": "erro", "motivo": "ligue o Meu navegador antes" })); };
    let nome = pedido["nome"].as_str().unwrap_or("habilidade").to_string();
    let criterio = pedido["criterio"].as_str().unwrap_or("").to_string();
    let passos: Vec<Value> = pedido["passos"].as_array().cloned().unwrap_or_default().into_iter()
        .filter(|p| matches!(p["t"].as_str(), Some("navegar") | Some("aba") | Some("clique") | Some("digitar") | Some("tecla") | Some("enviar"))).collect();
    if passos.is_empty() { return emitir(&app, json!({ "estado": "erro", "motivo": "roteiro sem passos executáveis" })); }
    let total = passos.len();

    // Conexão própria ao browser; a aba do roteiro é nova, para não bagunçar o que a pessoa tem aberto.
    let url = match crate::gravador::url_do_browser(porta).await { Ok(u) => u, Err(e) => return emitir(&app, json!({ "estado": "erro", "motivo": e })) };
    let (ws, _) = match tokio_tungstenite::connect_async(&url).await { Ok(x) => x, Err(e) => return emitir(&app, json!({ "estado": "erro", "motivo": format!("CDP: {e}") })) };
    let (mut tx_ws, mut rx_ws) = ws.split();
    let (para_cdp, mut fila) = mpsc::unbounded_channel::<Value>();
    let escritor = tokio::spawn(async move { while let Some(m) = fila.recv().await { if tx_ws.send(Message::Text(m.to_string().into())).await.is_err() { break; } } });
    let pend: Arc<Mutex<std::collections::HashMap<u64, tokio::sync::oneshot::Sender<Value>>>> = Arc::new(Mutex::new(Default::default()));
    let pend2 = pend.clone();
    let leitor = tokio::spawn(async move {
        while let Some(Ok(m)) = rx_ws.next().await {
            if let Message::Text(t) = m {
                if let Ok(v) = serde_json::from_str::<Value>(&t) {
                    if let Some(id) = v["id"].as_u64() { if let Some(tx) = pend2.lock().unwrap().remove(&id) { let _ = tx.send(v); } }
                }
            }
        }
    });
    let prox = Arc::new(Mutex::new(0u64));
    let browser = Cdp { tx: para_cdp.clone(), pend: pend.clone(), prox: prox.clone(), sessao: String::new() };
    // Sem sessionId para comandos de browser: manda direto.
    let mandar_browser = |metodo: &str, params: Value| {
        let id = { let mut p = prox.lock().unwrap(); *p += 1; *p };
        let (tx, rx) = tokio::sync::oneshot::channel();
        pend.lock().unwrap().insert(id, tx);
        let _ = para_cdp.send(json!({ "id": id, "method": metodo, "params": params }));
        rx
    };
    let primeira_url = passos.iter().find_map(|p| p["url"].as_str().filter(|u| u.starts_with("http"))).unwrap_or("about:blank").to_string();
    let alvo = match mandar_browser("Target.createTarget", json!({ "url": "about:blank" })).await { Ok(v) => v, Err(_) => Value::Null };
    let Some(target_id) = alvo["result"]["targetId"].as_str().map(|s| s.to_string()) else { escritor.abort(); leitor.abort(); return emitir(&app, json!({ "estado": "erro", "motivo": "não abri a aba do roteiro" })); };
    let att = match mandar_browser("Target.attachToTarget", json!({ "targetId": target_id, "flatten": true })).await { Ok(v) => v, Err(_) => Value::Null };
    let Some(sessao) = att["result"]["sessionId"].as_str().map(|s| s.to_string()) else { escritor.abort(); leitor.abort(); return emitir(&app, json!({ "estado": "erro", "motivo": "não anexei à aba do roteiro" })); };
    let cdp = Cdp { tx: browser.tx.clone(), pend: browser.pend.clone(), prox: browser.prox.clone(), sessao };
    cdp.mandar("Page.enable", json!({})).await;
    cdp.mandar("Runtime.enable", json!({})).await;
    cdp.mandar("Runtime.runIfWaitingForDebugger", json!({})).await;
    let _ = primeira_url;

    let (cancelar, cancelado) = tokio::sync::watch::channel(false);
    if let Ok(mut g) = estado.lock() { g.cancelar = Some(cancelar); g.rodando = true; }
    let inicio = agora_ms();
    let mut resultado = json!({ "estado": "concluido" });

    for (i, p) in passos.iter().enumerate() {
        if *cancelado.borrow() { resultado = json!({ "estado": "parou", "motivo": "você parou", "passo": i + 1 }); break; }
        let texto = descreve(p);
        emitir(&app, json!({ "estado": "rodando", "passo": i + 1, "total": total, "texto": texto }));
        crate::barra::agente(&app, "dn.os", &format!("roteiro \"{nome}\" · passo {}/{total} · {texto}", i + 1), false);
        let t = p["t"].as_str().unwrap_or("");
        let mut erro: Option<String> = None;
        match t {
            "navegar" | "aba" => {
                let u = p["url"].as_str().unwrap_or("about:blank");
                cdp.mandar("Page.navigate", json!({ "url": u })).await;
                esperar_carregar(&cdp).await;
            }
            "clique" => {
                match achar(&cdp, &p["alvo"]).await {
                    Some((x, y, desc)) => {
                        meu_chrome::registrar(&app, &format!("roteiro: clique em {desc} ({x:.0},{y:.0})"));
                        for tipo in ["mouseMoved", "mousePressed", "mouseReleased"] {
                            cdp.mandar("Input.dispatchMouseEvent", json!({ "type": tipo, "x": x, "y": y, "button": "left", "clickCount": 1 })).await;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
                        esperar_carregar(&cdp).await;
                    }
                    None => erro = Some(format!("não achei \"{}\" na tela", p["alvo"]["texto"].as_str().or(p["alvo"]["rotulo"].as_str()).unwrap_or("o alvo"))),
                }
            }
            "digitar" => {
                if p["senha"].as_bool().unwrap_or(false) { erro = Some("passo com senha: preciso de você na tela".into()); }
                else {
                    match achar(&cdp, &p["alvo"]).await {
                        Some((x, y, _)) => {
                            for tipo in ["mouseMoved", "mousePressed", "mouseReleased"] { cdp.mandar("Input.dispatchMouseEvent", json!({ "type": tipo, "x": x, "y": y, "button": "left", "clickCount": 1 })).await; }
                            cdp.avaliar("(() => { const a = document.activeElement; if (a && ('value' in a)) { a.select && a.select(); } return 1; })()".into()).await;
                            cdp.mandar("Input.insertText", json!({ "text": p["valor"].as_str().unwrap_or("") })).await;
                        }
                        None => erro = Some("não achei o campo para preencher".into()),
                    }
                }
            }
            "tecla" => {
                let k = p["tecla"].as_str().unwrap_or("Enter");
                let (code, keycode) = match k { "Tab" => ("Tab", 9), "Escape" => ("Escape", 27), _ => ("Enter", 13) };
                cdp.mandar("Input.dispatchKeyEvent", json!({ "type": "keyDown", "key": k, "code": code, "windowsVirtualKeyCode": keycode, "nativeVirtualKeyCode": keycode })).await;
                cdp.mandar("Input.dispatchKeyEvent", json!({ "type": "keyUp", "key": k, "code": code, "windowsVirtualKeyCode": keycode, "nativeVirtualKeyCode": keycode })).await;
                esperar_carregar(&cdp).await;
            }
            "enviar" => {
                cdp.mandar("Input.dispatchKeyEvent", json!({ "type": "keyDown", "key": "Enter", "code": "Enter", "windowsVirtualKeyCode": 13 })).await;
                cdp.mandar("Input.dispatchKeyEvent", json!({ "type": "keyUp", "key": "Enter", "code": "Enter", "windowsVirtualKeyCode": 13 })).await;
                esperar_carregar(&cdp).await;
            }
            _ => {}
        }
        // Verificação: a tela do passo seguinte (URL/título) ou o próximo alvo presente, em até 10 s.
        if erro.is_none() {
            if let Some(prox_p) = passos.get(i + 1) {
                let ok = esperar_tela(&cdp, prox_p).await;
                if !ok { erro = Some(format!("depois de {texto}, a tela não ficou como na demonstração (esperava {})", prox_p["url"].as_str().map(|u| u.split('?').next().unwrap_or(u).to_string()).unwrap_or_else(|| descreve(prox_p)))); }
            }
        }
        if let Some(e) = erro {
            let tela = cdp.avaliar("(() => ({ url: location.href, titulo: document.title }))()".into()).await;
            resultado = json!({ "estado": "parou", "passo": i + 1, "total": total, "texto": texto, "motivo": e, "tela": tela });
            break;
        }
    }
    let tela = cdp.avaliar("(() => ({ url: location.href, titulo: document.title }))()".into()).await;
    if resultado["estado"] == "concluido" { resultado = json!({ "estado": "concluido", "total": total, "tela": tela, "criterio": criterio, "ms": agora_ms() - inicio }); }
    crate::barra::agente(&app, "dn.os", "", true);
    if let Ok(mut g) = estado.lock() { g.cancelar = None; g.rodando = false; }
    emitir(&app, resultado);
    escritor.abort(); leitor.abort();
}

async fn achar(cdp: &Cdp, alvo: &Value) -> Option<(f64, f64, String)> {
    for _ in 0..20 { // até 10 s: a página pode estar terminando de desenhar
        let v = cdp.avaliar(format!("({})({})", SCRIPT_ACHAR, alvo)).await;
        if let (Some(x), Some(y)) = (v["x"].as_f64(), v["y"].as_f64()) { return Some((x, y, v["desc"].as_str().unwrap_or("").to_string())); }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    None
}

async fn esperar_carregar(cdp: &Cdp) {
    for _ in 0..20 {
        let v = cdp.avaliar("document.readyState".into()).await;
        if v.as_str() == Some("complete") { tokio::time::sleep(std::time::Duration::from_millis(400)).await; return; }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
}

/// A tela ficou como no passo seguinte da demonstração? URL/título parecidos, ou o alvo dele já existe.
async fn esperar_tela(cdp: &Cdp, prox: &Value) -> bool {
    let url_esp = prox["url"].as_str().map(|u| u.split('?').next().unwrap_or(u).trim_end_matches('/').to_string()).unwrap_or_default();
    let t = prox["t"].as_str().unwrap_or("");
    for _ in 0..20 {
        let v = cdp.avaliar("(() => ({ url: location.href, titulo: document.title }))()".into()).await;
        let url_atual = v["url"].as_str().unwrap_or("").split('?').next().unwrap_or("").trim_end_matches('/').to_string();
        if !url_esp.is_empty() && (url_atual == url_esp || (t == "navegar" || t == "aba")) { return true; }
        if matches!(t, "clique" | "digitar" | "tecla") && prox["alvo"].is_object() {
            let a = cdp.avaliar(format!("({})({})", SCRIPT_ACHAR, prox["alvo"])).await;
            if a["x"].is_number() { return true; }
        }
        if url_esp.is_empty() { return true; }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    false
}
