//! Cópias da prévia do dn.estudio numa pasta escolhida pela pessoa (11/10).
//!
//! No navegador, o dn.estudio guarda a cópia leve de cada vídeo no próprio navegador ou numa pasta (Chrome/Edge).
//! O WKWebView do app não tem o seletor de pasta da web, então a casca faz esse papel: abre o seletor nativo,
//! grava os bytes que a página baixou, lê de volta para a prévia, lista e apaga. A pasta fica em
//! `<config do app>/copias.json`. Só arquivos `.mp4` direto na pasta, com nome sem barra: a página nunca
//! escreve fora dela.
//!
//! Comandos (invoke, com permissão `allow-copias` só para a página publicada na janela main):
//!   copias_estado            → { pasta?, arquivos, bytes }
//!   copias_escolher_pasta    → { pasta? }          (seletor nativo; nada escolhido = pasta continua)
//!   copias_voltar            → apaga a escolha (os arquivos ficam na pasta)
//!   copias_listar            → [{ nome, bytes }]
//!   copias_ler { nome }      → bytes do arquivo (resposta binária) ou erro "nao_existe"
//!   copias_gravar            → corpo binário; cabeçalhos x-nome e x-prefixo (apaga as outras versões)
//!   copias_apagar { nome }

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tauri::{ipc::{InvokeBody, Request, Response}, AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;

#[derive(Serialize, Deserialize, Default)]
struct Config {
    pasta: Option<String>,
}

fn arquivo_config(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join("copias.json"))
}

fn ler_config(app: &AppHandle) -> Config {
    arquivo_config(app)
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn gravar_config(app: &AppHandle, c: &Config) -> Result<(), String> {
    let f = arquivo_config(app).ok_or("sem pasta de configuração")?;
    if let Some(d) = f.parent() { std::fs::create_dir_all(d).map_err(|e| e.to_string())?; }
    std::fs::write(f, serde_json::to_string(c).unwrap_or_default()).map_err(|e| e.to_string())
}

fn pasta(app: &AppHandle) -> Result<PathBuf, String> {
    let p = ler_config(app).pasta.ok_or("sem_pasta")?;
    let p = PathBuf::from(p);
    if p.is_dir() { Ok(p) } else { Err("pasta_sumiu".into()) }
}

/// Nome aceito: só o arquivo (sem barra, sem `..`), terminando em .mp4 (ou .mp4.parcial, interno).
pub fn nome_valido(nome: &str) -> bool {
    !nome.is_empty() && nome.len() <= 255 && !nome.contains('/') && !nome.contains('\\') && !nome.contains("..")
        && !nome.starts_with('.') && nome.ends_with(".mp4")
}

fn mp4s(dir: &Path) -> Vec<(String, u64)> {
    std::fs::read_dir(dir).map(|it| it.flatten().filter_map(|e| {
        let nome = e.file_name().to_string_lossy().to_string();
        let md = e.metadata().ok()?;
        (md.is_file() && nome_valido(&nome)).then(|| (nome, md.len()))
    }).collect()).unwrap_or_default()
}

#[tauri::command]
pub fn copias_estado(app: AppHandle) -> Value {
    let c = ler_config(&app);
    let lista = pasta(&app).map(|p| mp4s(&p)).unwrap_or_default();
    json!({ "pasta": c.pasta, "existe": pasta(&app).is_ok(), "arquivos": lista.len(), "bytes": lista.iter().map(|x| x.1).sum::<u64>() })
}

#[tauri::command]
pub async fn copias_escolher_pasta(app: AppHandle, window: tauri::WebviewWindow) -> Result<Value, String> {
    if window.label() != "main" { return Err("janela_nao_autorizada".into()); }
    let app2 = app.clone();
    let escolhida = tauri::async_runtime::spawn_blocking(move || app2.dialog().file().set_title("Pasta das cópias da prévia do dn.estudio").blocking_pick_folder())
        .await.map_err(|e| e.to_string())?;
    let Some(p) = escolhida.and_then(|f| f.into_path().ok()) else { return Ok(json!({ "pasta": ler_config(&app).pasta })) };
    gravar_config(&app, &Config { pasta: Some(p.to_string_lossy().to_string()) })?;
    Ok(json!({ "pasta": p.to_string_lossy() }))
}

#[tauri::command]
pub fn copias_voltar(app: AppHandle) -> Result<(), String> { gravar_config(&app, &Config::default()) }

#[tauri::command]
pub fn copias_listar(app: AppHandle) -> Result<Value, String> {
    Ok(json!(mp4s(&pasta(&app)?).into_iter().map(|(nome, bytes)| json!({ "nome": nome, "bytes": bytes })).collect::<Vec<_>>()))
}

#[tauri::command]
pub async fn copias_ler(app: AppHandle, nome: String) -> Result<Response, String> {
    if !nome_valido(&nome) { return Err("nome_invalido".into()); }
    let f = pasta(&app)?.join(&nome);
    if !f.is_file() { return Err("nao_existe".into()); }
    // fora da thread da janela: um vídeo leve tem dezenas de MB
    tauri::async_runtime::spawn_blocking(move || std::fs::read(f)).await.map_err(|e| e.to_string())?.map(Response::new).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn copias_gravar(app: AppHandle, request: Request<'_>) -> Result<(), String> {
    let cab = |k: &str| request.headers().get(k).and_then(|v| v.to_str().ok()).map(|s| s.to_string());
    let nome = cab("x-nome").ok_or("sem_nome")?;
    let prefixo = cab("x-prefixo").unwrap_or_default();
    if !nome_valido(&nome) { return Err("nome_invalido".into()); }
    let InvokeBody::Raw(bytes) = request.body() else { return Err("corpo_nao_binario".into()) };
    let dir = pasta(&app)?;
    // grava com outro nome e só depois troca: arquivo pela metade nunca vira cópia
    let parcial = dir.join(format!("{nome}.parcial"));
    std::fs::write(&parcial, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&parcial, dir.join(&nome)).map_err(|e| e.to_string())?;
    if !prefixo.is_empty() {
        for (outro, _) in mp4s(&dir) { if outro != nome && outro.starts_with(&prefixo) { let _ = std::fs::remove_file(dir.join(outro)); } }
    }
    Ok(())
}

// 0.8.15: a página publicada fala com a casca pelo IPC de texto (postMessage) — corpo binário chegava como JSON
// ("corpo_nao_binario"). Gravar e ler vão em pedaços base64 de até 8 MB.
use base64::Engine as _;
const B64: base64::engine::general_purpose::GeneralPurpose = base64::engine::general_purpose::STANDARD;

#[tauri::command]
pub async fn copias_gravar_parte(app: AppHandle, nome: String, dados: String, inicio: bool) -> Result<(), String> {
    if !nome_valido(&nome) { return Err("nome_invalido".into()); }
    let parcial = pasta(&app)?.join(format!("{nome}.parcial"));
    let bytes = B64.decode(dados.as_bytes()).map_err(|_| "base64_invalido")?;
    if bytes.len() > 12 * 1024 * 1024 { return Err("parte_grande_demais".into()); }
    tauri::async_runtime::spawn_blocking(move || -> std::io::Result<()> {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().create(true).write(true).append(!inicio).truncate(inicio).open(parcial)?;
        f.write_all(&bytes)
    }).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())
}

#[tauri::command]
pub fn copias_gravar_fim(app: AppHandle, nome: String, prefixo: String) -> Result<(), String> {
    if !nome_valido(&nome) { return Err("nome_invalido".into()); }
    let dir = pasta(&app)?;
    let parcial = dir.join(format!("{nome}.parcial"));
    if !parcial.is_file() { return Err("sem_parcial".into()); }
    std::fs::rename(&parcial, dir.join(&nome)).map_err(|e| e.to_string())?;
    if !prefixo.is_empty() {
        for (outro, _) in mp4s(&dir) { if outro != nome && outro.starts_with(&prefixo) { let _ = std::fs::remove_file(dir.join(outro)); } }
    }
    Ok(())
}

#[tauri::command]
pub async fn copias_ler_parte(app: AppHandle, nome: String, inicio: u64, tamanho: u64) -> Result<Value, String> {
    if !nome_valido(&nome) { return Err("nome_invalido".into()); }
    let f = pasta(&app)?.join(&nome);
    if !f.is_file() { return Err("nao_existe".into()); }
    let tamanho = tamanho.min(8 * 1024 * 1024);
    tauri::async_runtime::spawn_blocking(move || -> std::io::Result<Value> {
        use std::io::{Read, Seek, SeekFrom};
        let mut a = std::fs::File::open(f)?;
        let total = a.metadata()?.len();
        a.seek(SeekFrom::Start(inicio.min(total)))?;
        let mut buf = vec![0u8; tamanho.min(total.saturating_sub(inicio)) as usize];
        a.read_exact(&mut buf)?;
        Ok(json!({ "dados": B64.encode(&buf), "total": total }))
    }).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())
}

#[tauri::command]
pub fn copias_apagar(app: AppHandle, nome: String) -> Result<(), String> {
    if !nome_valido(&nome) { return Err("nome_invalido".into()); }
    let f = pasta(&app)?.join(nome);
    if f.is_file() { std::fs::remove_file(f).map_err(|e| e.to_string())?; }
    Ok(())
}

#[cfg(test)]
mod testes {
    use super::nome_valido;
    #[test]
    fn so_mp4_direto_na_pasta() {
        assert!(nome_valido("meu-video--leve--IMG_3075.MOV--1791-56180586.mp4"));
        assert!(!nome_valido("../fora.mp4"));
        assert!(!nome_valido("sub/pasta.mp4"));
        assert!(!nome_valido("a\\b.mp4"));
        assert!(!nome_valido(".escondido.mp4"));
        assert!(!nome_valido("documento.pdf"));
        assert!(!nome_valido(""));
    }
}
