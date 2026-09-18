//! Regressão da 0.7.13: registrar o handler não autoriza uma página remota.
//! O probe evita captura de tela; a checagem IPC/capabilities é a real do Tauri.
use tauri::{ipc::{CallbackFn, InvokeBody}, webview::InvokeRequest, WebviewUrl, WebviewWindowBuilder};

#[tauri::command]
fn olhar_computador() -> &'static str { "handler-alcancado" }

#[tauri::command]
fn explorar_computador() -> &'static str { "handler-alcancado" }

fn permitido_cmd(cmd: &str, janela: &str, origem: &str) -> bool {
    let app = tauri::test::mock_builder()
        .invoke_handler(tauri::generate_handler![olhar_computador, explorar_computador])
        .build(crate::contexto())
        .expect("contexto de produção");
    let view = WebviewWindowBuilder::new(&app, janela, WebviewUrl::App("index.html".into())).build().unwrap();
    tauri::test::get_ipc_response(&view, InvokeRequest {
        cmd: cmd.into(), callback: CallbackFn(0), error: CallbackFn(1),
        url: origem.parse().unwrap(), body: InvokeBody::default(), headers: Default::default(),
        invoke_key: tauri::test::INVOKE_KEY.to_string(),
    }).is_ok()
}

#[test]
fn pagina_publicada_alcanca_olhar_pelo_ipc() {
    assert!(permitido("main", "https://dnos.dnia.ai/chat?agent=kira"));
}
#[test]
fn barra_local_pode_chamar_parar() {
    assert!(permitido("barra-mac", "tauri://localhost/barra-mac.html"));
}
#[test]
fn outra_origem_nao_alcanca_olhar() {
    assert!(!permitido("main", "https://example.com/chat"));
    assert!(!permitido("main", "https://dnos.dnia.ai.example.com/chat"));
}
#[test]
fn outra_janela_nao_alcanca_olhar() {
    assert!(!permitido("janela-estranha", "https://dnos.dnia.ai/chat"));
}

fn permitido(janela: &str, origem: &str) -> bool { permitido_cmd("olhar_computador",janela,origem) }
#[test]
fn exploracao_usa_as_mesmas_fronteiras_ipc() {
 assert!(permitido_cmd("explorar_computador","main","https://dnos.dnia.ai/chat"));
 assert!(permitido_cmd("explorar_computador","barra-mac","tauri://localhost/barra-mac.html"));
 assert!(!permitido_cmd("explorar_computador","main","https://example.com"));
 assert!(!permitido_cmd("explorar_computador","estranha","https://dnos.dnia.ai/chat"));
}
