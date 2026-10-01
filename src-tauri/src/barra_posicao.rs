//! Barra flutuante: arrastar e minimizar (01/10/2026, pedido do Rodrigo).
//!
//! Na reunião a barra ficava parada no topo, por cima do que a pessoa estava
//! vendo. Agora:
//! - arrasta para qualquer lugar (a página chama `startDragging` da janela);
//! - na reunião (modos `reuniao` e `meet`), minimiza numa bolinha no canto
//!   direito com a foto do agente e o anel de gravando; um clique volta;
//! - onde a pessoa soltar a barra (ou a bolinha) fica guardado para a próxima.
//!
//! A página só avisa (`dnos://barra-mac/minimizar` e `/restaurar`); o tamanho e
//! o lugar da janela quem decide é esta casca. A bolinha só sobe e desce: ao
//! soltar, volta para a borda direita do monitor onde está.
//!
//! Quando o agente está usando o computador a barra NÃO minimiza (a página nem
//! mostra o botão): o Parar precisa estar sempre à mão.

use serde_json::{json, Value};
use std::sync::Mutex;
use tauri::{AppHandle, Listener, LogicalPosition, LogicalSize, Manager, WebviewWindow, WindowEvent};

pub const LARGURA: f64 = 560.0;
pub const ALTURA: f64 = 58.0;
pub const BOLHA: f64 = 66.0;
const MARGEM: f64 = 10.0;
const ARQUIVO: &str = "barra-posicao.json";

#[derive(Default)]
struct Geo {
    minimizada: bool,
    /// Último lugar da barra aberta (lógico).
    barra: Option<(f64, f64)>,
    /// Altura da bolinha na borda direita (lógico).
    bolha_y: Option<f64>,
    /// Cada movimento novo invalida o "assentou" do anterior.
    geracao: u64,
}

fn estado(app: &AppHandle) -> Option<tauri::State<'_, Mutex<Geo>>> {
    app.try_state::<Mutex<Geo>>()
}

fn caminho(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path().app_data_dir().ok().map(|p| p.join(ARQUIVO))
}

fn salvar(app: &AppHandle) {
    let Some(st) = estado(app) else { return };
    let Ok(g) = st.lock() else { return };
    let v = json!({ "barra": g.barra.map(|(x, y)| [x, y]), "bolha_y": g.bolha_y });
    if let Some(p) = caminho(app) {
        let _ = std::fs::create_dir_all(p.parent().unwrap_or(std::path::Path::new(".")));
        let _ = std::fs::write(p, v.to_string());
    }
}

fn carregar(app: &AppHandle) -> Geo {
    let mut g = Geo::default();
    if let Some(v) = caminho(app).and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
        if let (Some(x), Some(y)) = (v["barra"][0].as_f64(), v["barra"][1].as_f64()) { g.barra = Some((x, y)); }
        g.bolha_y = v["bolha_y"].as_f64();
    }
    g
}

/// Retângulo lógico (x, y, largura, altura) de cada monitor.
fn monitores(w: &WebviewWindow) -> Vec<(f64, f64, f64, f64)> {
    w.available_monitors().unwrap_or_default().iter().map(|m| {
        let s = m.scale_factor();
        (m.position().x as f64 / s, m.position().y as f64 / s, m.size().width as f64 / s, m.size().height as f64 / s)
    }).collect()
}

/// O ponto cabe inteiro em algum monitor? (monitor desligado desde a última vez = não)
fn cabe(w: &WebviewWindow, x: f64, y: f64, larg: f64, alt: f64) -> bool {
    monitores(w).iter().any(|&(mx, my, mw, mh)| x >= mx - 1.0 && y >= my - 1.0 && x + larg <= mx + mw + 1.0 && y + alt <= my + mh + 1.0)
}

/// Monitor em que a janela está agora (ou o principal).
fn monitor_da_janela(w: &WebviewWindow) -> Option<(f64, f64, f64, f64)> {
    let m = w.current_monitor().ok().flatten().or_else(|| w.primary_monitor().ok().flatten())?;
    let s = m.scale_factor();
    Some((m.position().x as f64 / s, m.position().y as f64 / s, m.size().width as f64 / s, m.size().height as f64 / s))
}

fn posicao_logica(w: &WebviewWindow) -> Option<(f64, f64)> {
    let s = w.scale_factor().ok()?;
    let p = w.outer_position().ok()?;
    Some((p.x as f64 / s, p.y as f64 / s))
}

/// Ao criar a barra: o lugar guardado, se ainda cabe num monitor; senão topo, centro.
pub fn posicionar_ao_abrir(app: &AppHandle, w: &WebviewWindow) {
    if let Some(st) = estado(app) {
        if let Ok(mut g) = st.lock() { g.minimizada = false; }
    }
    let guardada = estado(app).and_then(|st| st.lock().ok().and_then(|g| g.barra));
    if let Some((x, y)) = guardada.filter(|&(x, y)| cabe(w, x, y, LARGURA, ALTURA)) {
        let _ = w.set_position(LogicalPosition::new(x, y));
    } else if let Ok(Some(m)) = w.primary_monitor() {
        let largura = m.size().width as f64 / m.scale_factor();
        let _ = w.set_position(LogicalPosition::new(((largura - LARGURA) / 2.0).max(0.0), 8.0));
    }
    vigiar_movimento(app, w);
}

/// Depois de um arrasto: guarda o lugar; a bolinha volta para a borda direita.
fn vigiar_movimento(app: &AppHandle, w: &WebviewWindow) {
    let h = app.clone();
    let janela = w.clone();
    w.on_window_event(move |e| {
        if !matches!(e, WindowEvent::Moved(_)) { return; }
        let Some(st) = estado(&h) else { return };
        let geracao = { let Ok(mut g) = st.lock() else { return }; g.geracao += 1; g.geracao };
        let h2 = h.clone();
        let j2 = janela.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(350));
            assentou(&h2, &j2, geracao);
        });
    });
}

fn assentou(app: &AppHandle, w: &WebviewWindow, geracao: u64) {
    let Some(st) = estado(app) else { return };
    let Some((x, y)) = posicao_logica(w) else { return };
    let minimizada = {
        let Ok(g) = st.lock() else { return };
        if g.geracao != geracao { return; } // ainda se mexendo
        g.minimizada
    };
    if minimizada {
        if let Some((mx, my, mw, mh)) = monitor_da_janela(w) {
            let alvo_x = mx + mw - BOLHA - MARGEM;
            let alvo_y = y.clamp(my + 28.0, my + mh - BOLHA - MARGEM);
            if (x - alvo_x).abs() > 1.5 || (y - alvo_y).abs() > 1.5 {
                let _ = w.set_position(LogicalPosition::new(alvo_x, alvo_y));
            }
            if let Ok(mut g) = st.lock() { g.bolha_y = Some(alvo_y); }
        }
    } else if let Ok(mut g) = st.lock() {
        g.barra = Some((x, y));
    }
    salvar(app);
}

fn minimizar(app: &AppHandle) {
    let Some(w) = app.get_webview_window("barra-mac") else { return };
    let Some(st) = estado(app) else { return };
    let atual = posicao_logica(&w);
    let bolha_y = {
        let Ok(mut g) = st.lock() else { return };
        if g.minimizada { return; }
        g.minimizada = true;
        if let Some(p) = atual { g.barra = Some(p); }
        g.bolha_y
    };
    let _ = w.set_size(LogicalSize::new(BOLHA, BOLHA));
    if let Some((mx, my, mw, mh)) = monitor_da_janela(&w) {
        let y = bolha_y.or(atual.map(|p| p.1)).unwrap_or(my + 120.0).clamp(my + 28.0, my + mh - BOLHA - MARGEM);
        let _ = w.set_position(LogicalPosition::new(mx + mw - BOLHA - MARGEM, y));
    }
    salvar(app);
}

fn restaurar(app: &AppHandle) {
    let Some(w) = app.get_webview_window("barra-mac") else { return };
    let Some(st) = estado(app) else { return };
    let barra = {
        let Ok(mut g) = st.lock() else { return };
        if !g.minimizada { return; }
        g.minimizada = false;
        g.barra
    };
    let _ = w.set_size(LogicalSize::new(LARGURA, ALTURA));
    match barra.filter(|&(x, y)| cabe(&w, x, y, LARGURA, ALTURA)) {
        Some((x, y)) => { let _ = w.set_position(LogicalPosition::new(x, y)); }
        None => if let Some((mx, my, mw, _)) = monitor_da_janela(&w) {
            let _ = w.set_position(LogicalPosition::new(mx + ((mw - LARGURA) / 2.0).max(0.0), my + 8.0));
        },
    }
}

/// A barra fechou (fim da reunião): a próxima abre como barra, no lugar guardado.
pub fn fechou(app: &AppHandle) {
    if let Some(st) = estado(app) {
        if let Ok(mut g) = st.lock() { g.minimizada = false; }
    }
}

pub fn instalar(app: &AppHandle) {
    app.manage(Mutex::new(carregar(app)));
    let h = app.clone();
    app.listen_any("dnos://barra-mac/minimizar", move |_| minimizar(&h));
    let h = app.clone();
    app.listen_any("dnos://barra-mac/restaurar", move |_| restaurar(&h));
}
