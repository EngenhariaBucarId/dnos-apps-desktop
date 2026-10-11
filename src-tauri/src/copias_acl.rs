//! As cópias da prévia só respondem à página publicada, na janela principal.
use tauri::{ipc::{CallbackFn, InvokeBody}, webview::InvokeRequest, WebviewUrl, WebviewWindowBuilder};

#[tauri::command]
fn copias_estado() -> &'static str { "handler-alcancado" }

fn permitido(janela: &str, origem: &str) -> bool {
    let app = tauri::test::mock_builder()
        .invoke_handler(tauri::generate_handler![copias_estado])
        .build(crate::contexto())
        .expect("contexto de produção");
    let view = WebviewWindowBuilder::new(&app, janela, WebviewUrl::App("index.html".into())).build().unwrap();
    tauri::test::get_ipc_response(&view, InvokeRequest {
        cmd: "copias_estado".into(), callback: CallbackFn(0), error: CallbackFn(1),
        url: origem.parse().unwrap(), body: InvokeBody::default(), headers: Default::default(),
        invoke_key: tauri::test::INVOKE_KEY.to_string(),
    }).is_ok()
}

#[test]
fn pagina_publicada_alcanca_as_copias() { assert!(permitido("main", "https://dnos.dnia.ai/estudio")); }
#[test]
fn outra_origem_ou_janela_nao_alcanca() {
    assert!(!permitido("main", "https://example.com/estudio"));
    assert!(!permitido("barra-mac", "https://dnos.dnia.ai/estudio"));
}
