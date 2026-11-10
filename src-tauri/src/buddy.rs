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
//!                    → `dnos://buddy/balao` { aberto }          (cresce para o balão caber)
//!   casca → principal `dnos://buddy/abrir-na-dnos`

use serde_json::{json, Value};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Listener, LogicalPosition, LogicalSize, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

const JANELA: &str = "buddy";
const ARQUIVO: &str = "buddy-posicao.json";
/// Só o buddy, e com o balão ao lado (lógico). O canto de baixo à direita fica parado.
const SO_O_BUDDY: (f64, f64) = (136.0, 124.0);
const COM_BALAO: (f64, f64) = (420.0, 210.0);
const MARGEM: f64 = 24.0;

#[derive(Default)]
struct Estado {
    disponivel: bool,
    principal_focada: bool,
    escondido_ate_ms: u64,
    balao: bool,
    /// Canto de baixo à direita, onde a pessoa deixou (lógico).
    canto: Option<(f64, f64)>,
}

fn agora_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn caminho(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path().app_data_dir().ok().map(|p| p.join(ARQUIVO))
}

fn salvar_canto(app: &AppHandle, canto: (f64, f64)) {
    if let Some(p) = caminho(app) {
        let _ = std::fs::create_dir_all(p.parent().unwrap_or(std::path::Path::new(".")));
        let _ = std::fs::write(p, json!({ "canto": [canto.0, canto.1] }).to_string());
    }
}

fn carregar_canto(app: &AppHandle) -> Option<(f64, f64)> {
    let v: Value = caminho(app).and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok())?;
    Some((v["canto"][0].as_f64()?, v["canto"][1].as_f64()?))
}

/// Puro: deve mostrar a janela agora?
pub(crate) fn deve_mostrar(disponivel: bool, principal_focada: bool, agora: u64, escondido_ate: u64) -> bool {
    disponivel && !principal_focada && agora >= escondido_ate
}

/// Puro: canto de baixo à direita → posição da janela (lógico).
pub(crate) fn posicao_pelo_canto(canto: (f64, f64), tamanho: (f64, f64)) -> (f64, f64) {
    ((canto.0 - tamanho.0).max(0.0), (canto.1 - tamanho.1).max(0.0))
}

fn tamanho_atual(balao: bool) -> (f64, f64) {
    if balao { COM_BALAO } else { SO_O_BUDDY }
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
    let (canto, balao) = match st.lock() { Ok(g) => (g.canto, g.balao), Err(_) => return };
    let canto = canto.unwrap_or_else(|| canto_padrao(app));
    let tam = tamanho_atual(balao);
    let (x, y) = posicao_pelo_canto(canto, tam);
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
        .build()
        .ok()?;
    // Onde a pessoa soltar fica guardado (o canto de baixo à direita, para o balão crescer para a esquerda e para cima).
    let h = app.clone();
    w.on_window_event(move |ev| {
        if let WindowEvent::Moved(p) = ev {
            let Some(w) = h.get_webview_window(JANELA) else { return };
            let escala = w.scale_factor().unwrap_or(1.0);
            let pos = p.to_logical::<f64>(escala);
            let Some(st) = h.try_state::<Mutex<Estado>>() else { return };
            let canto = match st.lock() {
                Ok(mut g) => {
                    let tam = tamanho_atual(g.balao);
                    let canto = (pos.x + tam.0, pos.y + tam.1);
                    g.canto = Some(canto);
                    canto
                }
                Err(_) => return,
            };
            salvar_canto(&h, canto);
        }
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

fn ler(evento: &tauri::Event) -> Value {
    serde_json::from_str::<Value>(evento.payload()).unwrap_or(Value::Null)
}

pub fn instalar(app: &AppHandle) {
    let canto = carregar_canto(app);
    app.manage(Mutex::new(Estado { principal_focada: true, canto, ..Default::default() }));

    let h = app.clone();
    app.listen_any("dnos://buddy/disponivel", move |e| {
        let ligado = ler(&e)["ligado"].as_bool().unwrap_or(false);
        if let Some(st) = h.try_state::<Mutex<Estado>>() {
            if let Ok(mut g) = st.lock() { g.disponivel = ligado; }
        }
        atualizar(&h);
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
        atualizar(&h);
        // Passado o tempo, volta sozinho (se ainda fizer sentido mostrar).
        let h2 = h.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_secs(minutos * 60 + 1)).await;
            atualizar(&h2);
        });
    });

    let h = app.clone();
    app.listen_any("dnos://buddy/balao", move |e| {
        let aberto = ler(&e)["aberto"].as_bool().unwrap_or(false);
        if let Some(st) = h.try_state::<Mutex<Estado>>() {
            if let Ok(mut g) = st.lock() { g.balao = aberto; }
        }
        if let Some(w) = h.get_webview_window(JANELA) { posicionar(&h, &w); }
    });

    // Na janela principal, o buddy de dentro assume; fora dela, este aparece.
    if let Some(main) = app.get_webview_window("main") {
        let h = app.clone();
        main.on_window_event(move |ev| {
            if let WindowEvent::Focused(focada) = ev {
                if let Some(st) = h.try_state::<Mutex<Estado>>() {
                    if let Ok(mut g) = st.lock() { g.principal_focada = *focada; }
                }
                atualizar(&h);
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
    fn o_canto_de_baixo_a_direita_fica_parado() {
        assert_eq!(posicao_pelo_canto((1000.0, 800.0), SO_O_BUDDY), (1000.0 - 136.0, 800.0 - 124.0));
        assert_eq!(posicao_pelo_canto((1000.0, 800.0), COM_BALAO), (1000.0 - 420.0, 800.0 - 210.0));
        assert_eq!(posicao_pelo_canto((50.0, 50.0), COM_BALAO), (0.0, 0.0));
    }
}
