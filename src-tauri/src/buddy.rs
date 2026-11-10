//! Buddy fora do dn.os (fase 4, 09/11/2026 — docs/buddy.md no repositório do dn.os).
//!
//! O buddy é o assistente pessoal de cada pessoa. Dentro do dn.os ele fica num canto da tela
//! (BuddyFlutuante). Aqui, quando a pessoa sai do dn.os para outro app, ele aparece numa janela
//! pequena, sem moldura e sempre por cima, como a barra dos minds na reunião; quando ela volta
//! para o dn.os, a janela some (o de dentro assume).
//!
//! A janela carrega a página `/buddy-janela` da própria instância: o mesmo personagem e os
//! mesmos sinais do app (nada desenhado duas vezes). Quem decide tamanho, lugar e quando
//! aparece é esta casca. Eventos:
//!   página principal → `dnos://buddy/disponivel` { ligado }   (buddy pronto e a pessoa quer ver)
//!   janela do buddy  → `dnos://buddy/abrir`                     (foca o dn.os e abre a conversa)
//!                    → `dnos://buddy/esconder` { minutos }      (some por um tempo)
//!                    → `dnos://buddy/balao` { aberto }          (cresce para o balão caber; 0.8.24)
//!                    → `dnos://buddy/tamanho` { modo }          (0.8.25: "so" | "balao" | "chat")
//!
//! 0.8.25 (10/11, pedido do Rodrigo): o clique no buddy abre um chat ali mesmo, e a janela cresce
//! para ele (e ganha o foco, para dar para digitar). "Abrir no dn.os" segue pelo `abrir`.
//!   casca → principal `dnos://buddy/abrir-na-dnos`

use serde_json::{json, Value};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Listener, LogicalPosition, LogicalSize, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

const JANELA: &str = "buddy";
const ARQUIVO: &str = "buddy-posicao.json";
/// Só o buddy, com o balão ou o menu em cima dele, e com o chat (lógico). O canto de baixo à direita fica parado.
const SO_O_BUDDY: (f64, f64) = (136.0, 124.0);
const COM_BALAO: (f64, f64) = (300.0, 320.0);
const COM_CHAT: (f64, f64) = (400.0, 600.0);
/// 0.8.26 (10/11): o chat aumenta pela borda (de cima, da esquerda e o canto) e tem o "Ampliar".
const CHAT_MINIMO: (f64, f64) = (340.0, 420.0);
const CHAT_AMPLIADO: (f64, f64) = (660.0, 800.0);

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub(crate) enum Modo {
    #[default]
    So,
    Balao,
    Chat,
}

/// Puro: o nome que a página manda → o modo (desconhecido = só o buddy).
pub(crate) fn modo_pelo_nome(nome: &str) -> Modo {
    match nome {
        "balao" => Modo::Balao,
        "chat" => Modo::Chat,
        _ => Modo::So,
    }
}
const MARGEM: f64 = 24.0;
/// A página da janela do buddy sabe o que esta casca faz (a janela não recebe o script da principal).
const SCRIPT_DA_JANELA: &str = r#"window.__DNOS_DESKTOP__ = Object.assign(window.__DNOS_DESKTOP__ || {}, { versao: "__DNOS_VERSAO__", buddyFora: true, buddyChat: true, buddyRedimensionar: true, janelaDoBuddy: true });"#;

#[derive(Default)]
struct Estado {
    disponivel: bool,
    principal_focada: bool,
    escondido_ate_ms: u64,
    modo: Modo,
    /// A página já fala `tamanho` (0.8.25+): o `balao` antigo, que ela manda junto, é ignorado.
    entende_tamanho: bool,
    /// Canto de baixo à direita, onde a pessoa deixou (lógico).
    canto: Option<(f64, f64)>,
    /// Tamanho do chat que a pessoa deixou arrastando a borda (lógico).
    chat: Option<(f64, f64)>,
    /// "Ampliar" ligado: o chat grande, sem mexer no tamanho guardado.
    ampliado: bool,
}

fn agora_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn caminho(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path().app_data_dir().ok().map(|p| p.join(ARQUIVO))
}

fn salvar(app: &AppHandle, canto: Option<(f64, f64)>, chat: Option<(f64, f64)>) {
    if let Some(p) = caminho(app) {
        let _ = std::fs::create_dir_all(p.parent().unwrap_or(std::path::Path::new(".")));
        let par = |v: Option<(f64, f64)>| v.map(|(a, b)| json!([a, b])).unwrap_or(Value::Null);
        let _ = std::fs::write(p, json!({ "canto": par(canto), "chat": par(chat) }).to_string());
    }
}

/// (canto, tamanho do chat) guardados.
fn carregar(app: &AppHandle) -> (Option<(f64, f64)>, Option<(f64, f64)>) {
    let v: Value = caminho(app)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null);
    let par = |k: &str| Some((v[k][0].as_f64()?, v[k][1].as_f64()?));
    (par("canto"), par("chat").map(limitar_chat))
}

/// Puro: o chat nunca fica menor que o mínimo (o layout do chat quebra abaixo disso).
pub(crate) fn limitar_chat(t: (f64, f64)) -> (f64, f64) {
    (t.0.max(CHAT_MINIMO.0), t.1.max(CHAT_MINIMO.1))
}

/// Puro: o tamanho da janela agora. No chat: ampliado, o que a pessoa deixou, ou o padrão.
pub(crate) fn tamanho_do_estado(modo: Modo, chat: Option<(f64, f64)>, ampliado: bool) -> (f64, f64) {
    match modo {
        Modo::Chat if ampliado => CHAT_AMPLIADO,
        Modo::Chat => chat.map(limitar_chat).unwrap_or(COM_CHAT),
        outro => tamanho_do_modo(outro),
    }
}

/// Puro: deve mostrar a janela agora?
pub(crate) fn deve_mostrar(disponivel: bool, principal_focada: bool, agora: u64, escondido_ate: u64) -> bool {
    disponivel && !principal_focada && agora >= escondido_ate
}

/// Puro: canto de baixo à direita → posição da janela (lógico).
pub(crate) fn posicao_pelo_canto(canto: (f64, f64), tamanho: (f64, f64)) -> (f64, f64) {
    ((canto.0 - tamanho.0).max(0.0), (canto.1 - tamanho.1).max(0.0))
}

pub(crate) fn tamanho_do_modo(modo: Modo) -> (f64, f64) {
    match modo {
        Modo::So => SO_O_BUDDY,
        Modo::Balao => COM_BALAO,
        Modo::Chat => COM_CHAT,
    }
}

/// Canto padrão: embaixo à direita do monitor principal, acima da barra de tarefas/Dock.
fn canto_padrao(app: &AppHandle) -> (f64, f64) {
    if let Ok(Some(m)) = app.primary_monitor() {
        let escala = m.scale_factor();
        let tam = m.size().to_logical::<f64>(escala);
        let pos = m.position().to_logical::<f64>(escala);
        return (pos.x + tam.width - MARGEM, pos.y + tam.height - MARGEM * 3.0);
    }
    (1200.0, 760.0)
}

fn posicionar(app: &AppHandle, w: &WebviewWindow) {
    let Some(st) = app.try_state::<Mutex<Estado>>() else { return };
    let (canto, modo, chat, ampliado) = match st.lock() { Ok(g) => (g.canto, g.modo, g.chat, g.ampliado), Err(_) => return };
    let canto = canto.unwrap_or_else(|| canto_padrao(app));
    let tam = tamanho_do_estado(modo, chat, ampliado);
    let (x, y) = posicao_pelo_canto(canto, tam);
    // Só o chat (não ampliado) aumenta pela borda; o buddy sozinho e o balão têm tamanho fixo.
    let redimensiona = modo == Modo::Chat && !ampliado;
    let _ = w.set_resizable(redimensiona);
    let _ = w.set_min_size(if redimensiona { Some(LogicalSize::new(CHAT_MINIMO.0, CHAT_MINIMO.1)) } else { None });
    let _ = w.set_size(LogicalSize::new(tam.0, tam.1));
    let _ = w.set_position(LogicalPosition::new(x, y));
}

fn criar(app: &AppHandle) -> Option<WebviewWindow> {
    if let Some(w) = app.get_webview_window(JANELA) { return Some(w); }
    let url = url::Url::parse(&format!("{}/buddy-janela", crate::url_da_instancia())).ok()?;
    let tam = SO_O_BUDDY;
    let w = WebviewWindowBuilder::new(app, JANELA, WebviewUrl::External(url))
        .title("Buddy")
        .inner_size(tam.0, tam.1)
        .decorations(false)
        // Transparente e sem a sombra do sistema, como a barra-mac (macOSPrivateApi no tauri.conf).
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .resizable(false)
        .skip_taskbar(true)
        .focused(false)
        .visible(false)
        .initialization_script(&SCRIPT_DA_JANELA.replace("__DNOS_VERSAO__", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;
    // Onde a pessoa soltar fica guardado (o canto de baixo à direita, para o balão crescer para a esquerda e
    // para cima), e o tamanho do chat que ela deixou arrastando a borda. Lido da janela de verdade: arrastar
    // a borda de cima ou da esquerda mexe na posição e no tamanho ao mesmo tempo.
    let h = app.clone();
    w.on_window_event(move |ev| {
        let redimensionou = matches!(ev, WindowEvent::Resized(_));
        if !redimensionou && !matches!(ev, WindowEvent::Moved(_)) { return; }
        let Some(w) = h.get_webview_window(JANELA) else { return };
        let escala = w.scale_factor().unwrap_or(1.0);
        let (Ok(p), Ok(t)) = (w.outer_position(), w.inner_size()) else { return };
        let (pos, tam) = (p.to_logical::<f64>(escala), t.to_logical::<f64>(escala));
        let Some(st) = h.try_state::<Mutex<Estado>>() else { return };
        let (canto, chat) = match st.lock() {
            Ok(mut g) => {
                g.canto = Some((pos.x + tam.width, pos.y + tam.height));
                if redimensionou && g.modo == Modo::Chat && !g.ampliado {
                    g.chat = Some(limitar_chat((tam.width, tam.height)));
                }
                (g.canto, g.chat)
            }
            Err(_) => return,
        };
        salvar(&h, canto, chat);
    });
    Some(w)
}

fn atualizar(app: &AppHandle) {
    let Some(st) = app.try_state::<Mutex<Estado>>() else { return };
    let mostrar = match st.lock() {
        Ok(g) => deve_mostrar(g.disponivel, g.principal_focada, agora_ms(), g.escondido_ate_ms),
        Err(_) => return,
    };
    if mostrar {
        if let Some(w) = criar(app) {
            posicionar(app, &w);
            let _ = w.show();
        }
    } else if let Some(w) = app.get_webview_window(JANELA) {
        let _ = w.hide();
    }
}

/// Toda criação e troca de visibilidade passa pela thread principal, uma de cada vez.
/// 10/11: o aviso da página (`disponivel`, numa thread do runtime) e o foco da janela principal
/// (na thread principal) chegavam juntos; os dois viam "ainda não existe janela" e criavam cada um
/// a sua. A segunda ficava órfã, parada e sempre visível: o buddy aparecia duplicado.
fn atualizar_na_principal(app: &AppHandle) {
    let h = app.clone();
    let _ = app.run_on_main_thread(move || atualizar(&h));
}

fn ler(evento: &tauri::Event) -> Value {
    serde_json::from_str::<Value>(evento.payload()).unwrap_or(Value::Null)
}

pub fn instalar(app: &AppHandle) {
    let (canto, chat) = carregar(app);
    app.manage(Mutex::new(Estado { principal_focada: true, canto, chat, ..Default::default() }));

    let h = app.clone();
    app.listen_any("dnos://buddy/disponivel", move |e| {
        let ligado = ler(&e)["ligado"].as_bool().unwrap_or(false);
        if let Some(st) = h.try_state::<Mutex<Estado>>() {
            if let Ok(mut g) = st.lock() { g.disponivel = ligado; }
        }
        atualizar_na_principal(&h);
    });

    let h = app.clone();
    app.listen_any("dnos://buddy/abrir", move |_| {
        if let Some(main) = h.get_webview_window("main") {
            let _ = main.unminimize();
            let _ = main.show();
            let _ = main.set_focus();
        }
        let _ = h.emit_to("main", "dnos://buddy/abrir-na-dnos", ());
    });

    let h = app.clone();
    app.listen_any("dnos://buddy/esconder", move |e| {
        let minutos = ler(&e)["minutos"].as_u64().unwrap_or(60).clamp(1, 24 * 60);
        if let Some(st) = h.try_state::<Mutex<Estado>>() {
            if let Ok(mut g) = st.lock() { g.escondido_ate_ms = agora_ms() + minutos * 60_000; }
        }
        atualizar_na_principal(&h);
        // Passado o tempo, volta sozinho (se ainda fizer sentido mostrar).
        let h2 = h.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(minutos * 60 + 1)).await;
            atualizar_na_principal(&h2);
        });
    });

    let h = app.clone();
    app.listen_any("dnos://buddy/balao", move |e| {
        let aberto = ler(&e)["aberto"].as_bool().unwrap_or(false);
        if let Some(st) = h.try_state::<Mutex<Estado>>() {
            if let Ok(mut g) = st.lock() {
                if g.entende_tamanho { return; }
                g.modo = if aberto { Modo::Balao } else { Modo::So };
            }
        }
        if let Some(w) = h.get_webview_window(JANELA) { posicionar(&h, &w); }
    });

    let h = app.clone();
    app.listen_any("dnos://buddy/tamanho", move |e| {
        let carga = ler(&e);
        let modo = modo_pelo_nome(carga["modo"].as_str().unwrap_or("so"));
        let ampliado = modo == Modo::Chat && carga["ampliado"].as_bool().unwrap_or(false);
        if let Some(st) = h.try_state::<Mutex<Estado>>() {
            if let Ok(mut g) = st.lock() { g.entende_tamanho = true; g.modo = modo; g.ampliado = ampliado; }
        }
        if let Some(w) = h.get_webview_window(JANELA) {
            posicionar(&h, &w);
            // Para digitar no chat, a janela precisa do foco (ela nasce sem roubar o foco de ninguém).
            if modo == Modo::Chat { let _ = w.set_focus(); }
        }
    });

    // Na janela principal, o buddy de dentro assume; fora dela, este aparece.
    if let Some(main) = app.get_webview_window("main") {
        let h = app.clone();
        main.on_window_event(move |ev| {
            if let WindowEvent::Focused(focada) = ev {
                if let Some(st) = h.try_state::<Mutex<Estado>>() {
                    if let Ok(mut g) = st.lock() { g.principal_focada = *focada; }
                }
                atualizar_na_principal(&h);
            }
        });
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn mostra_so_fora_do_dnos_e_fora_do_tempo_escondido() {
        assert!(deve_mostrar(true, false, 10, 0));
        assert!(!deve_mostrar(true, true, 10, 0), "com o dn.os na frente, o buddy de dentro assume");
        assert!(!deve_mostrar(false, false, 10, 0), "sem buddy pronto, nada");
        assert!(!deve_mostrar(true, false, 10, 20), "escondido por um tempo");
    }

    #[test]
    fn modos_e_tamanhos() {
        assert_eq!(modo_pelo_nome("chat"), Modo::Chat);
        assert_eq!(modo_pelo_nome("balao"), Modo::Balao);
        assert_eq!(modo_pelo_nome("qualquer"), Modo::So, "desconhecido = só o buddy");
        let (so, balao, chat) = (tamanho_do_modo(Modo::So), tamanho_do_modo(Modo::Balao), tamanho_do_modo(Modo::Chat));
        assert!(balao.0 > so.0 && balao.1 > so.1);
        assert!(chat.0 >= 380.0 && chat.1 >= 520.0, "o chat precisa de espaço para ler e escrever");
        // O menu (240 de largura) e o buddy cabem um em cima do outro na janela do balão.
        assert!(balao.0 >= 260.0 && balao.1 >= 300.0);
    }

    #[test]
    fn o_chat_aumenta_mas_nao_encolhe_demais() {
        assert_eq!(tamanho_do_estado(Modo::Chat, None, false), COM_CHAT);
        assert_eq!(tamanho_do_estado(Modo::Chat, Some((520.0, 700.0)), false), (520.0, 700.0), "o tamanho que a pessoa deixou");
        assert_eq!(tamanho_do_estado(Modo::Chat, Some((100.0, 100.0)), false), CHAT_MINIMO, "nunca abaixo do mínimo");
        assert_eq!(tamanho_do_estado(Modo::Chat, Some((520.0, 700.0)), true), CHAT_AMPLIADO, "ampliado não perde o guardado");
        assert_eq!(tamanho_do_estado(Modo::So, Some((520.0, 700.0)), true), SO_O_BUDDY);
        assert_eq!(tamanho_do_estado(Modo::Balao, None, false), COM_BALAO);
    }

    #[test]
    fn o_canto_de_baixo_a_direita_fica_parado() {
        assert_eq!(posicao_pelo_canto((1000.0, 800.0), SO_O_BUDDY), (1000.0 - 136.0, 800.0 - 124.0));
        assert_eq!(posicao_pelo_canto((1000.0, 800.0), COM_BALAO), (1000.0 - 300.0, 800.0 - 320.0));
        assert_eq!(posicao_pelo_canto((1000.0, 800.0), COM_CHAT), (1000.0 - 400.0, 800.0 - 600.0));
        assert_eq!(posicao_pelo_canto((50.0, 50.0), COM_BALAO), (0.0, 0.0));
    }
}
