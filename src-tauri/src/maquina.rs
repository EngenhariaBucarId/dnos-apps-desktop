//! Gravador da máquina toda — "Aprenda comigo", fase 1 (13/09/2026).
//!
//! O gravador do Chrome (gravador.rs) escuta o DOM pelo CDP. Este escuta o
//! Mac inteiro: qualquer app, pela Acessibilidade e pelo CGEventTap, SÓ
//! observando. O trabalho pesado fica num ajudante em Swift
//! (`ajudantes/gravador-mac.swift`, compilado no CI e embutido aqui por
//! `include_bytes!`); a casca o escreve em `<app_data>/ajudantes/` na primeira
//! vez, o roda como processo filho e lê um passo por linha JSON no stdout.
//!
//! Eventos da página:
//!   `dnos://gravador/iniciar {nome?, modo:"mac"}` — cai aqui (gravador.rs despacha).
//!   `dnos://maquina/permissoes {pedir?}` → `dnos://maquina/permissoes-estado {acessibilidade, tela, ajudante, casca, versao}`.
//! Nota, parar, estado, listar, abrir, apagar: os mesmos do gravador do Chrome —
//! a gravação sai no mesmo JSON, com `modo: "mac"`, para a mesma revisão.
//!
//! Só existe no macOS. Em outro sistema `disponivel()` é falso e a página não
//! oferece a opção.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use tauri::{AppHandle, Emitter, Listener, Manager};
use tokio::sync::mpsc;

use crate::gravador::{self, Ativa, Compartilhado};
use crate::meu_chrome;

#[cfg(target_os = "macos")]
const AJUDANTE: &[u8] = include_bytes!("../ajudantes/dnos-gravador-mac");
#[cfg(not(target_os = "macos"))]
const AJUDANTE: &[u8] = &[];

pub fn disponivel() -> bool {
    cfg!(target_os = "macos") && !AJUDANTE.is_empty()
}

/// A casca em si é confiável para a Acessibilidade? (o ajudante herda da casca,
/// que é o "processo responsável"; quando os dois discordam, o macOS está com
/// um registro velho da casca — cada build sem assinatura é um app novo).
#[cfg(target_os = "macos")]
fn casca_confiavel() -> bool {
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" { fn AXIsProcessTrusted() -> bool; }
    unsafe { AXIsProcessTrusted() }
}
#[cfg(not(target_os = "macos"))]
fn casca_confiavel() -> bool { false }

/// Escreve o ajudante em disco (uma vez por versão da casca) e devolve o caminho.
fn caminho_do_ajudante(app: &AppHandle) -> Result<PathBuf, String> {
    if !disponivel() {
        return Err("gravador da máquina só existe no macOS".into());
    }
    let pasta = app.path().app_data_dir().map_err(|e| e.to_string())?.join("ajudantes");
    std::fs::create_dir_all(&pasta).map_err(|e| e.to_string())?;
    let versao = app.package_info().version.to_string();
    let arq = pasta.join(format!("dnos-gravador-mac-{versao}"));
    let atual = std::fs::metadata(&arq).map(|m| m.len() as usize).unwrap_or(0);
    if atual != AJUDANTE.len() {
        std::fs::write(&arq, AJUDANTE).map_err(|e| format!("não escrevi o ajudante: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&arq, std::fs::Permissions::from_mode(0o755));
        }
        meu_chrome::registrar(app, &format!("maquina: ajudante escrito em {}", arq.display()));
    }
    Ok(arq)
}

/// `permissoes [--pedir]` → {acessibilidade, tela}. Com `pedir`, o macOS abre os diálogos.
fn permissoes(app: &AppHandle, pedir: bool) -> Value {
    let arq = match caminho_do_ajudante(app) {
        Ok(a) => a,
        Err(e) => return json!({ "ajudante": false, "acessibilidade": false, "tela": false, "motivo": e }),
    };
    let mut cmd = Command::new(&arq);
    cmd.arg("permissoes");
    if pedir { cmd.arg("--pedir"); }
    match cmd.output() {
        Ok(saida) => {
            let txt = String::from_utf8_lossy(&saida.stdout);
            let mut v: Value = txt.lines().rev().find_map(|l| serde_json::from_str::<Value>(l).ok()).unwrap_or(json!({}));
            v["ajudante"] = json!(true);
            v["casca"] = json!(casca_confiavel());
            v["versao"] = json!(app.package_info().version.to_string());
            if !saida.status.success() { v["motivo"] = json!(format!("ajudante saiu com {}", saida.status)); }
            v
        }
        Err(e) => json!({ "ajudante": false, "acessibilidade": false, "tela": false, "motivo": format!("não rodei o ajudante: {e}") }),
    }
}

pub fn instalar(app: &AppHandle) {
    let h = app.clone();
    app.listen_any("dnos://maquina/permissoes", move |evento| {
        let v: Value = serde_json::from_str(evento.payload()).unwrap_or(json!({}));
        let pedir = v["pedir"].as_bool().unwrap_or(false);
        let h2 = h.clone();
        // O diálogo do macOS bloqueia o ajudante até a pessoa responder: fora do laço de eventos.
        std::thread::spawn(move || {
            let r = permissoes(&h2, pedir);
            meu_chrome::registrar(&h2, &format!("maquina: permissoes {r}"));
            // Nome DIFERENTE do que a casca escuta: emitir no mesmo nome fazia a
            // casca responder a si mesma sem parar (0.6.0/0.6.1: 60 mil linhas de diário em 30 min).
            let _ = h2.emit("dnos://maquina/permissoes-estado", r);
        });
    });
}

/// A gravação em si. Mesmo contrato do gravador do Chrome: `estado` guarda a
/// ativa (notas e parar chegam por ela), o JSON final vai para a mesma pasta e
/// sai por `dnos://gravador/pronta`.
pub async fn iniciar(app: AppHandle, estado: Compartilhado, nome: String) {
    if estado.lock().map(|g| g.ativa.is_some()).unwrap_or(false) {
        return gravador::emitir(&app, "erro", 0, None, Some("já existe uma gravação em andamento".into()));
    }
    let arq = match caminho_do_ajudante(&app) {
        Ok(a) => a,
        Err(e) => return gravador::emitir(&app, "erro", 0, None, Some(e)),
    };
    let identificador = app.config().identifier.clone();
    let mut filho = match Command::new(&arq)
        .arg("gravar").arg("--ignorar").arg(&identificador)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn()
    {
        Ok(f) => f,
        Err(e) => return gravador::emitir(&app, "erro", 0, None, Some(format!("não abri o gravador da máquina: {e}"))),
    };
    let mut entrada = filho.stdin.take();
    let saida = filho.stdout.take();
    let erro = filho.stderr.take();

    let id = format!("{}-{:x}", gravador::agora_ms(), std::process::id());
    let mut nome = if nome.trim().is_empty() { format!("Gravação de {}", gravador::chrono_curto()) } else { nome.trim().to_string() };
    let (tx_notas, mut rx_notas) = mpsc::unbounded_channel::<Value>();
    let (tx_parar, mut rx_parar) = mpsc::unbounded_channel::<(Option<String>, Option<String>)>();
    if let Ok(mut g) = estado.lock() {
        g.ativa = Some(Ativa { id: id.clone(), passos: 0, notas: tx_notas, parar: tx_parar });
    }

    // stdout do ajudante → canal (thread bloqueante; o laço abaixo é async).
    let (tx_linhas, mut rx_linhas) = mpsc::unbounded_channel::<Value>();
    if let Some(s) = saida {
        std::thread::spawn(move || {
            for l in BufReader::new(s).lines().map_while(Result::ok) {
                if let Ok(v) = serde_json::from_str::<Value>(&l) { if tx_linhas.send(v).is_err() { break; } }
            }
        });
    }
    // stderr → diário (o ajudante quase não escreve; quando escreve, importa).
    if let Some(e) = erro {
        let h = app.clone();
        std::thread::spawn(move || {
            for l in BufReader::new(e).lines().map_while(Result::ok) { meu_chrome::registrar(&h, &format!("maquina/ajudante: {l}")); }
        });
    }

    gravador::emitir(&app, "gravando", 0, Some(&id), None);
    crate::voz::ligar(&app);

    let inicio = gravador::agora_ms();
    let mut passos: Vec<Value> = Vec::new();
    let mut notas: Vec<Value> = Vec::new();
    let mut criterio: Option<String> = None;
    let mut motivo_fim = "parado".to_string();
    let mut permissoes_no_inicio = json!({});
    let mut pediu_parar = false;

    loop {
        tokio::select! {
            Some((c, n)) = rx_parar.recv(), if !pediu_parar => {
                criterio = c; if let Some(n) = n { nome = n; }
                pediu_parar = true;
                if let Some(e) = entrada.as_mut() { let _ = e.write_all(b"parar\n"); let _ = e.flush(); }
            }
            Some(n) = rx_notas.recv() => { notas.push(n); }
            l = rx_linhas.recv() => {
                let Some(v) = l else {
                    if !pediu_parar { motivo_fim = "o gravador da máquina fechou".into(); }
                    break;
                };
                match v["t"].as_str().unwrap_or("") {
                    "pronto" => {
                        permissoes_no_inicio = json!({ "acessibilidade": v["acessibilidade"], "tela": v["tela"] });
                        meu_chrome::registrar(&app, &format!("maquina: gravando (acessibilidade={}, tela={})", v["acessibilidade"], v["tela"]));
                    }
                    "fim" => break,
                    "erro" => {
                        motivo_fim = v["motivo"].as_str().unwrap_or("erro no gravador da máquina").to_string();
                        break;
                    }
                    "" => {}
                    _ => {
                        let mut p = v;
                        p["n"] = json!(passos.len() + 1);
                        if p.get("hora").is_none() { p["hora"] = json!(gravador::agora_ms()); }
                        passos.push(p);
                        if let Ok(mut g) = estado.lock() { if let Some(a) = g.ativa.as_mut() { a.passos = passos.len(); } }
                        gravador::emitir(&app, "gravando", passos.len(), Some(&id), None);
                    }
                }
            }
            // Pediu parar e o ajudante não respondeu "fim" em 2 s: encerra na marra.
            _ = tokio::time::sleep(std::time::Duration::from_secs(2)), if pediu_parar => { break; }
        }
    }
    let _ = filho.kill();
    let _ = filho.wait();
    crate::voz::desligar(&app);

    let gravacao = json!({
        "id": id, "nome": nome, "inicio": inicio, "fim": gravador::agora_ms(), "modo": "mac",
        "instancia": std::env::var("DNOS_URL").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "https://dnos.dnia.ai".into()),
        "criterio": criterio, "passos": passos, "notas": notas, "encerrada_por": motivo_fim,
        "permissoes": permissoes_no_inicio,
    });
    if let Some(p) = gravador::pasta(&app) {
        let _ = std::fs::write(p.join(format!("{id}.json")), gravacao.to_string());
    }
    if let Ok(mut g) = estado.lock() { g.ativa = None; }
    let n = gravacao["passos"].as_array().map(|a| a.len()).unwrap_or(0);
    gravador::emitir(&app, "parado", n, Some(&id), Some(motivo_fim.clone()));
    let _ = app.emit("dnos://gravador/pronta", json!({ "gravacao": gravacao }));
}
