//! Ícone do app que muda enquanto o dn.os está aberto (21/09/2026).
//!
//! A página desenha as imagens (o ícone do dn.os com os agentes) e manda por
//! - `dnos://icone/definir { png }` — uma imagem parada: base64 de um PNG, com ou sem o
//!   prefixo `data:image/png;base64,`;
//! - `dnos://icone/animar { quadros: [png…], fps }` — os quadros de UMA volta; a casca os
//!   repete num relógio próprio. Tem de ser aqui e não na página: com a janela em segundo
//!   plano o navegador embutido segura os temporizadores, e é aí que se olha o Dock;
//! - `dnos://icone/restaurar` — devolve o ícone do pacote.
//!
//! Onde muda: no macOS, o ícone do Dock (`NSApplication.applicationIconImage`,
//! que o Tauri não expõe — por isso a chamada nativa); no Windows e no Linux,
//! o ícone das janelas (barra de tarefas). Com o app fechado o sistema volta a
//! mostrar o ícone do pacote: nada aqui é gravado em disco.

use base64::Engine;
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Listener, Manager};

use crate::meu_chrome;

/// PNG de 512×512 com transparência cabe em bem menos que isto; acima, algo está errado.
const TETO_BYTES: usize = 4 * 1024 * 1024;
const ASSINATURA_PNG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// Quadros de uma volta: mais que isto é desenho errado, e cada um vira imagem na memória.
const MAX_QUADROS: usize = 24;
const TETO_DA_ANIMACAO_BYTES: usize = 8 * 1024 * 1024;
const FPS_PADRAO: u64 = 8;
const FPS_MAXIMO: u64 = 15;

/// O que a página mandou → os bytes do PNG, ou por que não serve.
pub fn decodificar(payload: &Value) -> Result<Vec<u8>, String> {
    decodificar_texto(payload["png"].as_str().ok_or("faltou o campo png")?)
}

fn decodificar_texto(bruto: &str) -> Result<Vec<u8>, String> {
    let base64 = bruto.strip_prefix("data:image/png;base64,").unwrap_or(bruto).trim();
    // O teto vale ANTES de decodificar: 4 MB de PNG são ~5,6 MB de base64.
    if base64.len() > TETO_BYTES * 4 / 3 + 8 { return Err("imagem grande demais".into()); }
    let bytes = base64::engine::general_purpose::STANDARD.decode(base64).map_err(|e| format!("base64 inválido: {e}"))?;
    if bytes.len() > TETO_BYTES { return Err("imagem grande demais".into()); }
    if bytes.len() < ASSINATURA_PNG.len() || bytes[..8] != ASSINATURA_PNG { return Err("não é um PNG".into()); }
    Ok(bytes)
}

#[derive(Debug)]
pub struct Animacao {
    pub quadros: Vec<Vec<u8>>,
    pub periodo: Duration,
}

/// `{ quadros: [png…], fps }` → os quadros já decodificados e o intervalo entre eles.
pub fn decodificar_animacao(payload: &Value) -> Result<Animacao, String> {
    let lista = payload["quadros"].as_array().ok_or("faltou o campo quadros")?;
    if lista.is_empty() { return Err("nenhum quadro".into()); }
    if lista.len() > MAX_QUADROS { return Err("quadros demais".into()); }
    let mut quadros = Vec::with_capacity(lista.len());
    let mut total = 0usize;
    for (i, q) in lista.iter().enumerate() {
        let bytes = decodificar_texto(q.as_str().ok_or_else(|| format!("quadro {i} não é texto"))?).map_err(|e| format!("quadro {i}: {e}"))?;
        total += bytes.len();
        if total > TETO_DA_ANIMACAO_BYTES { return Err("animação grande demais".into()); }
        quadros.push(bytes);
    }
    let fps = payload["fps"].as_u64().filter(|f| *f > 0).unwrap_or(FPS_PADRAO).min(FPS_MAXIMO);
    Ok(Animacao { quadros, periodo: Duration::from_millis(1000 / fps) })
}

/// Toca os quadros em laço, um a cada `periodo`, até chegar sinal em `parar` (ou o remetente sumir).
pub fn tocar(quadros: &[Vec<u8>], periodo: Duration, parar: &Receiver<()>, mut aplicar: impl FnMut(&[u8])) {
    if quadros.is_empty() { return; }
    loop {
        for q in quadros {
            aplicar(q);
            match parar.recv_timeout(periodo) {
                Err(RecvTimeoutError::Timeout) => {}
                _ => return,
            }
        }
    }
}

/// Quem toca a animação agora (no máximo uma) e a "geração" dela: um quadro que ficou na fila da
/// thread principal depois de a animação ser trocada confere a geração e some, em vez de
/// sobrescrever a imagem nova.
#[derive(Default)]
struct Animador(Mutex<Option<Sender<()>>>);
static GERACAO: AtomicU64 = AtomicU64::new(0);

fn parar_animacao(app: &AppHandle) -> u64 {
    if let Some(a) = app.try_state::<Animador>() {
        if let Ok(mut g) = a.0.lock() { if let Some(tx) = g.take() { let _ = tx.send(()); } }
    }
    GERACAO.fetch_add(1, Ordering::SeqCst) + 1
}

fn aplicar(app: &AppHandle, png: Option<Vec<u8>>) {
    let geracao = parar_animacao(app);
    aplicar_na_geracao(app, png, geracao);
}

/// Põe a imagem na thread principal — a menos que outra animação/imagem já tenha assumido no meio do caminho.
fn aplicar_na_geracao(app: &AppHandle, png: Option<Vec<u8>>, geracao: u64) {
    let h = app.clone();
    let _ = app.run_on_main_thread(move || {
        if GERACAO.load(Ordering::SeqCst) != geracao { return; }
        #[cfg(target_os = "macos")]
        {
            let ok = unsafe { crate::icone_dock::definir(png.as_deref()) };
            if !ok { meu_chrome::registrar(&h, "icone: o macOS não aceitou a imagem"); }
        }
        #[cfg(not(target_os = "macos"))]
        {
            use tauri::Manager;
            let imagem = match &png {
                Some(b) => tauri::image::Image::from_bytes(b).ok(),
                None => h.default_window_icon().cloned(),
            };
            match imagem {
                Some(i) => { for (_, j) in h.webview_windows() { let _ = j.set_icon(i.clone()); } }
                None => meu_chrome::registrar(&h, "icone: não consegui ler a imagem"),
            }
        }
    });
}

fn animar(app: &AppHandle, a: Animacao) {
    let geracao = parar_animacao(app);
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    if let Some(estado) = app.try_state::<Animador>() { if let Ok(mut g) = estado.0.lock() { *g = Some(tx); } }
    let h = app.clone();
    std::thread::spawn(move || {
        tocar(&a.quadros, a.periodo, &rx, |q| aplicar_na_geracao(&h, Some(q.to_vec()), geracao));
    });
}

pub fn instalar(app: &AppHandle) {
    app.manage(Animador::default());
    let h = app.clone();
    app.listen_any("dnos://icone/animar", move |evento| {
        let v: Value = serde_json::from_str(evento.payload()).unwrap_or(Value::Null);
        match decodificar_animacao(&v) {
            Ok(a) => animar(&h, a),
            Err(e) => meu_chrome::registrar(&h, &format!("icone: animação recusada ({e})")),
        }
    });
    let h = app.clone();
    app.listen_any("dnos://icone/definir", move |evento| {
        let v: Value = serde_json::from_str(evento.payload()).unwrap_or(Value::Null);
        match decodificar(&v) {
            Ok(bytes) => aplicar(&h, Some(bytes)),
            Err(e) => meu_chrome::registrar(&h, &format!("icone: imagem recusada ({e})")),
        }
    });
    let h = app.clone();
    app.listen_any("dnos://icone/restaurar", move |_| aplicar(&h, None));
}

#[cfg(test)]
mod testes {
    use super::*;
    use serde_json::json;

    /// Um PNG mínimo válido (1×1), só para passar pela assinatura.
    const PNG_1X1: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

    #[test]
    fn aceita_png_com_e_sem_prefixo() {
        assert!(decodificar(&json!({ "png": PNG_1X1 })).is_ok());
        assert!(decodificar(&json!({ "png": format!("data:image/png;base64,{PNG_1X1}") })).is_ok());
    }

    #[test]
    fn recusa_o_que_nao_e_png() {
        let jpeg = base64::engine::general_purpose::STANDARD.encode([0xFF, 0xD8, 0xFF, 0xE0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(decodificar(&json!({ "png": jpeg })).unwrap_err(), "não é um PNG");
        assert!(decodificar(&json!({ "png": "isto não é base64 !!!" })).is_err());
        assert!(decodificar(&json!({})).is_err());
        assert!(decodificar(&json!({ "png": "" })).is_err());
    }

    #[test]
    fn recusa_imagem_grande_demais_sem_decodificar_tudo() {
        let enorme = "A".repeat(TETO_BYTES * 2);
        assert_eq!(decodificar(&json!({ "png": enorme })).unwrap_err(), "imagem grande demais");
    }

    fn quadro() -> Value { json!(PNG_1X1) }

    #[test]
    fn animacao_valida_traz_os_quadros_e_o_ritmo() {
        let a = decodificar_animacao(&json!({ "quadros": [quadro(), quadro(), quadro()], "fps": 8 })).unwrap();
        assert_eq!(a.quadros.len(), 3);
        assert_eq!(a.periodo, Duration::from_millis(125));
    }

    #[test]
    fn ritmo_ausente_vira_padrao_e_exagerado_e_limitado() {
        assert_eq!(decodificar_animacao(&json!({ "quadros": [quadro()] })).unwrap().periodo, Duration::from_millis(125));
        assert_eq!(decodificar_animacao(&json!({ "quadros": [quadro()], "fps": 0 })).unwrap().periodo, Duration::from_millis(125));
        assert_eq!(decodificar_animacao(&json!({ "quadros": [quadro()], "fps": 500 })).unwrap().periodo, Duration::from_millis(66));
    }

    #[test]
    fn animacao_invalida_e_recusada_com_o_motivo() {
        assert!(decodificar_animacao(&json!({})).is_err());
        assert_eq!(decodificar_animacao(&json!({ "quadros": [] })).unwrap_err(), "nenhum quadro");
        let demais: Vec<Value> = (0..MAX_QUADROS + 1).map(|_| quadro()).collect();
        assert_eq!(decodificar_animacao(&json!({ "quadros": demais })).unwrap_err(), "quadros demais");
        // um quadro ruim no meio derruba a animação toda e diz qual foi
        let e = decodificar_animacao(&json!({ "quadros": [quadro(), "lixo!!", quadro()] })).unwrap_err();
        assert!(e.starts_with("quadro 1:"), "{e}");
        assert!(decodificar_animacao(&json!({ "quadros": [42] })).is_err());
    }

    #[test]
    fn animacao_pesada_demais_e_recusada() {
        // 6 quadros de ~1,5 MB (PNG "válido" só na assinatura) passam de 8 MB no total
        let mut bytes = ASSINATURA_PNG.to_vec();
        bytes.resize(1_500_000, 0);
        let q = json!(base64::engine::general_purpose::STANDARD.encode(&bytes));
        let e = decodificar_animacao(&json!({ "quadros": [q.clone(), q.clone(), q.clone(), q.clone(), q.clone(), q] })).unwrap_err();
        assert_eq!(e, "animação grande demais");
    }

    #[test]
    fn o_laco_toca_os_quadros_em_ordem_e_para_quando_mandam() {
        let quadros: Vec<Vec<u8>> = vec![vec![0], vec![1], vec![2]];
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let (visto_tx, visto_rx) = std::sync::mpsc::channel::<u8>();
        let t = std::thread::spawn(move || tocar(&quadros, Duration::from_millis(5), &rx, |q| { let _ = visto_tx.send(q[0]); }));
        std::thread::sleep(Duration::from_millis(60));
        tx.send(()).unwrap();
        t.join().unwrap(); // se o laço não parasse, o teste travaria aqui
        let vistos: Vec<u8> = visto_rx.try_iter().collect();
        assert!(vistos.len() >= 7, "deu poucas voltas: {vistos:?}");
        for (i, v) in vistos.iter().enumerate() { assert_eq!(*v as usize, i % 3, "fora de ordem em {i}: {vistos:?}"); }
    }

    #[test]
    fn o_laco_tambem_para_quando_o_remetente_some() {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let t = std::thread::spawn(move || tocar(&[vec![0], vec![1]], Duration::from_millis(5), &rx, |_| {}));
        std::thread::sleep(Duration::from_millis(20));
        drop(tx);
        t.join().unwrap();
    }
}
