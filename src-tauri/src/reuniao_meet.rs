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

use crate::{maquina, meu_chrome};

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

/// Tranca microfone e câmera no perfil do agente, ANTES de abrir o Chrome.
///
/// Dois motivos, os dois medidos em 20/09:
/// - a primeira entrada da Cora levou 68 s porque o Chrome pedia permissão de
///   microfone e câmera numa janela que o roteiro não enxerga (o Rodrigo teve
///   que clicar em "Permitir" na mão);
/// - desligar o microfone pelo botão do Meet é promessa de software; permissão
///   negada é impedimento. Com isto o agente NÃO CONSEGUE transmitir voz nem
///   imagem, mesmo que algum passo do roteiro falhe.
///
/// As legendas continuam chegando: elas vêm do servidor do Meet, não do áudio
/// captado aqui (provado ao vivo, com a permissão negada).
///
/// `2` é o valor do Chrome para "bloquear". O padrão não basta sozinho: uma
/// permissão dada antes vira exceção por site e vence o padrão — foi o que
/// aconteceu no perfil da Cora. Por isso as exceções existentes também caem.
fn trancar_microfone_e_camera(dados: &std::path::Path) {
    let arquivo = dados.join("Default").join("Preferences");
    // Perfil novo ainda não tem o arquivo. Escrever um mínimo ANTES da primeira
    // abertura é o que impede a janela de permissão de aparecer logo na estreia
    // do agente — provado com um perfil zerado: microfone e câmera já nascem
    // negados, sem nenhuma pergunta na tela.
    if !arquivo.exists() {
        let _ = std::fs::create_dir_all(arquivo.parent().unwrap_or(dados));
        let minimo = json!({ "profile": { "default_content_setting_values": { "media_stream_mic": 2, "media_stream_camera": 2 } } });
        let _ = std::fs::write(&arquivo, minimo.to_string());
        return;
    }
    let Ok(texto) = std::fs::read_to_string(&arquivo) else { return };
    let Ok(mut v) = serde_json::from_str::<Value>(&texto) else { return };
    let perfil = v.as_object_mut().and_then(|o| o.entry("profile").or_insert(json!({})).as_object_mut().map(|_| ()));
    if perfil.is_none() { return; }
    for chave in ["media_stream_mic", "media_stream_camera"] {
        v["profile"]["default_content_setting_values"][chave] = json!(2);
        if let Some(sites) = v["profile"]["content_settings"]["exceptions"][chave].as_object_mut() {
            for (_site, regra) in sites.iter_mut() { regra["setting"] = json!(2); }
        }
    }
    let _ = std::fs::write(&arquivo, v.to_string());
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
    trancar_microfone_e_camera(&dados);
    Command::new(bin)
        .arg(format!("--remote-debugging-port={PORTA}"))
        .arg(format!("--user-data-dir={}", dados.display()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        // Mudo, sempre: a pessoa já ouve a reunião pelo Chrome dela. Sem isto o
        // som saía duas vezes (e a própria voz voltava com atraso). As legendas
        // vêm do servidor do Meet, não do áudio que toca aqui — silenciar não
        // tira nada da transcrição.
        .arg("--mute-audio")
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
///
/// Os rótulos vêm da tela real do Meet, lida em 20/09 com o Chrome do Milo.
/// Dois detalhes que custaram descobrir: o Meet rotula microfone/câmera/legendas
/// com a AÇÃO ("Desativar microfone" = está ligado), e alguns botões — como o
/// de pessoas — não têm `aria-label`: o nome vem de `aria-labelledby`.
const ROTEIRO: &str = r#"
(() => {
  const botoes = () => [...document.querySelectorAll('button,[role=button]')];
  const nomeDe = (b) => {
    const direto = (b.getAttribute('aria-label') || '').trim();
    if (direto) return direto;
    const ids = (b.getAttribute('aria-labelledby') || '').split(/\s+/).filter(Boolean);
    return ids.map((i) => (document.getElementById(i) || {}).textContent || '').join(' ').trim();
  };
  const porRotulo = (re) => botoes().find((b) => re.test(b.getAttribute('aria-label') || ''));
  const porTexto = (re) => botoes().find((b) => re.test((b.innerText || '').trim()));
  const texto = document.body.innerText || '';

  // Microfone e câmera: desligar antes de entrar não é opcional.
  const micLigado = !!porRotulo(/^Desativar microfone|^Turn off microphone/i);
  const camLigada = !!porRotulo(/^Desativar câmera|^Turn off camera/i);
  if (micLigado) porRotulo(/^Desativar microfone|^Turn off microphone/i).click();
  if (camLigada) porRotulo(/^Desativar câmera|^Turn off camera/i).click();

  const esperando = /Aguarde até que|Asking to be let in|Pedindo para entrar/i.test(texto);
  const legendasBotao = porRotulo(/^Ativar legendas|^Turn on captions/i);
  const legendasLigadas = !!porRotulo(/^Desativar legendas|^Turn off captions/i);
  const naSala = legendasLigadas || !!legendasBotao;

  // Telas em que não há mais o que fazer: melhor dizer do que ficar esperando.
  if (!naSala) {
    if (/pedido para participar foi negado|negou (sua|seu|o seu)|denied your request|Você não pode participar d|You can.t join this/i.test(texto)) return JSON.stringify({ estado: 'negado' });
    if (/Você saiu da reunião|Você foi removido|reunião foi encerrada|You left the meeting|You.ve been removed|meeting has ended/i.test(texto)) return JSON.stringify({ estado: 'encerrada' });
    // Depois da tela de saída, o Meet volta sozinho para a tela inicial (achado em
    // 20/09: sem isto o agente ficava "entrando" até o teto de 2 horas).
    if (/^\/(home|landing)?\/?$/.test(location.pathname)) return JSON.stringify({ estado: 'encerrada' });
  }

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

  // Quantas pessoas: o botão "Pessoas" mostra o número (o nome vem de aria-labelledby).
  let pessoas = null;
  for (const b of botoes()) {
    if (/^(Pessoas|People)$/i.test(nomeDe(b))) {
      const t = (b.innerText || '').trim();
      if (/^\d+$/.test(t)) { pessoas = parseInt(t, 10); break; }
    }
  }
  return JSON.stringify({
    estado: 'na-reuniao',
    legendas: painel ? painel.innerText : '',
    // O painel só existe DEPOIS de as legendas ligarem (e fica com altura zero
    // enquanto ninguém fala): antes disso, "sem painel" seria falso alarme.
    legendasOn: legendasLigadas,
    semPainel: legendasLigadas && !painel,
    pessoas,
  });
})()
"#;

/// Coloca o "Idioma da reunião" em Português (Brasil). É uma configuração da
/// SALA, não do agente: na sala do Milo já vem em português, na de quem tem a
/// conta em inglês vem em inglês — e aí as legendas saem em inglês, com a fala
/// em português virando bobagem. Roda com `awaitPromise` porque espera a tela.
/// Testado ao vivo em 20/09: Inglês → Português (Brasil), nos dois caminhos.
const IDIOMA: &str = r#"
(async () => {
  const espera = (ms) => new Promise((r) => setTimeout(r, ms));
  const PT = /^Portugu[eê]s \(Brasil\)$|^Portuguese \(Brazil\)$/i;
  const combo = () => [...document.querySelectorAll('[role=combobox]')].find((c) => /Idioma da reunião|Meeting language/i.test(c.getAttribute('aria-label') || ''));
  const valor = (c) => (c?.innerText || '').replace(/^language\s*/i, '').replace(/\s+/g, ' ').trim();
  let c = combo();
  let abriuPainel = false;
  if (!c) {
    const abrir = [...document.querySelectorAll('button,[role=button]')].find((b) => /^(Abrir configurações de legenda|Open caption settings)/i.test(b.getAttribute('aria-label') || ''));
    if (!abrir) return { ok: false, motivo: 'sem botão de configurações de legenda' };
    abrir.click(); abriuPainel = true;
    for (let i = 0; i < 10 && !c; i++) { await espera(300); c = combo(); }
    if (!c) return { ok: false, motivo: 'painel de legendas não abriu' };
  }
  const antes = valor(c);
  const fecharSeAbri = async () => {
    if (!abriuPainel) return;
    const fechar = [...document.querySelectorAll('button,[role=button]')].find((b) => /^(Fechar caixa de diálogo|Close dialog)/i.test(b.getAttribute('aria-label') || ''));
    if (fechar) { fechar.click(); await espera(400); }
  };
  if (PT.test(antes)) { await fecharSeAbri(); return { ok: true, ja: true, antes }; }
  if (c.getAttribute('aria-expanded') !== 'true') { c.click(); await espera(500); }
  // Duas listas de idioma existem no DOM (a das Configurações fica escondida): vale a visível.
  const lista = [...document.querySelectorAll('[role=listbox]')].find((l) => /Idioma da reunião|Meeting language/i.test(l.getAttribute('aria-label') || '') && l.getBoundingClientRect().height > 0);
  if (!lista) return { ok: false, motivo: 'lista de idiomas não apareceu', antes };
  const opcao = [...lista.querySelectorAll('[role=option]')].find((o) => PT.test((o.getAttribute('aria-label') || '').replace(/\s*BETA$/i, '').trim()));
  if (!opcao) return { ok: false, motivo: 'sem a opção Português (Brasil)', antes };
  opcao.click();
  await espera(1000);
  const depois = valor(combo());
  // Deixa a tela como encontrou: quem abriu o painel de configurações fecha.
  if (abriuPainel) {
    const fechar = [...document.querySelectorAll('button,[role=button]')].find((b) => /^(Fechar caixa de diálogo|Close dialog)/i.test(b.getAttribute('aria-label') || ''));
    if (fechar) { fechar.click(); await espera(400); }
  }
  return { ok: PT.test(depois), antes, depois, abriuPainel };
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
    maquina::mostrar_barra(&app);
    maquina::falar_na_barra(&app, "meet", &agente, "", "entrando na sala — sem microfone, sem câmera".into(), false);

    let (mut tx, mut rx) = ws.split();
    let mut prox_id: u64 = 1;
    let mut sessao: Option<String> = None;
    // Vários pedidos podem estar em voo: a resposta de um pedido antigo ainda vale.
    let mut pedidos_do_roteiro: std::collections::HashSet<u64> = std::collections::HashSet::new();
    let mut ultimo_estado = String::new();
    let mut ultima_legenda = String::new();
    let mut avisou_sem_painel = false;
    // Idioma: tenta algumas vezes (a tela demora a montar) e depois avisa.
    let mut idioma_ok = false;
    let mut legendas_on = false;
    let mut tentativas_idioma: u32 = 0;
    let mut ultimo_idioma: Option<std::time::Instant> = None;
    let mut pedidos_de_idioma: std::collections::HashSet<u64> = std::collections::HashSet::new();
    // Quem saiu deixa o agente sozinho: ele não fica pendurado sem ninguém.
    let mut viu_outros = false;
    let mut sozinho_desde: Option<std::time::Instant> = None;
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

    let mut relogio = tokio::time::interval(std::time::Duration::from_millis(500));
    loop {
        tokio::select! {
            _ = rx_parar.recv() => { break; }
            _ = relogio.tick() => {
                if inicio.elapsed() > std::time::Duration::from_secs(TETO_MIN * 60) {
                    motivo_fim = "tempo máximo de 2 horas".into();
                    break;
                }
                if let Some(sid) = sessao.clone() {
                    // Só depois de as legendas ligarem: é ali que o Meet mostra o idioma.
                    if ultimo_estado == "na-reuniao" && legendas_on && !idioma_ok && tentativas_idioma < 6
                        && ultimo_idioma.map(|t| t.elapsed() >= std::time::Duration::from_secs(4)).unwrap_or(true) {
                        tentativas_idioma += 1;
                        ultimo_idioma = Some(std::time::Instant::now());
                        let (rid, m) = mandar("Runtime.evaluate", json!({ "expression": IDIOMA, "returnByValue": true, "awaitPromise": true, "userGesture": true }), Some(&sid), &mut prox_id);
                        pedidos_de_idioma.insert(rid);
                        if tx.send(Message::Text(m.to_string().into())).await.is_err() { motivo_fim = "o Chrome do agente fechou".into(); break; }
                    }
                    let (rid, m) = mandar("Runtime.evaluate", json!({ "expression": ROTEIRO, "returnByValue": true, "userGesture": true }), Some(&sid), &mut prox_id);
                    pedidos_do_roteiro.insert(rid);
                    if pedidos_do_roteiro.len() > 40 { pedidos_do_roteiro.clear(); pedidos_do_roteiro.insert(rid); }
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
                let Some(rid) = v["id"].as_u64() else { continue };
                if pedidos_de_idioma.remove(&rid) {
                    let r = &v["result"]["result"]["value"];
                    if r["ok"].as_bool() == Some(true) {
                        idioma_ok = true;
                        emitir(&app, json!({ "estado": "idioma", "id": id, "agente": agente, "ok": true, "de": r["antes"], "para": r["depois"] }));
                    } else if tentativas_idioma >= 6 {
                        emitir(&app, json!({ "estado": "idioma", "id": id, "agente": agente, "ok": false, "motivo": r["motivo"] }));
                    }
                    continue;
                }
                if !pedidos_do_roteiro.remove(&rid) { continue; }
                let Some(bruto) = v["result"]["result"]["value"].as_str() else { continue };
                let Ok(r) = serde_json::from_str::<Value>(bruto) else { continue };
                let est = r["estado"].as_str().unwrap_or("").to_string();
                if est != ultimo_estado {
                    ultimo_estado = est.clone();
                    emitir(&app, json!({ "estado": est, "id": id, "agente": agente }));
                    if est == "aguardando" {
                        maquina::falar_na_barra(&app, "meet", &agente, "", "esperando alguém aceitar a entrada".into(), false);
                    } else if est == "na-reuniao" {
                        maquina::falar_na_barra(&app, "meet", &agente, "", "ouvindo pelas legendas — sem microfone, sem câmera".into(), false);
                    }
                }
                legendas_on = r["legendasOn"].as_bool() == Some(true);
                if est == "negado" { motivo_fim = "a entrada foi negada".into(); break; }
                if est == "encerrada" { motivo_fim = "a reunião acabou".into(); break; }
                if let Some(n) = r["pessoas"].as_u64() {
                    if n >= 2 { viu_outros = true; sozinho_desde = None; }
                    else if n == 1 {
                        let desde = *sozinho_desde.get_or_insert_with(std::time::Instant::now);
                        // Quem estava e saiu: 30 s de folga (queda de rede volta rápido).
                        // Ninguém nunca entrou além do agente: espera mais antes de desistir.
                        let folga = if viu_outros { 30 } else { 90 };
                        if desde.elapsed() >= std::time::Duration::from_secs(folga) {
                            motivo_fim = if viu_outros { "todos saíram da reunião".into() } else { "ninguém entrou na reunião".into() };
                            break;
                        }
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
    maquina::esconder_barra(&app);
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
