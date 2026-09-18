//! Olhar: autorização efêmera para UM agente e UM app. Fotos nunca passam por
//! eventos da webview nem por disco; só pelo socket autenticado que pediu a leitura.
use serde_json::{json, Value};
use std::{io::Read, process::{Command, Stdio}, sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};
use tauri::{AppHandle, Manager};
use tokio::sync::mpsc::UnboundedSender;
use crate::maquina::{self, ReservaDeUso};

#[cfg(target_os = "macos")]
const AJUDANTE: &[u8] = include_bytes!("../ajudantes/dnos-olhar-mac");
#[cfg(not(target_os = "macos"))]
const AJUDANTE: &[u8] = &[];
const LIMITE: u64 = 3_000_000;

struct Permissao {
    agente: String, nome: String, bundle: String, ate: u64, sessao: Option<String>,
    leituras: u32, ocupada: bool, cancelada: Arc<AtomicBool>, _reserva: ReservaDeUso,
}
#[derive(Default)]
pub struct Estado { conexao: Option<(u64, UnboundedSender<String>)>, permissao: Option<Permissao> }
type Compartilhado = Arc<Mutex<Estado>>;
fn agora() -> u64 { crate::gravador::agora_ms() as u64 }
fn app_permitido(bundle: &str) -> bool { matches!(bundle, "com.lemon.lvoverseas" | "com.apple.finder") }
fn enviar(g: &Estado, v: Value) { if let Some((_, tx)) = &g.conexao { let _ = tx.send(v.to_string()); } }
fn encerrar(app: &AppHandle, g: &mut Estado, motivo: &str) {
    if let Some(p) = g.permissao.take() {
        p.cancelada.store(true, Ordering::SeqCst);
        if let Some(sessao) = &p.sessao { enviar(g, json!({"t":"sessao-fim","sessao":sessao,"motivo":motivo})); }
        enviar(g, json!({"t":"olhar-permissao","permitido":false}));
        maquina::esconder_barra(app);
        // A reserva só é liberada depois que a barra antiga foi removida.
        drop(p);
    }
}
fn estado(g: &Estado) -> Value {
    match &g.permissao {
        Some(p) => json!({"disponivel":!AJUDANTE.is_empty(),"conectado":g.conexao.is_some(),"ativo":true,"agente":p.agente,"nome":p.nome,"bundle":p.bundle,"expira_em":p.ate}),
        None => json!({"disponivel":!AJUDANTE.is_empty(),"conectado":g.conexao.is_some(),"ativo":false}),
    }
}

/// Comando restrito à janela principal; a barra só pode parar/consultar.
/// Consentimento vem do clique na interface, nunca de um verbo remoto do relay.
#[tauri::command]
pub async fn olhar_computador(app: AppHandle, window: tauri::WebviewWindow, acao: String,
    agente: Option<String>, nome: Option<String>, bundle: Option<String>) -> Result<Value, String> {
    if window.label() != "main" && !(window.label() == "barra-mac" && acao == "parar") { return Err("janela_nao_autorizada".into()); }
    if acao == "autorizar" {
        let atual = window.url().map_err(|_| "origem_indisponivel")?;
        let instancia = url::Url::parse(&crate::url_da_instancia()).map_err(|_| "instancia_invalida")?;
        if atual.origin() != instancia.origin() { return Err("origem_nao_autorizada".into()); }
    }
    let e = app.state::<Compartilhado>();
    let mut g = e.lock().map_err(|_| "observacao_indisponivel")?;
    if g.permissao.as_ref().map(|p| agora() >= p.ate).unwrap_or(false) { encerrar(&app, &mut g, "tempo_esgotado"); }
    match acao.as_str() {
        "estado" => {},
        "parar" => encerrar(&app, &mut g, "pessoa"),
        "autorizar" => {
            if AJUDANTE.is_empty() { return Err("Olhar está disponível no macOS nesta primeira etapa.".into()); }
            if g.conexao.is_none() { return Err("A conexão do computador ainda não está pronta. Tente novamente.".into()); }
            if g.permissao.is_some() { return Err("Encerre a observação atual antes de iniciar outra.".into()); }
            let agente = agente.filter(|s| !s.is_empty() && s.len() <= 100).ok_or("agente_obrigatorio")?;
            let nome = nome.unwrap_or_else(|| agente.clone()).chars().take(100).collect::<String>();
            let bundle = bundle.filter(|s| app_permitido(s)).ok_or("aplicativo_nao_autorizado")?;
            let reserva = maquina::reservar_uso(&app, "olhando")?;
            let ate = agora() + 300_000;
            g.permissao = Some(Permissao { agente: agente.clone(), nome: nome.clone(), bundle: bundle.clone(), ate, sessao: None,
                leituras: 0, ocupada: false, cancelada: Arc::new(AtomicBool::new(false)), _reserva: reserva });
            maquina::mostrar_barra(&app);
            // Sem barra visível não existe sessão silenciosa de reserva.
            if app.get_webview_window("barra-mac").is_none() { encerrar(&app, &mut g, "barra_indisponivel"); return Err("Não foi possível abrir a barra de observação.".into()); }
            maquina::falar_na_barra(&app, "olhando", &nome, if bundle == "com.apple.finder" {"Finder"} else {"CapCut"}, "Somente leitura · até 5 minutos · Parar encerra o acesso".into(), false);
            enviar(&g, json!({"t":"olhar-permissao","permitido":true,"agente":agente,"bundle":bundle,"expira_em":ate}));
        },
        _ => return Err("acao_desconhecida".into()),
    }
    Ok(estado(&g))
}

pub fn conectar(app: &AppHandle, geracao: u64, tx: UnboundedSender<String>) {
    // Um socket antigo que terminou o login depois de uma troca não reassume.
    if app.state::<maquina::NoMacCompartilhado>().lock().map(|n| n.geracao != geracao).unwrap_or(true) { return; }
    if let Ok(mut g) = app.state::<Compartilhado>().lock() {
        encerrar(app, &mut g, "conexao_substituida"); g.conexao = Some((geracao, tx));
    }
}
pub fn desconectar(app: &AppHandle, geracao: u64) {
    if let Ok(mut g) = app.state::<Compartilhado>().lock() {
        if g.conexao.as_ref().map(|c| c.0) == Some(geracao) { encerrar(app, &mut g, "computador_desconectado"); g.conexao = None; }
    }
}
pub fn invalidar(app: &AppHandle) {
    if let Ok(mut g) = app.state::<Compartilhado>().lock() { encerrar(app, &mut g, "identidade_alterada"); g.conexao = None; }
}

/// A fila de envio pode ainda conter uma foto quando a pessoa aperta Parar.
/// Conferir no último ponto antes da rede, além de conferir no fim da captura.
pub fn pode_enviar(app: &AppHandle, raw: &str) -> bool {
    let Ok(v) = serde_json::from_str::<Value>(raw) else { return false };
    if v["t"] != "olhar-resposta" || v["ok"] != true { return true; }
    app.state::<Compartilhado>().lock().map(|g| g.permissao.as_ref().map(|p|
        p.ate > agora() && !p.cancelada.load(Ordering::SeqCst) && p.sessao.as_deref() == v["sessao"].as_str()
    ).unwrap_or(false)).unwrap_or(false)
}

fn correlacao(v: &Value, campo: &str) -> Option<String> {
    v[campo].as_str().filter(|s| s.len() == 36 && s.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-')).map(str::to_owned)
}
pub fn receber(app: &AppHandle, geracao: u64, v: &Value) {
    let e = app.state::<Compartilhado>().inner().clone();
    let Ok(mut g) = e.lock() else { return };
    if g.conexao.as_ref().map(|c| c.0) != Some(geracao) { return; }
    let Some(sessao) = correlacao(v, "sessao") else { return };
    if v["t"] == "sessao-fim" {
        if g.permissao.as_ref().and_then(|p| p.sessao.as_ref()) == Some(&sessao) { encerrar(app, &mut g, "relay_encerrou"); }
        return;
    }
    let Some(requisicao) = correlacao(v, "requisicao") else { return };
    let p = match g.permissao.as_mut() {
        Some(p) if p.ate > agora() && !p.cancelada.load(Ordering::SeqCst) && Some(p.agente.as_str()) == v["agente"].as_str()
            && p.sessao.as_ref().map(|s| s == &sessao).unwrap_or(true) && p.leituras < 30 && !p.ocupada => p,
        _ => { enviar(&g, json!({"t":"olhar-resposta","sessao":sessao,"requisicao":requisicao,"ok":false,"motivo":"olhar_nao_autorizado_ou_ocupado"})); return; }
    };
    p.sessao = Some(sessao.clone()); p.leituras += 1; p.ocupada = true;
    let cancelada = p.cancelada.clone(); let bundle = p.bundle.clone();
    let h = app.clone(); drop(g);
    std::thread::spawn(move || {
        let resultado = capturar(&h, &bundle, &cancelada);
        if let Ok(mut g) = e.lock() {
            // Revalidar DEPOIS do processo: nunca enviar o resultado de uma
            // permissão cancelada ou de um socket substituído.
            if g.conexao.as_ref().map(|c| c.0) != Some(geracao) { return; }
            let Some(p) = g.permissao.as_mut() else { return };
            if !Arc::ptr_eq(&p.cancelada, &cancelada) || cancelada.load(Ordering::SeqCst) || p.ate <= agora() { return; }
            p.ocupada = false;
            let msg = match resultado {
                Ok(leitura) if leitura["ok"] == true && leitura["consistente"] == true => json!({"t":"olhar-resposta","sessao":sessao,"requisicao":requisicao,"ok":true,"leitura":leitura}),
                Ok(leitura) => json!({"t":"olhar-resposta","sessao":sessao,"requisicao":requisicao,"ok":false,"motivo":leitura["motivo"].as_str().unwrap_or("leitura_inconsistente")}),
                Err(motivo) => json!({"t":"olhar-resposta","sessao":sessao,"requisicao":requisicao,"ok":false,"motivo":motivo}),
            };
            enviar(&g, msg);
        }
    });
}

fn capturar(app: &AppHandle, bundle: &str, cancelada: &AtomicBool) -> Result<Value, String> {
    if AJUDANTE.is_empty() { return Err("sistema_nao_suportado".into()); }
    let pasta = app.path().app_data_dir().map_err(|_| "pasta_indisponivel")?.join("ajudantes");
    std::fs::create_dir_all(&pasta).map_err(|_| "pasta_indisponivel")?;
    let arq = pasta.join(format!("dnos-olhar-mac-{}", app.package_info().version));
    if std::fs::read(&arq).ok().as_deref() != Some(AJUDANTE) {
        std::fs::write(&arq, AJUDANTE).map_err(|_| "ajudante_indisponivel")?;
        #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&arq, std::fs::Permissions::from_mode(0o700)).map_err(|_| "ajudante_indisponivel")?; }
    }
    if cancelada.load(Ordering::SeqCst) { return Err("pessoa".into()); }
    let mut filho = Command::new(arq).args(["--app", bundle]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|_| "ajudante_nao_iniciou")?;
    let stdout = filho.stdout.take().ok_or("saida_indisponivel")?;
    let leitor = std::thread::spawn(move || { let mut dados = Vec::new(); stdout.take(LIMITE + 1).read_to_end(&mut dados).map(|_| dados) });
    let inicio = Instant::now();
    let motivo = loop {
        if cancelada.load(Ordering::SeqCst) { break Some("pessoa"); }
        if inicio.elapsed() > Duration::from_secs(10) { break Some("captura_sem_resposta"); }
        match filho.try_wait() { Ok(Some(status)) => break if status.success() {None} else {Some("ajudante_falhou")}, Err(_) => break Some("ajudante_falhou"), _ => {} }
        std::thread::sleep(Duration::from_millis(30));
    };
    let _ = filho.kill(); let _ = filho.wait();
    let dados = leitor.join().map_err(|_| "saida_indisponivel")?.map_err(|_| "saida_indisponivel")?;
    if let Some(m) = motivo { return Err(m.into()); }
    if dados.len() as u64 > LIMITE { return Err("leitura_muito_grande".into()); }
    serde_json::from_slice(&dados).map_err(|_| "leitura_invalida".into())
}

pub fn instalar(app: &AppHandle) {
    app.manage::<Compartilhado>(Arc::new(Mutex::new(Estado::default())));
    let h = app.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(200));
        if let Ok(mut g) = h.state::<Compartilhado>().lock() {
            if g.permissao.as_ref().map(|p| p.ate <= agora()).unwrap_or(false) { encerrar(&h, &mut g, "tempo_esgotado"); }
        }
    });
}

#[cfg(test)]
mod testes {
    use super::*;
    #[test] fn somente_apps_do_piloto() { assert!(app_permitido("com.apple.finder")); assert!(app_permitido("com.lemon.lvoverseas")); assert!(!app_permitido("com.apple.keychainaccess")); assert!(!app_permitido("")); }
    #[test] fn correlacao_nao_aceita_id_de_roteiro_nem_texto_livre() { assert!(correlacao(&json!({"id":"qualquer"}), "sessao").is_none()); assert!(correlacao(&json!({"sessao":"../arquivo"}), "sessao").is_none()); }
}
