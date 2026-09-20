//! O agente entra na reunião do Meet (fase 2, 20/09/2026).
//!
//! Cada agente tem um perfil de Chrome próprio, com a conta Google dele — é
//! assim que ele aparece na sala com nome e foto, em vez de um "convidado" sem
//! rosto. O perfil vive em `app_data/chrome-agentes/<agente>`; o primeiro login
//! é feito à mão pela pessoa, uma vez, e fica guardado ali.
//!
//! Quem dirige é o código, não o agente (mesma decisão da gravação): desligar
//! microfone e câmera ANTES de entrar não pode depender de alguém lembrar, e
//! cada clique pedido ao agente passaria pela VPS, custando segundos e tokens.
//!
//! O que sai daqui para a página: o estado da sala e o texto do painel de
//! legendas do Meet, cru. Juntar isso em falas é trabalho da web
//! (`src/lib/legendas-do-meet.ts`), onde dá para testar.
//!
//! Frágil por natureza: se o Google mudar o desenho da tela, o roteiro abaixo
//! deixa de achar os botões. Por isso cada passo avisa o que não achou, em vez
//! de falhar calado.
use serde_json::{json, Value};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Listener, Manager};
use tokio::net::TcpStream;

use crate::{barra, maquina, meu_chrome};

/// Porta de depuração do Chrome do agente. Fora da faixa do Meu Chrome
/// (19222+), que é o Chrome da pessoa.
const PORTA: u16 = 19340;
/// Teto de uma sessão, igual ao da gravação.
const TETO_MIN: u64 = 120;

struct Sessao {
    id: String,
    agente: String,
    parar: tokio::sync::mpsc::UnboundedSender<()>,
    _reserva: maquina::ReservaDeUso,
}

#[derive(Default)]
pub struct Meet {
    ativa: Option<Sessao>,
}
pub type Compartilhado = Arc<Mutex<Meet>>;

fn emitir(app: &AppHandle, v: Value) {
    meu_chrome::registrar(app, &format!("reuniao-meet: {v}"));
    let _ = app.emit("dnos://reuniao-meet", v);
}

pub fn ativa(app: &AppHandle) -> bool {
    app.try_state::<Compartilhado>()
        .and_then(|e| e.lock().ok().map(|g| g.ativa.is_some()))
        .unwrap_or(false)
}

/// Só letras, números e hífen: o nome do agente vira nome de pasta.
fn pasta_do_agente(nome: &str) -> String {
    let limpo: String = nome
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let limpo = limpo.trim_matches('-').to_string();
    if limpo.is_empty() { "agente".into() } else { limpo.chars().take(40).collect() }
}

fn link_de_meet(link: &str) -> Option<String> {
    let l = link.trim();
    let l = if l.starts_with("http") { l.to_string() } else { format!("https://{l}") };
    if l.starts_with("https://meet.google.com/") { Some(l) } else { None }
}

fn abrir_chrome(app: &AppHandle, agente: &str, link: &str) -> Result<Child, String> {
    let bin = meu_chrome::binario_do_chrome().ok_or("não achei o Google Chrome neste computador")?;
    let dados = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("chrome-agentes")
        .join(pasta_do_agente(agente));
    std::fs::create_dir_all(&dados).map_err(|e| e.to_string())?;
    Command::new(bin)
        .arg(format!("--remote-debugging-port={PORTA}"))
        .arg(format!("--user-data-dir={}", dados.display()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--new-window")
        .arg(link)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("não consegui abrir o Chrome do agente: {e}"))
}

fn fechar_chrome(app: &AppHandle, agente: &str) {
    let Ok(base) = app.path().app_data_dir() else { return };
    let alvo = base.join("chrome-agentes").join(pasta_do_agente(agente));
    #[cfg(not(target_os = "windows"))]
    let _ = Command::new("pkill").arg("-f").arg(format!("user-data-dir={}", alvo.display())).status();
    #[cfg(target_os = "windows")]
    let _ = Command::new("taskkill").args(["/F", "/IM", "chrome.exe"]).status();
}

async fn esperar_porta() -> bool {
    for _ in 0..120 {
        if TcpStream::connect(("127.0.0.1", PORTA)).await.is_ok() { return true; }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    false
}

/// O roteiro dentro da página do Meet. Roda a cada volta e é idempotente:
/// só clica no que ainda precisa de clique, e conta o que está vendo.
const ROTEIRO: &str = r#"
(() => {
  const botoes = () => [...document.querySelectorAll('button,[role=button]')];
  const porRotulo = (re) => botoes().find((b) => re.test(b.getAttribute('aria-label') || ''));
  const porTexto = (re) => botoes().find((b) => re.test((b.innerText || '').trim()));
  const texto = document.body.innerText || '';

  // Microfone e câmera: o Meet rotula com a AÇÃO, então "Desativar microfone"
  // significa que ele está LIGADO. Desligar antes de entrar não é opcional.
  const micLigado = !!porRotulo(/^Desativar microfone|^Turn off microphone/i);
  const camLigada = !!porRotulo(/^Desativar câmera|^Turn off camera/i);
  if (micLigado) porRotulo(/^Desativar microfone|^Turn off microphone/i).click();
  if (camLigada) porRotulo(/^Desativar câmera|^Turn off camera/i).click();

  const esperando = /Aguarde até que|Asking to be let in|Pedindo para entrar/i.test(texto);
  const legendasBotao = porRotulo(/^Ativar legendas|^Turn on captions/i);
  const legendasLigadas = !!porRotulo(/^Desativar legendas|^Turn off captions/i);
  const naSala = legendasLigadas || !!legendasBotao;

  if (!naSala && !esperando && !micLigado && !camLigada) {
    const entrar = porTexto(/^(Pedir para participar|Participar agora|Ask to join|Join now)$/i);
    if (entrar) { entrar.click(); return JSON.stringify({ estado: 'pedindo' }); }
  }
  if (esperando) return JSON.stringify({ estado: 'aguardando' });
  if (!naSala) return JSON.stringify({ estado: 'entrando', mic: micLigado, cam: camLigada });

  // Na sala: legendas ligadas e o painel lido cru — juntar falas é na web.
  if (legendasBotao) legendasBotao.click();
  const painel = document.querySelector('[role=region][aria-label*=egenda]')
    || document.querySelector('[role=region][aria-label*=aption]');
  return JSON.stringify({
    estado: 'na-reuniao',
    legendas: painel ? painel.innerText : '',
    semPainel: !painel,
  });
})()
"#;

async fn url_do_navegador() -> Result<String, String> {
    crate::gravador::url_do_browser(PORTA).await
}

async fn rodar(app: AppHandle, estado: Compartilhado, id: String, agente: String, link: String) {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let reserva = match maquina::reservar_uso(&app, "reuniao") {
        Ok(r) => r,
        Err(motivo) => return emitir(&app, json!({ "estado": "erro", "motivo": motivo })),
    };
    let Some(link) = link_de_meet(&link) else {
        return emitir(&app, json!({ "estado": "erro", "motivo": "esse link não é de uma reunião do Google Meet" }));
    };

    let filho = match abrir_chrome(&app, &agente, &link) {
        Ok(c) => c,
        Err(e) => return emitir(&app, json!({ "estado": "erro", "motivo": e })),
    };
    emitir(&app, json!({ "estado": "abrindo", "id": id, "agente": agente }));
    if !esperar_porta().await {
        fechar_chrome(&app, &agente);
        return emitir(&app, json!({ "estado": "erro", "motivo": "o Chrome do agente não respondeu" }));
    }

    let ws = {
        let mut achado = None;
        for _ in 0..12 {
            if let Ok(url) = url_do_navegador().await {
                if let Ok((w, _)) = tokio_tungstenite::connect_async(&url).await { achado = Some(w); break; }
            }
            tokio::time::sleep(std::time::Duration::from_millis(700)).await;
        }
        achado
    };
    let Some(ws) = ws else {
        fechar_chrome(&app, &agente);
        return emitir(&app, json!({ "estado": "erro", "motivo": "não conversei com o Chrome do agente" }));
    };

    let (tx_parar, mut rx_parar) = tokio::sync::mpsc::unbounded_channel::<()>();
    if let Ok(mut g) = estado.lock() {
        g.ativa = Some(Sessao { id: id.clone(), agente: agente.clone(), parar: tx_parar, _reserva: reserva });
    }
    barra::mostrar(&app, json!({
        "modo": "grav",
        "titulo": format!("{agente} está na reunião"),
        "sub": "entrou calado, sem câmera — só transcrição",
        "parar": true,
    }));

    let (mut tx, mut rx) = ws.split();
    let mut prox_id: u64 = 1;
    let mut sessao: Option<String> = None;
    let mut pedido_do_roteiro: Option<u64> = None;
    let mut ultimo_estado = String::new();
    let mut ultima_legenda = String::new();
    let mut avisou_sem_painel = false;
    let inicio = std::time::Instant::now();
    let mut motivo_fim = "pedido".to_string();

    let mandar = |metodo: &str, params: Value, sessao: Option<&str>, prox: &mut u64| -> (u64, Value) {
        let id = *prox; *prox += 1;
        let mut m = json!({ "id": id, "method": metodo, "params": params });
        if let Some(s) = sessao { m["sessionId"] = json!(s); }
        (id, m)
    };
    let (_, primeiro) = mandar("Target.setAutoAttach", json!({ "autoAttach": true, "waitForDebuggerOnStart": false, "flatten": true }), None, &mut prox_id);
    if tx.send(Message::Text(primeiro.to_string().into())).await.is_err() {
        fechar_chrome(&app, &agente);
        return emitir(&app, json!({ "estado": "erro", "motivo": "o Chrome do agente caiu ao abrir" }));
    }

    let mut relogio = tokio::time::interval(std::time::Duration::from_millis(900));
    loop {
        tokio::select! {
            _ = rx_parar.recv() => { break; }
            _ = relogio.tick() => {
                if inicio.elapsed() > std::time::Duration::from_secs(TETO_MIN * 60) {
                    motivo_fim = "tempo máximo de 2 horas".into();
                    break;
                }
                if let Some(sid) = sessao.clone() {
                    let (rid, m) = mandar("Runtime.evaluate", json!({ "expression": ROTEIRO, "returnByValue": true, "userGesture": true }), Some(&sid), &mut prox_id);
                    pedido_do_roteiro = Some(rid);
                    if tx.send(Message::Text(m.to_string().into())).await.is_err() { motivo_fim = "o Chrome do agente fechou".into(); break; }
                }
            }
            m = rx.next() => {
                let Some(Ok(Message::Text(txt))) = m else {
                    if matches!(m, Some(Ok(_))) { continue; }
                    motivo_fim = "o Chrome do agente fechou".into();
                    break;
                };
                let Ok(v) = serde_json::from_str::<Value>(&txt) else { continue };
                if v["method"].as_str() == Some("Target.attachedToTarget") {
                    let info = &v["params"]["targetInfo"];
                    if info["type"].as_str() != Some("page") { continue; }
                    if !info["url"].as_str().unwrap_or("").contains("meet.google.com") { continue; }
                    if let Some(sid) = v["params"]["sessionId"].as_str() {
                        sessao = Some(sid.to_string());
                        let (_, a) = mandar("Runtime.runIfWaitingForDebugger", json!({}), Some(sid), &mut prox_id);
                        let (_, b) = mandar("Runtime.enable", json!({}), Some(sid), &mut prox_id);
                        let _ = tx.send(Message::Text(a.to_string().into())).await;
                        let _ = tx.send(Message::Text(b.to_string().into())).await;
                    }
                    continue;
                }
                if Some(v["id"].as_u64().unwrap_or(0)) != pedido_do_roteiro.map(|x| x) { continue; }
                let Some(bruto) = v["result"]["result"]["value"].as_str() else { continue };
                let Ok(r) = serde_json::from_str::<Value>(bruto) else { continue };
                let est = r["estado"].as_str().unwrap_or("").to_string();
                if est != ultimo_estado {
                    ultimo_estado = est.clone();
                    emitir(&app, json!({ "estado": est, "id": id, "agente": agente }));
                    if est == "aguardando" {
                        barra::mesclar(&app, json!({ "sub": "esperando alguém aceitar a entrada" }));
                    } else if est == "na-reuniao" {
                        barra::mesclar(&app, json!({ "sub": "ouvindo pelas legendas — sem microfone, sem câmera" }));
                    }
                }
                if r["semPainel"].as_bool() == Some(true) && !avisou_sem_painel {
                    avisou_sem_painel = true;
                    emitir(&app, json!({ "estado": "sem-legendas", "id": id, "motivo": "não achei o painel de legendas do Meet" }));
                }
                let legendas = r["legendas"].as_str().unwrap_or("");
                if !legendas.is_empty() && legendas != ultima_legenda {
                    ultima_legenda = legendas.to_string();
                    let _ = app.emit("dnos://reuniao-meet/legenda", json!({ "id": id, "texto": legendas }));
                }
            }
        }
    }

    // Sair da sala antes de fechar: senão o agente fica "pendurado" para os outros.
    if let Some(sid) = sessao {
        let sair = r#"(() => { const b = [...document.querySelectorAll('button,[role=button]')].find((x) => /^Sair da chamada|^Leave call/i.test(x.getAttribute('aria-label') || '')); if (b) { b.click(); return 'saiu'; } return 'sem botão'; })()"#;
        let (_, m) = mandar("Runtime.evaluate", json!({ "expression": sair, "returnByValue": true, "userGesture": true }), Some(&sid), &mut prox_id);
        let _ = tx.send(Message::Text(m.to_string().into())).await;
        tokio::time::sleep(std::time::Duration::from_millis(900)).await;
    }
    fechar_chrome(&app, &agente);
    barra::esconder(&app);
    if let Ok(mut g) = estado.lock() { g.ativa = None; }
    drop(filho);
    emitir(&app, json!({ "estado": "saiu", "id": id, "agente": agente, "motivo": motivo_fim }));
}

pub fn parar_pela_barra(app: &AppHandle) -> bool {
    let Some(e) = app.try_state::<Compartilhado>() else { return false };
    e.lock().ok().and_then(|g| g.ativa.as_ref().map(|a| a.parar.send(()).is_ok())).unwrap_or(false)
}

pub fn instalar(app: &AppHandle) {
    app.manage::<Compartilhado>(Arc::new(Mutex::new(Meet::default())));

    let h = app.clone();
    app.listen_any("dnos://reuniao-meet/entrar", move |evento| {
        let v: Value = serde_json::from_str(evento.payload()).unwrap_or(json!({}));
        let id = v["id"].as_str().unwrap_or("").trim().to_string();
        let agente = v["agente"].as_str().unwrap_or("").trim().to_string();
        let link = v["link"].as_str().unwrap_or("").trim().to_string();
        if id.is_empty() || agente.is_empty() || link.is_empty() {
            emitir(&h, json!({ "estado": "erro", "motivo": "faltou o agente ou o link da reunião" }));
            return;
        }
        if ativa(&h) {
            emitir(&h, json!({ "estado": "erro", "motivo": "o agente já está numa reunião" }));
            return;
        }
        let Some(estado) = h.try_state::<Compartilhado>().map(|e| e.inner().clone()) else { return };
        let h2 = h.clone();
        tauri::async_runtime::spawn(rodar(h2, estado, id, agente, link));
    });

    let h = app.clone();
    app.listen_any("dnos://reuniao-meet/sair", move |_| { parar_pela_barra(&h); });

    // A barra é a mesma da gravação; quando quem está no ar é o Meet, é ele que sai.
    let h = app.clone();
    app.listen_any("dnos://gravador/parar-pela-barra", move |_| { parar_pela_barra(&h); });

    let h = app.clone();
    app.listen_any("dnos://reuniao-meet/estado", move |_| {
        let atual = h.try_state::<Compartilhado>()
            .and_then(|e| e.lock().ok().map(|g| g.ativa.as_ref().map(|a| (a.id.clone(), a.agente.clone()))))
            .flatten();
        match atual {
            Some((id, agente)) => emitir(&h, json!({ "estado": "na-reuniao", "id": id, "agente": agente })),
            None => emitir(&h, json!({ "estado": "fora" })),
        }
    });
}
