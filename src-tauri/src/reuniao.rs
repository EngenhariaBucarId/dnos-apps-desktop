//! Reunião ao vivo, fase 1 (20/09/2026): a pessoa clica em "Reunião" no chat,
//! o dn.os abre o microfone desta máquina e a fala vai virando texto enquanto
//! a reunião acontece. Serve para reunião presencial em qualquer sistema — o
//! microfone é a única fonte desta fase; som da chamada é o passo 2.
//!
//! O que este módulo faz, e o que deixa para a página:
//! - aqui: reservar o computador (um uso por vez, igual à gravação e à
//!   execução), abrir o microfone pelo `voz` com corte de reunião, manter a
//!   barra flutuante visível com Parar, e derrubar tudo no teto de 2 h;
//! - na página: transcrever cada trecho (`dnos://reuniao/audio`), juntar o
//!   texto, mandar ao agente e pedir o resumo no fim.
//!
//! Áudio não é guardado: o trecho vai em memória para a página, é transcrito e
//! descartado. Nada toca o disco (decisão do Rodrigo, 20/09).
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Listener, Manager};

use crate::{barra, maquina, meu_chrome, voz};

/// Teto de uma sessão. Reunião esquecida aberta é microfone aberto.
const TETO: std::time::Duration = std::time::Duration::from_secs(2 * 60 * 60);

struct Sessao {
    id: String,
    agente: String,
    inicio: std::time::Instant,
    /// Solta a reserva do computador quando a sessão morre (RAII).
    _reserva: maquina::ReservaDeUso,
}

#[derive(Default)]
pub struct Reuniao {
    ativa: Option<Sessao>,
}
pub type Compartilhado = Arc<Mutex<Reuniao>>;

fn emitir(app: &AppHandle, v: Value) {
    meu_chrome::registrar(app, &format!("reuniao: {v}"));
    let _ = app.emit("dnos://reuniao", v);
}

/// Há uma reunião gravando agora?
pub fn ativa(app: &AppHandle) -> bool {
    app.try_state::<Compartilhado>()
        .and_then(|e| e.lock().ok().map(|g| g.ativa.is_some()))
        .unwrap_or(false)
}

fn barra_da_reuniao(app: &AppHandle, agente: &str) {
    let titulo = if agente.is_empty() { "dn.os está ouvindo a reunião".to_string() } else { format!("{agente} está ouvindo a reunião") };
    barra::mostrar(app, json!({ "modo": "grav", "titulo": titulo, "sub": "só transcrição — o áudio não é guardado", "parar": true }));
}

fn iniciar(app: &AppHandle, id: String, agente: String) {
    if let Some(estado) = app.try_state::<Compartilhado>() {
        if let Ok(g) = estado.lock() {
            if let Some(a) = g.ativa.as_ref() {
                emitir(app, json!({ "estado": "gravando", "id": a.id, "agente": a.agente }));
                return;
            }
        }
    }
    // Um uso por vez: se um agente já está executando ou gravando aqui, a
    // reunião não entra por cima (mesma reserva da 0.7.12).
    let reserva = match maquina::reservar_uso(app, "reuniao") {
        Ok(r) => r,
        Err(motivo) => { emitir(app, json!({ "estado": "erro", "motivo": motivo })); return; }
    };
    let Some(estado) = app.try_state::<Compartilhado>() else {
        emitir(app, json!({ "estado": "erro", "motivo": "reunião indisponível nesta versão" }));
        return;
    };
    if let Ok(mut g) = estado.lock() {
        g.ativa = Some(Sessao { id: id.clone(), agente: agente.clone(), inicio: std::time::Instant::now(), _reserva: reserva });
    }
    voz::ligar_com(app, &id, voz::REUNIAO);
    barra_da_reuniao(app, &agente);
    emitir(app, json!({ "estado": "gravando", "id": id, "agente": agente }));

    // Teto: mata a sessão sozinha, mesmo se a página sumir.
    let h = app.clone();
    let id_teto = id.clone();
    std::thread::spawn(move || {
        std::thread::sleep(TETO);
        let ainda = h.try_state::<Compartilhado>()
            .and_then(|e| e.lock().ok().map(|g| g.ativa.as_ref().map(|a| a.id == id_teto).unwrap_or(false)))
            .unwrap_or(false);
        if ainda { parar_com(&h, "tempo máximo de 2 horas"); }
    });
}

fn parar_com(app: &AppHandle, motivo: &str) {
    let Some(estado) = app.try_state::<Compartilhado>() else { return };
    let fim = match estado.lock() {
        Ok(mut g) => g.ativa.take(),
        Err(_) => return,
    };
    let Some(sessao) = fim else { return };
    voz::desligar(app);
    barra::esconder(app);
    emitir(app, json!({
        "estado": "parado",
        "id": sessao.id,
        "agente": sessao.agente,
        "segundos": sessao.inicio.elapsed().as_secs(),
        "motivo": motivo,
    }));
}

/// Parar vindo da barra flutuante — só age quando a reunião é quem está no ar.
pub fn parar_pela_barra(app: &AppHandle) -> bool {
    if !ativa(app) { return false; }
    parar_com(app, "barra");
    true
}

pub fn instalar(app: &AppHandle) {
    app.manage::<Compartilhado>(Arc::new(Mutex::new(Reuniao::default())));

    let h = app.clone();
    app.listen_any("dnos://reuniao/iniciar", move |evento| {
        let v: Value = serde_json::from_str(evento.payload()).unwrap_or(json!({}));
        let id = v["id"].as_str().unwrap_or("").trim().to_string();
        let agente = v["agente"].as_str().unwrap_or("").trim().to_string();
        if id.is_empty() || id.len() > 64 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            emitir(&h, json!({ "estado": "erro", "motivo": "identificador inválido" }));
            return;
        }
        iniciar(&h, id, agente);
    });

    let h = app.clone();
    app.listen_any("dnos://reuniao/parar", move |_| parar_com(&h, "pedido"));

    let h = app.clone();
    app.listen_any("dnos://reuniao/estado", move |_| {
        let atual = h.try_state::<Compartilhado>()
            .and_then(|e| e.lock().ok().map(|g| g.ativa.as_ref().map(|a| (a.id.clone(), a.agente.clone(), a.inicio.elapsed().as_secs()))))
            .flatten();
        match atual {
            Some((id, agente, seg)) => emitir(&h, json!({ "estado": "gravando", "id": id, "agente": agente, "segundos": seg })),
            None => emitir(&h, json!({ "estado": "parado" })),
        }
    });

    // A barra é a mesma da gravação; o botão Parar dela emite este evento.
    // Quando quem está no ar é a reunião, é a reunião que encerra.
    let h = app.clone();
    app.listen_any("dnos://gravador/parar-pela-barra", move |_| { parar_pela_barra(&h); });
}
