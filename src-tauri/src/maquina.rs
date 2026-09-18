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
//!   `dnos://roteiro/executar {modo:"mac", nome, criterio, passos}` (roteiro.rs despacha para cá):
//!     o ajudante executa os passos no Mac (fase 2); progresso em `dnos://roteiro`
//!     `{estado: rodando|parou|concluido|erro, modo:"mac", passo, total, texto, motivo, tela:{app, janela}}`
//!     e numa barra flutuante ("barra-mac") com Parar; `dnos://roteiro/parar` para.
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
use std::sync::{Arc, Mutex};

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

/// Microfone (13/09, 0.7.6): o ajudante é um binário solto e o macOS julga o
/// microfone pela identidade DELE (sem texto de uso → negado na hora, sem
/// pergunta). Estado e pedido têm que sair do próprio app: o estado pela
/// AVFoundation, o pedido abrindo o microfone por um instante (o CoreAudio
/// mostra o diálogo do sistema quando ainda não foi perguntado).
#[cfg(target_os = "macos")]
mod microfone {
    use std::ffi::c_void;
    #[link(name = "objc", kind = "dylib")]
    extern "C" {
        fn objc_getClass(nome: *const std::os::raw::c_char) -> *mut c_void;
        fn sel_registerName(nome: *const std::os::raw::c_char) -> *mut c_void;
        fn objc_msgSend();
    }
    #[link(name = "AVFoundation", kind = "framework")]
    extern "C" { static AVMediaTypeAudio: *mut c_void; }
    /// 0 ainda não perguntado · 1 restrito · 2 negado · 3 autorizado · -1 sem AVFoundation
    pub fn estado() -> i64 {
        unsafe {
            let cls = objc_getClass(b"AVCaptureDevice\0".as_ptr() as *const _);
            if cls.is_null() { return -1; }
            let sel = sel_registerName(b"authorizationStatusForMediaType:\0".as_ptr() as *const _);
            let f: extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> i64 = std::mem::transmute(objc_msgSend as *const c_void);
            f(cls, sel, AVMediaTypeAudio)
        }
    }
    pub fn autorizado() -> bool { estado() == 3 }
    /// Pede: abre o microfone por um instante (dispara a pergunta do sistema) e
    /// espera a resposta até 2 min; já negado → abre o painel dos Ajustes.
    pub fn pedir() -> bool {
        match estado() {
            3 => true,
            0 => {
                // Segura o microfone aberto enquanto a pergunta está na tela (até 2 min).
                crate::voz::segurar_microfone_ate(&|| estado() != 0, std::time::Duration::from_secs(120));
                estado() == 3
            }
            _ => {
                let _ = std::process::Command::new("/usr/bin/open").arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone").status();
                false
            }
        }
    }
}
#[cfg(not(target_os = "macos"))]
mod microfone {
    pub fn autorizado() -> bool { false }
    pub fn pedir() -> bool { false }
}

/// `permissoes [--pedir]` → {acessibilidade, tela, microfone}. Com `pedir`, uma por vez:
/// Acessibilidade e Tela pelo ajudante; Microfone pelo próprio app (ver `microfone`).
fn permissoes(app: &AppHandle, pedir: bool) -> Value {
    let arq = match caminho_do_ajudante(app) {
        Ok(a) => a,
        Err(e) => return json!({ "ajudante": false, "acessibilidade": false, "tela": false, "motivo": e }),
    };
    let rodar = |pedir: bool| -> Value {
        let mut cmd = Command::new(&arq);
        cmd.arg("permissoes");
        if pedir { cmd.arg("--pedir"); }
        match cmd.output() {
            Ok(saida) => {
                let txt = String::from_utf8_lossy(&saida.stdout);
                let mut v: Value = txt.lines().rev().find_map(|l| serde_json::from_str::<Value>(l).ok()).unwrap_or(json!({}));
                v["ajudante"] = json!(true);
                if !saida.status.success() { v["motivo"] = json!(format!("ajudante saiu com {}", saida.status)); }
                v
            }
            Err(e) => json!({ "ajudante": false, "acessibilidade": false, "tela": false, "motivo": format!("não rodei o ajudante: {e}") }),
        }
    };
    let mut v = rodar(false);
    if pedir {
        let ax = v["acessibilidade"].as_bool().unwrap_or(false);
        let tela = v["tela"].as_bool().unwrap_or(false);
        if !ax || !tela { v = rodar(true); }
        else if !microfone::autorizado() { let ok = microfone::pedir(); meu_chrome::registrar(app, &format!("maquina: microfone pedido pelo app → {}", if ok { "autorizado" } else { "não" })); }
    }
    v["microfone"] = json!(microfone::autorizado());
    v["casca"] = json!(casca_confiavel());
    v["versao"] = json!(app.package_info().version.to_string());
    v
}

/// Estado da execução em curso: o stdin do ajudante (para "parar") e se está rodando.
#[derive(Default)]
pub struct ExecucaoMac {
    pub entrada: Option<std::process::ChildStdin>,
    pub rodando: bool,
    /// Último estado da barra do computador. A webview pede este valor quando
    /// termina de carregar, porque o primeiro emit pode acontecer antes de ela
    /// instalar o listener.
    pub barra_atual: Option<Value>,
}
pub type ExecucaoCompartilhada = Arc<Mutex<ExecucaoMac>>;

/// Gravação e execução usam o mesmo mouse, teclado e barra. Uma reserva única
/// impede que as duas operações disputem o computador. O guard libera também
/// nos retornos antecipados e erros.
#[derive(Default)]
pub struct UsoComputador { modo: Option<&'static str> }
pub type UsoCompartilhado = Arc<Mutex<UsoComputador>>;

pub struct ReservaDeUso { estado: UsoCompartilhado, modo: &'static str }
impl Drop for ReservaDeUso {
    fn drop(&mut self) {
        if let Ok(mut g) = self.estado.lock() {
            if g.modo == Some(self.modo) { g.modo = None; }
        }
    }
}

pub fn reservar_uso(app: &AppHandle, modo: &'static str) -> Result<ReservaDeUso, String> {
    let estado = app.try_state::<UsoCompartilhado>().ok_or_else(|| "controle do computador indisponível".to_string())?.inner().clone();
    {
        let mut g = estado.lock().map_err(|_| "controle do computador indisponível".to_string())?;
        if let Some(atual) = g.modo {
            let fazendo = if atual == "gravando" { "uma gravação" } else if atual == "olhando" { "uma observação" } else { "uma execução" };
            return Err(format!("o computador já está ocupado com {fazendo}"));
        }
        g.modo = Some(modo);
    }
    Ok(ReservaDeUso { estado, modo })
}

/// Parar vindo da página ou da barra flutuante.
pub fn parar(app: &AppHandle) {
    if let Some(e) = app.try_state::<ExecucaoCompartilhada>() {
        if let Ok(mut g) = e.lock() {
            if let Some(entrada) = g.entrada.as_mut() { let _ = entrada.write_all(b"parar\n"); let _ = entrada.flush(); }
        }
    }
}

fn emitir_roteiro(app: &AppHandle, v: Value) {
    meu_chrome::registrar(app, &format!("roteiro-mac: {}", v.to_string().chars().take(220).collect::<String>()));
    let _ = app.emit("dnos://roteiro", v);
}

/// Escreve na barra flutuante. `modo` decide o texto e quem o Parar chama:
/// "atuando" (o agente executa) ou "assistindo" (a pessoa demonstra, 17/09).
/// A foto sai da mesma fonte da barra do Chrome.
pub(crate) fn falar_na_barra(app: &AppHandle, modo: &str, agente: &str, titulo: &str, sub: String, ouvindo: bool) {
    let foto = if agente.is_empty() { None } else { crate::barra::foto_do_agente(app, agente) };
    let estado = json!({
        "modo": modo, "agente": agente, "titulo": titulo, "sub": sub, "ouvindo": ouvindo, "foto": foto,
    });
    if let Some(e) = app.try_state::<ExecucaoCompartilhada>() {
        if let Ok(mut g) = e.lock() {
            let mut estado = estado.clone();
            if modo == "assistindo" { estado["ouvindo"] = g.barra_atual.as_ref().filter(|b| b["modo"] == "assistindo").map(|b| b["ouvindo"].clone()).unwrap_or(json!(false)); }
            g.barra_atual = Some(estado.clone());
            let _ = app.emit("dnos://barra-mac", estado);
            return;
        }
    }
    let _ = app.emit("dnos://barra-mac", estado);
}

/// A barra só acende o microfone quando o capturador detecta fala real.
pub(crate) fn atualizar_escuta(app: &AppHandle, ouvindo: bool) {
    if let Some(e) = app.try_state::<ExecucaoCompartilhada>() {
        if let Ok(mut g) = e.lock() {
            if let Some(v) = g.barra_atual.as_mut().filter(|v| v["modo"] == "assistindo") {
                v["ouvindo"] = json!(ouvindo);
                let _ = app.emit("dnos://barra-mac", v.clone());
            }
        }
    }
}

/// Barra flutuante por cima de tudo: "<agente> está usando o seu computador · passo 3 de 9" + Parar.
pub(crate) fn mostrar_barra(app: &AppHandle) {
    if app.get_webview_window("barra-mac").is_some() { return; }
    let _ = tauri::WebviewWindowBuilder::new(app, "barra-mac", tauri::WebviewUrl::App("barra-mac.html".into()))
        .title("dn.os")
        .inner_size(560.0, 58.0)
        .position(0.0, 0.0)
        .decorations(false)
        .always_on_top(true)
        .resizable(false)
        .skip_taskbar(true)
        .focused(false)
        .build();
    if let Some(w) = app.get_webview_window("barra-mac") {
        // Topo, centralizada no monitor principal.
        if let Ok(Some(m)) = w.primary_monitor() {
            let largura = m.size().width as f64 / m.scale_factor();
            let _ = w.set_position(tauri::LogicalPosition::new(((largura - 560.0) / 2.0).max(0.0), 8.0));
        }
    }
}
pub(crate) fn esconder_barra(app: &AppHandle) {
    if let Some(e) = app.try_state::<ExecucaoCompartilhada>() {
        if let Ok(mut g) = e.lock() { g.barra_atual = None; }
    }
    if let Some(w) = app.get_webview_window("barra-mac") { let _ = w.close(); }
}

pub async fn executar(app: AppHandle, pedido: Value) {
    let _reserva = match reservar_uso(&app, "executando") {
        Ok(r) => r,
        Err(e) => return emitir_roteiro(&app, json!({ "estado": "erro", "modo": "mac", "motivo": e })),
    };
    let estado = match app.try_state::<ExecucaoCompartilhada>() { Some(e) => e.inner().clone(), None => return };
    if estado.lock().map(|g| g.rodando).unwrap_or(false) { return emitir_roteiro(&app, json!({ "estado": "erro", "modo": "mac", "motivo": "já há um roteiro rodando" })); }
    let arq = match caminho_do_ajudante(&app) { Ok(a) => a, Err(e) => return emitir_roteiro(&app, json!({ "estado": "erro", "modo": "mac", "motivo": e })) };
    let nome = pedido["nome"].as_str().unwrap_or("habilidade").to_string();
    let criterio = pedido["criterio"].as_str().unwrap_or("").to_string();
    let agente = pedido["agente"].as_str().unwrap_or("").to_string();
    let id = pedido["id"].as_str().unwrap_or("").to_string();
    let passos: Vec<Value> = pedido["passos"].as_array().cloned().unwrap_or_default();
    if passos.is_empty() { return emitir_roteiro(&app, json!({ "estado": "erro", "modo": "mac", "motivo": "roteiro sem passos executáveis" })); }
    let total = passos.len();
    // O roteiro vai por arquivo: passos com fotos podem ser grandes demais para argumento.
    let pasta = match app.path().app_data_dir() { Ok(p) => p.join("roteiros"), Err(e) => return emitir_roteiro(&app, json!({ "estado": "erro", "modo": "mac", "motivo": e.to_string() })) };
    let _ = std::fs::create_dir_all(&pasta);
    let arq_roteiro = pasta.join(format!("{}.json", gravador::agora_ms()));
    if let Err(e) = std::fs::write(&arq_roteiro, json!({ "nome": nome, "passos": passos }).to_string()) { return emitir_roteiro(&app, json!({ "estado": "erro", "modo": "mac", "motivo": e.to_string() })); }
    let identificador = app.config().identifier.clone();
    let mut filho = match Command::new(&arq).arg("executar").arg("--roteiro").arg(&arq_roteiro).arg("--ignorar").arg(&identificador)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
        Ok(f) => f,
        Err(e) => return emitir_roteiro(&app, json!({ "estado": "erro", "modo": "mac", "motivo": format!("não abri o executor da máquina: {e}") })),
    };
    if let Ok(mut g) = estado.lock() { g.entrada = filho.stdin.take(); g.rodando = true; }
    let saida = filho.stdout.take();
    let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
    if let Some(s) = saida {
        std::thread::spawn(move || { for l in BufReader::new(s).lines().map_while(Result::ok) { if let Ok(v) = serde_json::from_str::<Value>(&l) { if tx.send(v).is_err() { break; } } } });
    }
    mostrar_barra(&app);
    falar_na_barra(&app, "atuando", &agente, &nome, format!("preparando · {total} passos"), false);
    emitir_roteiro(&app, json!({ "estado": "rodando", "modo": "mac", "id": id, "passo": 0, "total": total, "texto": "preparando" }));
    let mut fim = json!({ "estado": "erro", "modo": "mac", "motivo": "o executor da máquina fechou sem terminar" });
    while let Some(v) = rx.recv().await {
        match v["t"].as_str().unwrap_or("") {
            "passo" => {
                if v["estado"].as_str() == Some("rodando") {
                    let texto = v["texto"].as_str().unwrap_or("").to_string();
                    falar_na_barra(&app, "atuando", &agente, &nome, format!("passo {} de {} · {}", v["n"], total, texto), false);
                    emitir_roteiro(&app, json!({ "estado": "rodando", "modo": "mac", "id": id, "passo": v["n"], "total": total, "texto": texto }));
                }
            }
            "parou" => { fim = json!({ "estado": "parou", "modo": "mac", "id": id, "passo": v["passo"], "total": total, "texto": v["texto"], "motivo": v["motivo"], "tela": { "app": v["app"], "janela": v["janela"] }, "foto": v["foto"], "criterio": criterio }); break; }
            "concluido" => { fim = json!({ "estado": "concluido", "modo": "mac", "id": id, "total": total, "ms": v["ms"], "tela": { "app": v["app"], "janela": v["janela"] }, "foto": v["foto"], "criterio": criterio }); break; }
            "erro" => { fim = json!({ "estado": "erro", "modo": "mac", "id": id, "motivo": v["motivo"] }); break; }
            _ => {}
        }
    }
    let _ = filho.kill(); let _ = filho.wait();
    let _ = std::fs::remove_file(&arq_roteiro);
    if let Ok(mut g) = estado.lock() { g.entrada = None; g.rodando = false; }
    esconder_barra(&app);
    emitir_roteiro(&app, fim);
}

/// Nó do Mac (fase 2B, 13/09): a casca fica conectada ao relay da VPS
/// (`wss://tela.<domínio>/node?tipo=mac`, mesmo auth do Meu Chrome) esperando
/// pedidos de roteiro do agente; devolve o andamento por `{t:"roteiro-estado"}`.
/// A página manda `dnos://maquina/no {endereco, token}` ao entrar (e renova).
#[derive(Default)]
pub struct NoMac { pub endereco: String, pub token: String, pub geracao: u64 }
pub type NoMacCompartilhado = Arc<Mutex<NoMac>>;

async fn no_mac(app: AppHandle, estado: NoMacCompartilhado, geracao: u64) {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    let mut espera = 5u64;
    loop {
        let (endereco, token, gen) = match estado.lock() { Ok(g) => (g.endereco.clone(), g.token.clone(), g.geracao), Err(_) => return };
        if gen != geracao || endereco.is_empty() || token.is_empty() { return; } // outra conexão assumiu, ou saiu
        let url = format!("{}/node?tipo=mac", endereco.trim_end_matches('/'));
        match tokio_tungstenite::connect_async(&url).await {
            Ok((ws, _)) => {
                espera = 5;
                let (mut tx, mut rx) = ws.split();
                let versao = app.package_info().version.to_string();
                if tx.send(Message::Text(json!({ "t": "auth", "token": token, "versao": versao, "capacidades": ["olhar-v1"] }).to_string().into())).await.is_err() { continue; }
                meu_chrome::registrar(&app, "no-mac: conectado ao relay");
                // Andamento do roteiro → relay (só os eventos com id, que vieram de lá).
                let (para_relay, mut fila) = mpsc::unbounded_channel::<String>();
                let para_olhar = para_relay.clone();
                let ouvinte = app.listen_any("dnos://roteiro", move |ev| {
                    if let Ok(v) = serde_json::from_str::<Value>(ev.payload()) {
                        if v["modo"] == "mac" && v["id"].as_str().map(|s| !s.is_empty()).unwrap_or(false) {
                            let mut m = v.clone(); m["t"] = json!("roteiro-estado");
                            let _ = para_relay.send(m.to_string());
                        }
                    }
                });
                let mut pulso = tokio::time::interval(std::time::Duration::from_secs(25));
                loop {
                    tokio::select! {
                        Some(m) = fila.recv() => { if crate::olhar::pode_enviar(&app, &m) && tx.send(Message::Text(m.into())).await.is_err() { break; } }
                        _ = pulso.tick() => { if tx.send(Message::Text(json!({ "t": "ping" }).to_string().into())).await.is_err() { break; } }
                        msg = rx.next() => {
                            match msg {
                                Some(Ok(Message::Text(t))) => {
                                    if let Ok(v) = serde_json::from_str::<Value>(&t) {
                                        if v["t"] == "pronto-mac" { crate::olhar::conectar(&app, geracao, para_olhar.clone()); }
                                        if v["t"] == "olhar" || v["t"] == "sessao-fim" { crate::olhar::receber(&app, geracao, &v); }
                                        if v["t"] == "roteiro" {
                                            meu_chrome::registrar(&app, &format!("no-mac: roteiro {} ({})", v["id"].as_str().unwrap_or("?").chars().take(8).collect::<String>(), v["nome"].as_str().unwrap_or("")));
                                            let mut pedido = v.clone(); pedido["modo"] = json!("mac");
                                            let _ = app.emit("dnos://roteiro/executar", pedido);
                                        }
                                    }
                                }
                                Some(Ok(Message::Close(_))) | None => break,
                                Some(Err(_)) => break,
                                _ => {}
                            }
                        }
                    }
                    if estado.lock().map(|g| g.geracao != geracao).unwrap_or(true) { crate::olhar::desconectar(&app, geracao); let _ = tx.close().await; app.unlisten(ouvinte); return; }
                }
                crate::olhar::desconectar(&app, geracao);
                app.unlisten(ouvinte);
                meu_chrome::registrar(&app, "no-mac: conexão caiu, religando");
            }
            Err(e) => { meu_chrome::registrar(&app, &format!("no-mac: não conectei ({e}); tento de novo em {espera}s")); }
        }
        tokio::time::sleep(std::time::Duration::from_secs(espera)).await;
        espera = (espera * 2).min(120);
    }
}

pub fn instalar(app: &AppHandle) {
    app.manage::<ExecucaoCompartilhada>(Arc::new(Mutex::new(ExecucaoMac::default())));
    app.manage::<UsoCompartilhado>(Arc::new(Mutex::new(UsoComputador::default())));
    app.manage::<NoMacCompartilhado>(Arc::new(Mutex::new(NoMac::default())));
    // A janela pode perder o primeiro estado se o emit ocorrer durante a sua
    // criação. Quando o JavaScript avisa que está pronto, devolvemos o estado
    // guardado. Assim o botão Parar sempre chama a operação correta.
    let h = app.clone();
    app.listen_any("dnos://barra-mac/pronta", move |_| {
        let atual = h.try_state::<ExecucaoCompartilhada>()
            .and_then(|e| e.lock().ok().and_then(|g| g.barra_atual.clone()));
        if let Some(v) = atual { let _ = h.emit("dnos://barra-mac", v); }
    });
    let h = app.clone();
    app.listen_any("dnos://maquina/no", move |evento| {
        if !disponivel() { return; }
        let v: Value = serde_json::from_str(evento.payload()).unwrap_or(json!({}));
        let endereco = v["endereco"].as_str().unwrap_or("").trim().to_string();
        let token = v["token"].as_str().unwrap_or("").trim().to_string();
        let Some(estado) = h.try_state::<NoMacCompartilhado>() else { return };
        let estado = estado.inner().clone();
        let geracao = match estado.lock() {
            Ok(mut g) => {
                // Mesmo endereço e token: só renova, sem religar. Token novo ou sair (vazio): nova geração.
                if g.endereco == endereco && g.token == token && !token.is_empty() { return; }
                g.endereco = endereco; g.token = token; g.geracao += 1; g.geracao
            }
            Err(_) => return,
        };
        crate::olhar::invalidar(&h);
        let h2 = h.clone();
        tauri::async_runtime::spawn(async move { no_mac(h2, estado, geracao).await });
    });
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
pub async fn iniciar(app: AppHandle, estado: Compartilhado, nome: String, agente: String) {
    let _reserva = match reservar_uso(&app, "gravando") {
        Ok(r) => r,
        Err(e) => return gravador::emitir(&app, "erro", 0, None, Some(e)),
    };
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
        g.ativa = Some(Ativa { id: id.clone(), passos: 0, visao: Vec::new(), notas: tx_notas, parar: tx_parar });
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
    crate::voz::ligar(&app, &id);
    // Gravando no computador, a janela do dn.os fica atrás do app que a pessoa
    // está demonstrando — sem esta barra ela não tem onde parar (17/09).
    mostrar_barra(&app);
    falar_na_barra(&app, "assistindo", &agente, "Gravando", "0 passos · fale para anotar".into(), false);

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
                        let _ = app.emit("dnos://gravador/passo", json!({ "id": id, "passo": p }));
                        passos.push(p);
                        if let Ok(mut g) = estado.lock() { if let Some(a) = g.ativa.as_mut() { a.passos = passos.len(); } }
                        gravador::emitir(&app, "gravando", passos.len(), Some(&id), None);
                        falar_na_barra(&app, "assistindo", &agente, "Gravando", format!("{} passos · fale para anotar", passos.len()), false);
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
    esconder_barra(&app);

    let Ok(mut guarda) = estado.lock() else { return };
    while let Ok(nota) = rx_notas.try_recv() { notas.push(nota); }
    let visao = guarda.ativa.as_ref().map(|a| a.visao.clone()).unwrap_or_default();
    let quadros = passos.iter().filter(|p| p["quadro"].is_string()).count();
    let gravacao = json!({
        "captura": { "quadros": quadros, "limite_quadros": 80, "limitada": quadros >= 80 },
        "visao": visao,
        "id": id, "nome": nome, "inicio": inicio, "fim": gravador::agora_ms(), "modo": "mac",
        "instancia": std::env::var("DNOS_URL").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "https://dnos.dnia.ai".into()),
        "criterio": criterio, "passos": passos, "notas": notas, "encerrada_por": motivo_fim,
        "permissoes": permissoes_no_inicio,
    });
    if let Some(p) = gravador::pasta(&app) {
        let _ = std::fs::write(p.join(format!("{id}.json")), gravacao.to_string());
    }
    guarda.ativa = None;
    drop(guarda);
    let n = gravacao["passos"].as_array().map(|a| a.len()).unwrap_or(0);
    gravador::emitir(&app, "parado", n, Some(&id), Some(motivo_fim.clone()));
    let _ = app.emit("dnos://gravador/pronta", json!({ "gravacao": gravacao }));
}
