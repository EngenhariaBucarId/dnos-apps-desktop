//! "Salvar como…" do vídeo final do dn.estudio (0.8.18, 14/10).
//!
//! No app, o link do vídeo final abria no navegador e o arquivo caía direto em Downloads. Agora a página pede à
//! casca: abre a janela nativa de salvar (com o nome do vídeo sugerido), e a própria casca baixa o arquivo para onde
//! a pessoa escolheu — primeiro num `.parcial` ao lado, trocado pelo nome final só quando termina. Só grava no
//! caminho que a pessoa escolheu na janela; endereço só https.

use futures_util::StreamExt;
use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

static ULTIMO: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Nome de arquivo seguro para sugerir na janela: sem barras nem caracteres que o Windows recusa, com .mp4.
pub fn nome_sugerido(nome: &str) -> String {
    let limpo: String = nome
        .chars()
        .map(|c| if "/\\:*?\"<>|".contains(c) || c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let base = limpo.trim_end_matches(".mp4").trim_end_matches(".MP4").trim();
    let base = if base.is_empty() { "video-final" } else { base };
    let base: String = base.chars().take(120).collect();
    format!("{base}.mp4")
}

/// Só https (o vídeo final vem do serviço do dn.estudio por link assinado).
pub fn endereco_valido(url: &str) -> bool {
    url::Url::parse(url).map(|u| u.scheme() == "https" && u.host_str().is_some()).unwrap_or(false)
}

#[tauri::command]
pub async fn salvar_video(app: AppHandle, window: tauri::WebviewWindow, url: String, nome: String) -> Result<Value, String> {
    if window.label() != "main" { return Err("janela_nao_autorizada".into()); }
    if !endereco_valido(&url) { return Err("endereco_invalido".into()); }
    let sugerido = nome_sugerido(&nome);
    let app2 = app.clone();
    let escolhido = tauri::async_runtime::spawn_blocking(move || {
        app2.dialog().file().set_title("Salvar o vídeo final").set_file_name(&sugerido).add_filter("Vídeo MP4", &["mp4"]).blocking_save_file()
    })
    .await
    .map_err(|e| e.to_string())?;
    let Some(destino) = escolhido.and_then(|f| f.into_path().ok()) else { return Ok(json!({ "cancelado": true })) };

    let parcial = destino.with_extension("mp4.parcial");
    let resultado = baixar(&url, &parcial).await;
    match resultado {
        Ok(bytes) => {
            std::fs::rename(&parcial, &destino).map_err(|e| { let _ = std::fs::remove_file(&parcial); e.to_string() })?;
            *ULTIMO.lock().unwrap() = Some(destino.clone());
            Ok(json!({ "caminho": destino.to_string_lossy(), "bytes": bytes }))
        }
        Err(e) => {
            let _ = std::fs::remove_file(&parcial);
            Err(e)
        }
    }
}

async fn baixar(url: &str, para: &PathBuf) -> Result<u64, String> {
    let resp = reqwest::get(url).await.map_err(|e| format!("sem_conexao: {e}"))?;
    if !resp.status().is_success() { return Err(format!("servidor_respondeu_{}", resp.status().as_u16())); }
    let mut arquivo = std::fs::File::create(para).map_err(|e| format!("nao_gravou: {e}"))?;
    let mut total = 0u64;
    let mut fluxo = resp.bytes_stream();
    while let Some(pedaco) = fluxo.next().await {
        let pedaco = pedaco.map_err(|e| format!("download_interrompido: {e}"))?;
        arquivo.write_all(&pedaco).map_err(|e| format!("nao_gravou: {e}"))?;
        total += pedaco.len() as u64;
    }
    arquivo.flush().map_err(|e| e.to_string())?;
    Ok(total)
}

/// Nome sugerido para qualquer arquivo (0.8.19, 21/10): mesma limpeza do vídeo, mantendo a extensão que veio.
pub fn nome_de_arquivo(nome: &str) -> String {
    let limpo: String = nome
        .chars()
        .map(|c| if "/\\:*?\"<>|".contains(c) || c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let limpo = limpo.trim().trim_start_matches('.').to_string();
    if limpo.is_empty() { return "arquivo".into(); }
    limpo.chars().take(150).collect()
}

/// Extensão do nome (sem o ponto), para o filtro da janela; nada se não tiver.
pub fn extensao(nome: &str) -> Option<String> {
    let (base, ext) = nome.rsplit_once('.')?;
    if base.is_empty() || ext.is_empty() || ext.len() > 8 || !ext.chars().all(|c| c.is_ascii_alphanumeric()) { return None; }
    Some(ext.to_ascii_lowercase())
}

const MAX_DADOS: usize = 300 * 1024 * 1024;

/// "Salvar como…" de qualquer arquivo do dn.os (0.8.19, 21/10, pedido do Rodrigo: imagem, anexo, PDF exportado
/// caíam direto em Downloads). A página manda o endereço https (a casca baixa) ou os bytes já prontos em base64
/// (arquivo gerado na própria página, como o PDF do dn.docs). Só grava onde a pessoa escolheu na janela.
#[tauri::command]
pub async fn salvar_arquivo(app: AppHandle, window: tauri::WebviewWindow, nome: String, url: Option<String>, dados: Option<String>) -> Result<Value, String> {
    if window.label() != "main" { return Err("janela_nao_autorizada".into()); }
    if let Some(u) = &url { if !endereco_valido(u) { return Err("endereco_invalido".into()); } }
    let bytes = match (&url, dados) {
        (Some(_), _) => None,
        (None, Some(b64)) => {
            if b64.len() > MAX_DADOS / 3 * 4 + 4 { return Err("arquivo_grande_demais".into()); }
            use base64::Engine;
            Some(base64::engine::general_purpose::STANDARD.decode(b64.as_bytes()).map_err(|_| "dados_invalidos".to_string())?)
        }
        (None, None) => return Err("sem_arquivo".into()),
    };
    let sugerido = nome_de_arquivo(&nome);
    let ext = extensao(&sugerido);
    let app2 = app.clone();
    let escolhido = tauri::async_runtime::spawn_blocking(move || {
        let mut d = app2.dialog().file().set_title("Salvar arquivo").set_file_name(&sugerido);
        if let Some(e) = &ext { d = d.add_filter(e.to_uppercase(), &[e.as_str()]); }
        d.blocking_save_file()
    })
    .await
    .map_err(|e| e.to_string())?;
    let Some(destino) = escolhido.and_then(|f| f.into_path().ok()) else { return Ok(json!({ "cancelado": true })) };

    let total = if let Some(b) = bytes {
        std::fs::write(&destino, &b).map_err(|e| format!("nao_gravou: {e}"))?;
        b.len() as u64
    } else {
        let mut parcial = destino.clone().into_os_string();
        parcial.push(".parcial");
        let parcial = PathBuf::from(parcial);
        match baixar(url.as_deref().unwrap_or_default(), &parcial).await {
            Ok(n) => { std::fs::rename(&parcial, &destino).map_err(|e| { let _ = std::fs::remove_file(&parcial); e.to_string() })?; n }
            Err(e) => { let _ = std::fs::remove_file(&parcial); return Err(e); }
        }
    };
    *ULTIMO.lock().unwrap() = Some(destino.clone());
    Ok(json!({ "caminho": destino.to_string_lossy(), "bytes": total }))
}

/// "Mostrar na pasta": só o último arquivo que a própria casca salvou.
#[tauri::command]
pub fn salvar_mostrar(window: tauri::WebviewWindow) -> Result<(), String> {
    if window.label() != "main" { return Err("janela_nao_autorizada".into()); }
    let Some(p) = ULTIMO.lock().unwrap().clone() else { return Err("nada_salvo".into()) };
    tauri_plugin_opener::reveal_item_in_dir(p).map_err(|e| e.to_string())
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn nome_sem_caracteres_proibidos_e_com_mp4() {
        assert_eq!(nome_sugerido("Reel: lançamento/março"), "Reel lançamento março.mp4");
        assert_eq!(nome_sugerido("final.mp4"), "final.mp4");
        assert_eq!(nome_sugerido("  "), "video-final.mp4");
    }

    #[test]
    fn nome_e_extensao_de_qualquer_arquivo() {
        assert_eq!(nome_de_arquivo("Relatório: out/26.pdf"), "Relatório out 26.pdf");
        assert_eq!(nome_de_arquivo("..env"), "env");
        assert_eq!(nome_de_arquivo(""), "arquivo");
        assert_eq!(extensao("foto.PNG").as_deref(), Some("png"));
        assert_eq!(extensao("sem-extensao"), None);
        assert_eq!(extensao(".bashrc"), None);
    }

    #[test]
    fn so_https() {
        assert!(endereco_valido("https://estudio.dnia.ai/a/xyz/final.mp4"));
        assert!(!endereco_valido("http://estudio.dnia.ai/final.mp4"));
        assert!(!endereco_valido("file:///etc/passwd"));
    }
}
