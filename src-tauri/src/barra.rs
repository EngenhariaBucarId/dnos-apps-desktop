//! Barra do dn.os dentro do Chrome da pessoa.
//!
//! Enquanto o Meu Chrome está ligado, a casca mantém uma conexão CDP com esse
//! Chrome e injeta em toda página um script que sabe desenhar uma barra fixa
//! no topo. Ela mostra o que está acontecendo sem a pessoa voltar ao dn.os:
//! "● Gravando · 12 passos · fale para anotar" durante o Aprenda comigo, e
//! "Lia está clicando em Exportar" quando um agente age nesse Chrome. O botão
//! Parar da barra encerra a gravação; a revisão abre no dn.os.
//!
//! Estado: `mostrar(json)` avalia `__dnosBarra(json)` em todas as abas;
//! `esconder()` some. A página do dn.os também pode pedir por evento
//! `dnos://barra` `{mostrar: {...}}` ou `{esconder: true}`.

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Listener, Manager};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::meu_chrome;

/// Desenha e atualiza a barra (shadow DOM, para não brigar com o CSS do site).
const SCRIPT_DA_BARRA: &str = r#"
(() => {
  if (window.__dnosBarra) { if (window.__dnosBarraEstado) window.__dnosBarra(window.__dnosBarraEstado); return; }
  let host = null, raiz = null, estado = null;
  const garantir = () => {
    if (host && document.documentElement.contains(host)) return;
    host = document.createElement("div"); host.id = "__dnos-barra";
    host.style.cssText = "all:initial;position:fixed;top:0;left:0;right:0;z-index:2147483647;pointer-events:none;";
    raiz = host.attachShadow({ mode: "open" });
    raiz.innerHTML = `<style>
      .b{pointer-events:auto;font:13px/1.3 -apple-system,Inter,Segoe UI,sans-serif;color:#fff;display:flex;align-items:center;gap:10px;padding:7px 12px;background:rgba(10,10,10,.92);border-bottom:1px solid rgba(255,255,255,.12);box-shadow:0 2px 12px rgba(0,0,0,.35);backdrop-filter:blur(8px)}
      .b.grav{border-bottom-color:rgba(228,26,17,.7)} .b.agente{border-bottom-color:rgba(61,97,255,.8)}
      .dot{width:9px;height:9px;border-radius:50%;background:#E41A11;flex:none;animation:p 1.2s infinite} .agente .dot{background:#3D61FF}
      @keyframes p{0%,100%{opacity:1}50%{opacity:.35}}
      .t{font-weight:600;white-space:nowrap} .s{opacity:.75;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;flex:1;min-width:0}
      .n{opacity:.9;font-style:italic;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;max-width:38%}
      .mic{font-size:13px;opacity:.35;transition:opacity .15s} .mic.on{opacity:1;animation:p .6s infinite}
      button{all:initial;font:600 12px -apple-system,Inter,Segoe UI,sans-serif;color:#fff;background:#E41A11;border-radius:6px;padding:5px 10px;cursor:pointer} button:hover{filter:brightness(1.1)}
      .marca{font:700 12px -apple-system,Inter,sans-serif;letter-spacing:.02em;opacity:.7}
    </style><div class="b"><span class="dot"></span><span class="marca">dn.os</span><span class="t"></span><span class="s"></span><span class="n"></span><span class="mic" hidden>🎙</span><button hidden>Parar</button></div>`;
    document.documentElement.appendChild(host);
    raiz.querySelector("button").addEventListener("click", () => { try { window.__dnosBarraCmd("parar"); } catch {} });
  };
  window.__dnosBarra = (e) => {
    estado = e;
    if (!e || e.esconder) { if (host) { host.remove(); host = null; } return; }
    garantir();
    const b = raiz.querySelector(".b"); b.className = "b " + (e.modo || "");
    raiz.querySelector(".t").textContent = e.titulo || "";
    raiz.querySelector(".s").textContent = e.sub || "";
    raiz.querySelector(".n").textContent = e.nota ? "“" + e.nota + "”" : "";
    raiz.querySelector("button").hidden = !e.parar;
    const mic = raiz.querySelector(".mic"); mic.hidden = e.modo !== "grav"; mic.className = "mic" + (e.ouvindo ? " on" : "");
  };
  if (window.__dnosBarraEstado) window.__dnosBarra(window.__dnosBarraEstado);
})();
"#;

pub struct Barra {
    /// Canal para o laço do CDP: Value = estado da barra (ou null para esconder).
    tx: Option<mpsc::UnboundedSender<Value>>,
    /// Último estado, reaplicado em aba nova.
    atual: Value,
}
pub type Compartilhado = Arc<Mutex<Barra>>;

pub fn instalar(app: &AppHandle) {
    let estado: Compartilhado = Arc::new(Mutex::new(Barra { tx: None, atual: Value::Null }));
    app.manage(estado);
    // A página do dn.os pede diretamente (ex.: "Lia está clicando em Exportar").
    let h = app.clone();
    app.listen_any("dnos://barra", move |evento| {
        let v: Value = serde_json::from_str(evento.payload()).unwrap_or(json!({}));
        if v["esconder"].as_bool().unwrap_or(false) { esconder(&h); }
        else if v["mostrar"].is_object() { mostrar(&h, v["mostrar"].clone()); }
    });
}

pub fn mostrar(app: &AppHandle, estado: Value) {
    if let Some(b) = app.try_state::<Compartilhado>() {
        if let Ok(mut g) = b.lock() {
            g.atual = estado.clone();
            if let Some(tx) = g.tx.as_ref() { let _ = tx.send(estado); }
        }
    }
}
pub fn esconder(app: &AppHandle) { mostrar(app, Value::Null); }

/// Atualiza só alguns campos do estado atual (ex.: `ouvindo`), sem apagar o resto.
pub fn mesclar(app: &AppHandle, campos: Value) {
    if let Some(b) = app.try_state::<Compartilhado>() {
        if let Ok(mut g) = b.lock() {
            if !g.atual.is_object() { return; }
            if let (Some(dest), Some(src)) = (g.atual.as_object_mut(), campos.as_object()) {
                for (k, v) in src { dest.insert(k.clone(), v.clone()); }
            }
            let e = g.atual.clone();
            if let Some(tx) = g.tx.as_ref() { let _ = tx.send(e); }
        }
    }
}

/// Liga a conexão com o Chrome (chamado pelo Meu Chrome quando o Chrome responde).
pub fn ligar(app: &AppHandle, porta: u16) {
    let (tx, rx) = mpsc::unbounded_channel::<Value>();
    if let Some(b) = app.try_state::<Compartilhado>() {
        if let Ok(mut g) = b.lock() { g.tx = Some(tx); }
    }
    let h = app.clone();
    tauri::async_runtime::spawn(async move { laco(h, porta, rx).await });
}

/// Desliga (o Meu Chrome fechou): o laço termina sozinho quando o canal morre.
pub fn desligar(app: &AppHandle) {
    if let Some(b) = app.try_state::<Compartilhado>() {
        if let Ok(mut g) = b.lock() { g.tx = None; g.atual = Value::Null; }
    }
}

async fn laco(app: AppHandle, porta: u16, mut rx: mpsc::UnboundedReceiver<Value>) {
    // O Chrome pode demorar a aceitar CDP logo depois de abrir.
    let mut ws = None;
    for _ in 0..10 {
        if let Ok(url) = crate::gravador::url_do_browser(porta).await {
            if let Ok((w, _)) = tokio_tungstenite::connect_async(&url).await { ws = Some(w); break; }
        }
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    }
    let Some(ws) = ws else { meu_chrome::registrar(&app, "barra: não conectei ao Chrome"); return; };
    let (mut tx_ws, mut rx_ws) = ws.split();
    let (para_cdp, mut fila) = mpsc::unbounded_channel::<Value>();
    let escritor = tokio::spawn(async move {
        while let Some(m) = fila.recv().await {
            if tx_ws.send(Message::Text(m.to_string().into())).await.is_err() { break; }
        }
    });
    let mut prox: u64 = 1;
    let mut mandar = |metodo: &str, params: Value, sessao: Option<&str>| -> u64 {
        let id = prox; prox += 1;
        let mut m = json!({ "id": id, "method": metodo, "params": params });
        if let Some(s) = sessao { m["sessionId"] = json!(s); }
        let _ = para_cdp.send(m);
        id
    };
    let mut scripts: std::collections::HashMap<String, String> = std::collections::HashMap::new(); // sessao -> identifier do script de novo documento
    let mut pendentes: std::collections::HashMap<u64, String> = std::collections::HashMap::new(); // id da requisicao -> sessao
    mandar("Target.setAutoAttach", json!({ "autoAttach": true, "waitForDebuggerOnStart": false, "flatten": true }), None);
    let mut sessoes: HashSet<String> = HashSet::new();
    let estado_atual = || app.try_state::<Compartilhado>().and_then(|b| b.lock().ok().map(|g| g.atual.clone())).unwrap_or(Value::Null);
    meu_chrome::registrar(&app, "barra: ligada");

    loop {
        tokio::select! {
            e = rx.recv() => {
                let Some(e) = e else { break; }; // Meu Chrome desligou
                let estado_js = if e.is_null() { "{esconder:true}".to_string() } else { e.to_string() };
                let js = format!("window.__dnosBarraEstado = {0}; if (window.__dnosBarra) window.__dnosBarra({0});", estado_js);
                // Documento novo (navegação, aba nova) nasce sem o estado: o script
                // de novo documento leva o estado junto. Troca o anterior de cada aba.
                let fonte = format!("window.__dnosBarraEstado = {estado_js};{SCRIPT_DA_BARRA}");
                for s in sessoes.iter() {
                    mandar("Runtime.evaluate", json!({ "expression": js }), Some(s));
                    if let Some(id_antigo) = scripts.get(s) { mandar("Page.removeScriptToEvaluateOnNewDocument", json!({ "identifier": id_antigo }), Some(s)); }
                    let rid = mandar("Page.addScriptToEvaluateOnNewDocument", json!({ "source": fonte }), Some(s));
                    pendentes.insert(rid, s.clone());
                }
            }
            m = rx_ws.next() => {
                let Some(Ok(Message::Text(txt))) = m else { if matches!(m, Some(Ok(_))) { continue; } break; };
                let Ok(v) = serde_json::from_str::<Value>(&txt) else { continue };
                if let Some(rid) = v["id"].as_u64() {
                    if let Some(sid) = pendentes.remove(&rid) {
                        if let Some(ident) = v["result"]["identifier"].as_str() { scripts.insert(sid, ident.to_string()); }
                    }
                    continue;
                }
                match v["method"].as_str().unwrap_or("") {
                    "Target.attachedToTarget" => {
                        if v["params"]["targetInfo"]["type"].as_str() != Some("page") { continue; }
                        let Some(sid) = v["params"]["sessionId"].as_str() else { continue };
                        sessoes.insert(sid.to_string());
                        mandar("Runtime.enable", json!({}), Some(sid));
                        mandar("Runtime.addBinding", json!({ "name": "__dnosBarraCmd" }), Some(sid));
                        mandar("Page.enable", json!({}), Some(sid));
                        let atual = estado_atual();
                        let prelude = if atual.is_null() { String::new() } else { format!("window.__dnosBarraEstado = {};", atual) };
                        let rid = mandar("Page.addScriptToEvaluateOnNewDocument", json!({ "source": format!("{prelude}{SCRIPT_DA_BARRA}") }), Some(sid));
                        pendentes.insert(rid, sid.to_string());
                        mandar("Runtime.evaluate", json!({ "expression": format!("{prelude}{SCRIPT_DA_BARRA}") }), Some(sid));
                    }
                    "Target.detachedFromTarget" => { if let Some(sid) = v["params"]["sessionId"].as_str() { sessoes.remove(sid); scripts.remove(sid); } }
                    "Runtime.bindingCalled" => {
                        if v["params"]["name"].as_str() == Some("__dnosBarraCmd") && v["params"]["payload"].as_str() == Some("parar") {
                            // Parar pela barra: a gravação encerra e a revisão abre no dn.os.
                            let _ = app.emit("dnos://gravador/parar-pela-barra", json!({}));
                            crate::gravador::parar_de_fora(&app);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    escritor.abort();
    meu_chrome::registrar(&app, "barra: desligada");
}
