//! Executor de roteiro v2: roda, sem modelo, os passos que o agente escreveu na
//! exploração (`menu`, `clique`, `esperar_janela`, `tecla`…), uma linha JSON de
//! progresso por passo. Mesmo protocolo do `gravador-mac executar`:
//!   {"t":"pronto"} · {"t":"passo","n","de","texto","estado"} ·
//!   {"t":"parou","passo","de","texto","motivo","app","janela","foto"} ·
//!   {"t":"concluido","ms","de","app","janela","foto"} · {"t":"erro","motivo"}
//! Parar: "parar" no stdin (a casca) ou Esc duas vezes em 600 ms, de qualquer lugar.
//! Passo desconhecido PARA (antes seguia calado). Roteiro gravado no Mac com alvo
//! por Acessibilidade (`ax`) não vale aqui: para dizendo isso.
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE};

use super::captura;
use super::entrada::{self, dormir};
use super::janelas;
use super::uia::Uia;
use crate::logica::{self, Ret};

static PARAR: AtomicBool = AtomicBool::new(false);
/// Verdadeiro enquanto o próprio roteiro aperta Esc: não conta como "parar".
static ESC_PROPRIO: AtomicBool = AtomicBool::new(false);
static ULTIMO_ESC_MS: AtomicU64 = AtomicU64::new(0);

fn agora_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub fn linha(v: Value) {
    let so = std::io::stdout();
    let mut o = so.lock();
    let _ = writeln!(o, "{v}");
    let _ = o.flush();
}

fn pediu_parar() -> bool {
    PARAR.load(Ordering::SeqCst)
}

/// Dorme em fatias, acordando se pedirem para parar.
fn esperar(ms: u64) {
    let fim = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < fim && !pediu_parar() {
        dormir(50.min(ms));
    }
}

fn frase_do_passo(p: &Value) -> String {
    if let Some(d) = p["descricao"].as_str().filter(|d| !d.is_empty()) {
        return d.to_string();
    }
    let tipo = p["t"].as_str().or_else(|| p["acao"].as_str()).unwrap_or("");
    match tipo {
        "menu" => format!("menu {}", p["caminho"].as_array().map(|c| c.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(" › ")).unwrap_or_default()),
        "clique" => "clicando".into(),
        "duplo" => "duplo clique".into(),
        "passar" => "passando o mouse".into(),
        "tecla" => format!("pressionando {}", p["tecla"].as_str().unwrap_or("")),
        "atalho" => format!("atalho {}", p["tecla"].as_str().unwrap_or("")),
        "digitar" => "digitando".into(),
        "rolar" => "rolando".into(),
        "esperar_janela" => format!("esperando a janela \"{}\"", p["titulo_contem"].as_str().unwrap_or("")),
        "esperar" => "esperando".into(),
        "ler" => "lendo a tela".into(),
        outro => outro.to_string(),
    }
}

/// Monitor da janela em primeiro plano: a base das coordenadas 0..1 do v2.
fn tela_do_app() -> Option<Ret> {
    let (h, _) = janelas::da_frente()?;
    janelas::monitor_da_janela(h)
}

fn foto_pequena() -> Option<String> {
    let t = tela_do_app()?;
    let img = captura::capturar(t)?;
    img.jpeg_base64(640, 55).map(|(b, _, _)| b)
}

fn parou(n: usize, total: usize, texto: &str, motivo: &str) {
    let mut o = json!({ "t": "parou", "passo": n, "de": total, "texto": texto, "motivo": motivo });
    if let Some(app) = janelas::exe_da_frente() {
        o["app"] = json!(logica::nome_do_exe(&app));
        o["janela"] = json!(janelas::titulo_da_frente());
        if let Some(f) = foto_pequena() {
            o["foto"] = json!(f);
        }
    }
    linha(o);
}

fn atalho_por_nome(combo: &str) -> Result<(), String> {
    let partes: Vec<&str> = combo.split('+').filter(|s| !s.is_empty()).collect();
    let Some((tecla, mods)) = partes.split_last() else { return Err("atalho desconhecido".into()) };
    let mods: Vec<String> = mods.iter().map(|m| m.to_lowercase()).collect();
    aperta(&tecla.to_lowercase(), &mods)
}

fn aperta(tecla: &str, mods: &[String]) -> Result<(), String> {
    if logica::atalho_bloqueado(mods, tecla) {
        return Err(format!("atalho bloqueado: {}", if mods.is_empty() { tecla.to_string() } else { format!("{}+{}", mods.join("+"), tecla) }));
    }
    let escape = matches!(tecla, "escape" | "esc");
    if escape {
        ESC_PROPRIO.store(true, Ordering::SeqCst);
    }
    let r = entrada::tecla(tecla, mods).map_err(|_| "tecla desconhecida".to_string());
    if escape {
        dormir(120);
        ESC_PROPRIO.store(false, Ordering::SeqCst);
    }
    r
}

pub fn executar(caminho: &str, ignorar: &str) -> i32 {
    let Ok(dados) = std::fs::read_to_string(caminho) else {
        linha(json!({ "t": "erro", "motivo": format!("não li o roteiro em {caminho}") }));
        return 1;
    };
    let Ok(roteiro) = serde_json::from_str::<Value>(&dados) else {
        linha(json!({ "t": "erro", "motivo": format!("o roteiro em {caminho} não é um JSON válido") }));
        return 1;
    };
    let passos: Vec<Value> = roteiro["passos"].as_array().cloned().unwrap_or_default();
    let total = passos.len();
    if total == 0 {
        linha(json!({ "t": "erro", "motivo": "roteiro sem passos" }));
        return 0;
    }

    // "parar" no stdin (a casca) ou stdin fechado (a casca morreu).
    std::thread::spawn(|| {
        let stdin = std::io::stdin();
        for l in stdin.lock().lines().map_while(Result::ok) {
            if l.trim() == "parar" {
                PARAR.store(true, Ordering::SeqCst);
            }
        }
        PARAR.store(true, Ordering::SeqCst);
    });
    // Esc duas vezes seguidas (600 ms), de qualquer lugar.
    std::thread::spawn(|| {
        let mut apertado = false;
        loop {
            let agora = unsafe { GetAsyncKeyState(VK_ESCAPE.0 as i32) } as u16 & 0x8000 != 0;
            if agora && !apertado && !ESC_PROPRIO.load(Ordering::SeqCst) {
                let t = agora_ms();
                if t.saturating_sub(ULTIMO_ESC_MS.load(Ordering::SeqCst)) < 600 {
                    PARAR.store(true, Ordering::SeqCst);
                }
                ULTIMO_ESC_MS.store(t, Ordering::SeqCst);
            }
            apertado = agora;
            dormir(20);
        }
    });

    linha(json!({ "t": "pronto", "hora": agora_ms(), "passos": total }));
    let uia = Uia::nova();
    let inicio = agora_ms();

    // Roteiro v2: `bundle` (executável) no topo; o Desktop traz o app para a frente, abrindo se preciso.
    let mut atual = String::new();
    if let Some(b) = roteiro["bundle"].as_str().filter(|b| !b.is_empty()) {
        let b_l = b.to_lowercase();
        if b == ignorar || b_l.contains("dnos") {
            linha(json!({ "t": "parou", "passo": 0, "de": total, "texto": "abrir app", "motivo": "o roteiro é dentro do próprio dn.os" }));
            return 0;
        }
        let nome = roteiro["app"].as_str().unwrap_or(b);
        if !janelas::ativar_app(&b_l, Duration::from_secs(30)) {
            linha(json!({ "t": "parou", "passo": 0, "de": total, "texto": "abrir app", "motivo": format!("não consegui abrir {nome}") }));
            return 0;
        }
        atual = b_l;
        dormir(400);
    }

    for (i, p) in passos.iter().enumerate() {
        let n = i + 1;
        let frase = frase_do_passo(p);
        if pediu_parar() {
            parou(n, total, &frase, "você parou");
            return 0;
        }
        linha(json!({ "t": "passo", "n": n, "de": total, "texto": frase, "estado": "rodando" }));
        let tipo = p["t"].as_str().or_else(|| p["acao"].as_str()).unwrap_or("").to_string();
        if p.get("ax").is_some() || p.get("coord").is_some() {
            parou(n, total, &frase, "este passo foi gravado no Mac (alvo por Acessibilidade): aprenda a habilidade de novo neste computador");
            return 0;
        }
        // Apps sem `bundle` no topo: age no que estiver na frente, como o Mac.
        let Some(app_exe) = janelas::exe_da_frente() else {
            parou(n, total, &frase, "nenhum app na frente");
            return 0;
        };
        if atual.is_empty() {
            atual = app_exe.clone();
        }
        let pids = janelas::pids_do_exe(&app_exe);
        let hwnd_frente = janelas::da_frente().map(|(h, _)| janelas::numero(h)).unwrap_or(0);
        let mut falhou: Option<String> = None;
        let ponto = || -> Option<(i32, i32)> { logica::ponto_na_area(tela_do_app()?, p["x"].as_f64()?, p["y"].as_f64()?) };

        match tipo.as_str() {
            "app" => {}
            "clique" | "duplo" => match ponto() {
                Some(pt) => {
                    let direito = p["botao"].as_str() == Some("direito");
                    let vezes = if tipo == "duplo" || p["cliques"].as_i64() == Some(2) { 2 } else { 1 };
                    entrada::clicar(pt.0, pt.1, direito, vezes);
                }
                None => falhou = Some("o passo não tem x/y válidos (0 a 1 da tela)".into()),
            },
            "passar" => {
                if let Some(pt) = ponto() {
                    entrada::mover(pt.0, pt.1);
                }
            }
            "digitar" => {
                if p["senha"].as_bool() == Some(true) {
                    falhou = Some("senha: eu não digito; digite você e continue".into());
                } else if uia.as_ref().map(|u| u.foco_protegido()).unwrap_or(false) {
                    falhou = Some("o foco está num campo de senha: eu não digito".into());
                } else {
                    entrada::texto(p["valor"].as_str().unwrap_or(""));
                }
            }
            "tecla" => {
                let mods: Vec<String> = p["mods"].as_array().map(|m| m.iter().filter_map(|x| x.as_str().map(|s| s.to_lowercase())).collect()).unwrap_or_default();
                if let Err(e) = aperta(&p["tecla"].as_str().unwrap_or("").to_lowercase(), &mods) {
                    falhou = Some(e);
                }
            }
            "atalho" => {
                if let Err(e) = atalho_por_nome(&p["tecla"].as_str().unwrap_or("").to_lowercase()) {
                    falhou = Some(e);
                }
            }
            "rolar" => {
                let pt = ponto().or_else(|| tela_do_app().map(|t| (t.x + t.w / 2, t.y + t.h / 2))).unwrap_or((600, 400));
                let quanto = p["quanto"].as_i64().or_else(|| p["dy"].as_i64().map(|d| d.abs())).unwrap_or(300) as i32;
                let para_cima = p["direcao"].as_str().map(|d| d == "cima").unwrap_or_else(|| p["dy"].as_i64().unwrap_or(0) > 0);
                let dy = if para_cima { quanto } else { -quanto };
                entrada::rolar(pt.0, pt.1, dy, p["dx"].as_i64().unwrap_or(0) as i32);
            }
            "menu" => {
                let caminho: Vec<String> = p["caminho"].as_array().map(|c| c.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
                if !(1..=4).contains(&caminho.len()) {
                    falhou = Some("menu: caminho inválido".into());
                } else {
                    match uia.as_ref() {
                        Some(u) => {
                            if let Err(m) = u.menu(hwnd_frente, &caminho, false) {
                                falhou = Some(match m.as_str() {
                                    "menu_indisponivel" => "este app não expõe o menu ao Windows: use um atalho ou um clique medido".into(),
                                    "menu_desativado" => "item de menu desativado".into(),
                                    outro => outro.replace('_', " "),
                                });
                            }
                        }
                        None => falhou = Some("a Automação de Interface do Windows não abriu".into()),
                    }
                }
            }
            "esperar_janela" => {
                let quer = p["titulo_contem"].as_str().unwrap_or("").to_lowercase();
                let fim = Instant::now() + Duration::from_millis(p["ms"].as_u64().unwrap_or(8000).min(30_000));
                let mut achou = false;
                while Instant::now() < fim && !pediu_parar() {
                    if janelas::listar(false).iter().any(|j| pids.contains(&j.pid) && j.titulo.to_lowercase().contains(&quer)) {
                        achou = true;
                        break;
                    }
                    dormir(250);
                }
                if !achou {
                    falhou = Some(format!("a janela \"{}\" não apareceu", p["titulo_contem"].as_str().unwrap_or("")));
                }
            }
            "esperar" => esperar(p["ms"].as_u64().unwrap_or(500).min(10_000)),
            "ler" => {}
            "" => falhou = Some("passo sem tipo".into()),
            outro => falhou = Some(format!("passo desconhecido: {outro}")),
        }
        if let Some(f) = falhou {
            parou(n, total, &frase, &f);
            return 0;
        }
        linha(json!({ "t": "passo", "n": n, "de": total, "texto": frase, "estado": "ok" }));
        // Espera a tela assentar antes do próximo.
        esperar(if tipo == "app" { 300 } else { 450 });
    }

    let mut fim = json!({ "t": "concluido", "ms": agora_ms() - inicio, "de": total });
    if let Some(app) = janelas::exe_da_frente() {
        fim["app"] = json!(logica::nome_do_exe(&app));
        fim["janela"] = json!(janelas::titulo_da_frente());
        if let Some(f) = foto_pequena() {
            fim["foto"] = json!(f);
        }
    }
    linha(fim);
    0
}
