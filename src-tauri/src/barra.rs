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
      .ag{display:flex;align-items:center;gap:12px;overflow:hidden;max-width:50%}
      .a{display:inline-flex;align-items:center;gap:6px;color:#8fb0ff;font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;min-width:0}
      .a img,.a i{width:22px;height:22px;border-radius:50%;flex:none;object-fit:cover;box-shadow:0 0 0 1.5px rgba(61,97,255,.7),0 0 10px rgba(61,97,255,.35)}
      .a i{display:inline-flex;align-items:center;justify-content:center;font:700 11px -apple-system,Inter,sans-serif;font-style:normal;color:#fff;background:#3D61FF}
      button{all:initial;font:600 12px -apple-system,Inter,Segoe UI,sans-serif;color:#fff;background:#E41A11;border-radius:6px;padding:5px 10px;cursor:pointer} button:hover{filter:brightness(1.1)}
      .marca{font:700 12px -apple-system,Inter,sans-serif;letter-spacing:.02em;opacity:.7}
    </style><div class="b"><span class="dot"></span><span class="marca">dn.os</span><span class="t"></span><span class="s"></span><span class="n"></span><span class="ag"></span><span class="mic" hidden>🎙</span><button hidden>Parar</button></div>`;
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
    // Agentes agindo: foto (data URI) ou a inicial num círculo, e "Nome: o que faz".
    const ag = raiz.querySelector(".ag"); ag.textContent = "";
    for (const a of (Array.isArray(e.agentes) ? e.agentes : [])) {
      const s = document.createElement("span"); s.className = "a";
      if (a.foto) { const img = document.createElement("img"); img.src = a.foto; img.alt = ""; s.appendChild(img); }
      else { const i = document.createElement("i"); i.textContent = (a.nome || "?").slice(0, 1).toUpperCase(); s.appendChild(i); }
      s.appendChild(document.createTextNode(a.texto ? a.nome + ": " + a.texto : a.nome + " está usando este Chrome"));
      ag.appendChild(s);
    }
  };
  if (window.__dnosBarraEstado) window.__dnosBarra(window.__dnosBarraEstado);
})();
"#;

pub struct Barra {
    /// Canal para o laço do CDP: Value = estado da barra (ou null para esconder).
    tx: Option<mpsc::UnboundedSender<Value>>,
    /// Último estado, reaplicado em aba nova.
    atual: Value,
    /// Estado da gravação (modo grav), se houver — a barra combina com os agentes.
    gravacao: Value,
    /// Agentes agindo neste Chrome agora: nome -> o que está fazendo.
    agentes: std::collections::BTreeMap<String, String>,
    /// Fotos mandadas pela página do dn.os (nome em minúsculas -> data URI), para
    /// agentes que a casca não traz embutidos.
    fotos: std::collections::HashMap<String, String>,
}

/// Os 8 agentes do time vêm embutidos (56 px, círculo) — a barra mostra a foto
/// mesmo sem a página do dn.os ter mandado nada.
const FOTOS_EMBUTIDAS: [(&str, &[u8]); 8] = [
    ("lia", include_bytes!("../agentes/lia.png")),
    ("milo", include_bytes!("../agentes/milo.png")),
    ("kira", include_bytes!("../agentes/kira.png")),
    ("malu", include_bytes!("../agentes/malu.png")),
    ("radar", include_bytes!("../agentes/radar.png")),
    ("rock", include_bytes!("../agentes/rock.png")),
    ("sigma", include_bytes!("../agentes/sigma.png")),
    ("koringa", include_bytes!("../agentes/koringa.png")),
];

/// Chave da foto: primeira palavra do nome, minúscula, sem acento ("Malu (CS)" -> "malu").
fn chave_do_nome(nome: &str) -> String {
    let primeira = nome.split_whitespace().next().unwrap_or("");
    primeira.chars().filter(|c| c.is_alphanumeric()).map(|c| match c.to_lowercase().next().unwrap_or(c) {
        'á' | 'à' | 'â' | 'ã' => 'a', 'é' | 'ê' => 'e', 'í' => 'i', 'ó' | 'ô' | 'õ' => 'o', 'ú' => 'u', 'ç' => 'c', o => o,
    }).collect()
}

fn foto_de(g: &Barra, nome: &str) -> Option<String> {
    use base64::Engine;
    let chave = chave_do_nome(nome);
    if let Some(f) = g.fotos.get(&chave) { return Some(f.clone()); }
    FOTOS_EMBUTIDAS.iter().find(|(n, _)| *n == chave)
        .map(|(_, bytes)| format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

/// Recompõe o estado da barra: gravação (se houver) + agentes ativos (com nome e foto).
fn recompor(g: &mut Barra) -> Value {
    let lista: Vec<Value> = g.agentes.iter().map(|(n, t)| json!({ "nome": n, "texto": t, "foto": foto_de(g, n) })).collect();
    let mut e = if g.gravacao.is_object() { g.gravacao.clone() } else if !lista.is_empty() {
        json!({ "modo": "agente", "titulo": if g.agentes.len() == 1 { format!("{} está usando seu Chrome", g.agentes.keys().next().unwrap()) } else { format!("{} agentes usando seu Chrome", g.agentes.len()) } })
    } else { Value::Null };
    if e.is_object() && !lista.is_empty() { e["agentes"] = json!(lista); }
    g.atual = e.clone();
    e
}

/// A página do dn.os manda as fotos dos agentes (`dnos://barra/fotos`
/// `{ "milo": "data:image/png;base64,…", … }`), para cobrir agentes novos.
pub fn fotos(app: &AppHandle, mapa: Value) {
    if let Some(b) = app.try_state::<Compartilhado>() {
        if let Ok(mut g) = b.lock() {
            if let Some(o) = mapa.as_object() {
                for (n, v) in o { if let Some(s) = v.as_str() { if s.starts_with("data:image/") && s.len() < 200_000 { g.fotos.insert(chave_do_nome(n), s.to_string()); } } }
            }
            if !g.agentes.is_empty() { let e = recompor(&mut g); if let Some(tx) = g.tx.as_ref() { let _ = tx.send(e); } }
        }
    }
}

/// Um agente começou/continuou (`fim=false`) ou terminou (`fim=true`) de agir neste Chrome.
pub fn agente(app: &AppHandle, nome: &str, texto: &str, fim: bool) {
    if let Some(b) = app.try_state::<Compartilhado>() {
        if let Ok(mut g) = b.lock() {
            if fim { g.agentes.remove(nome); } else { g.agentes.insert(nome.to_string(), texto.to_string()); }
            let e = recompor(&mut g);
            if let Some(tx) = g.tx.as_ref() { let _ = tx.send(e); }
        }
    }
}
pub type Compartilhado = Arc<Mutex<Barra>>;

pub fn instalar(app: &AppHandle) {
    let estado: Compartilhado = Arc::new(Mutex::new(Barra { tx: None, atual: Value::Null, gravacao: Value::Null, agentes: Default::default(), fotos: Default::default() }));
    app.manage(estado);
    let h2 = app.clone();
    app.listen_any("dnos://barra/fotos", move |evento| {
        let v: Value = serde_json::from_str(evento.payload()).unwrap_or(json!({}));
        fotos(&h2, v);
    });
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
            // Gravação manda o estado inteiro (modo grav) ou null ao terminar; os
            // agentes continuam aparecendo por cima/depois, com nome.
            if estado["modo"] == "grav" || estado.is_null() { g.gravacao = estado; }
            else { g.atual = estado.clone(); if let Some(tx) = g.tx.as_ref() { let _ = tx.send(estado); } return; }
            let e = recompor(&mut g);
            if let Some(tx) = g.tx.as_ref() { let _ = tx.send(e); }
        }
    }
}
pub fn esconder(app: &AppHandle) { mostrar(app, Value::Null); }

/// Atualiza só alguns campos do estado atual (ex.: `ouvindo`), sem apagar o resto.
pub fn mesclar(app: &AppHandle, campos: Value) {
    if let Some(b) = app.try_state::<Compartilhado>() {
        if let Ok(mut g) = b.lock() {
            if !g.gravacao.is_object() { return; }
            if let (Some(dest), Some(src)) = (g.gravacao.as_object_mut(), campos.as_object()) {
                for (k, v) in src { dest.insert(k.clone(), v.clone()); }
            }
            let e = recompor(&mut g);
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
        if let Ok(mut g) = b.lock() { g.tx = None; g.atual = Value::Null; g.gravacao = Value::Null; g.agentes.clear(); }
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
                        // Popup aberto por window.open fica PAUSADO enquanto houver cliente com
                        // auto-attach — mesmo com waitForDebuggerOnStart:false (medido 06/09: o
                        // design do Canva abria em branco). Isto solta a aba; é inofensivo quando
                        // ela não está esperando.
                        mandar("Runtime.runIfWaitingForDebugger", json!({}), Some(sid));
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
