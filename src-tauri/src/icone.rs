//! Ícone do app que muda enquanto o dn.os está aberto (21/09/2026).
//!
//! A página desenha a imagem (o ícone do dn.os com o rosto de quem está agindo,
//! ou o de quem está na vez do rodízio) e manda por
//! `dnos://icone/definir { png }` — base64 de um PNG, com ou sem o prefixo
//! `data:image/png;base64,`. `dnos://icone/restaurar` devolve o ícone do pacote.
//!
//! Onde muda: no macOS, o ícone do Dock (`NSApplication.applicationIconImage`,
//! que o Tauri não expõe — por isso a chamada nativa); no Windows e no Linux,
//! o ícone das janelas (barra de tarefas). Com o app fechado o sistema volta a
//! mostrar o ícone do pacote: nada aqui é gravado em disco.

use base64::Engine;
use serde_json::Value;
use tauri::{AppHandle, Listener};

use crate::meu_chrome;

/// PNG de 512×512 com transparência cabe em bem menos que isto; acima, algo está errado.
const TETO_BYTES: usize = 4 * 1024 * 1024;
const ASSINATURA_PNG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// O que a página mandou → os bytes do PNG, ou por que não serve.
pub fn decodificar(payload: &Value) -> Result<Vec<u8>, String> {
    let bruto = payload["png"].as_str().ok_or("faltou o campo png")?;
    let base64 = bruto.strip_prefix("data:image/png;base64,").unwrap_or(bruto).trim();
    // O teto vale ANTES de decodificar: 4 MB de PNG são ~5,6 MB de base64.
    if base64.len() > TETO_BYTES * 4 / 3 + 8 { return Err("imagem grande demais".into()); }
    let bytes = base64::engine::general_purpose::STANDARD.decode(base64).map_err(|e| format!("base64 inválido: {e}"))?;
    if bytes.len() > TETO_BYTES { return Err("imagem grande demais".into()); }
    if bytes.len() < ASSINATURA_PNG.len() || bytes[..8] != ASSINATURA_PNG { return Err("não é um PNG".into()); }
    Ok(bytes)
}

fn aplicar(app: &AppHandle, png: Option<Vec<u8>>) {
    let h = app.clone();
    let _ = app.run_on_main_thread(move || {
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

pub fn instalar(app: &AppHandle) {
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
}
