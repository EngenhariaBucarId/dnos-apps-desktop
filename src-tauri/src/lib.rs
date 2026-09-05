//! dn.os Desktop — a casca.
//!
//! O que ela faz, e só isso:
//! - abre a instância web do dn.os (por padrão `https://dnos.dnia.ai`; um remix
//!   define `DNOS_URL`) numa janela nativa, com ícone, menu e instância única;
//! - entrega notificação nativa e deep link `dnos://…` para a página;
//! - abre links externos no navegador do sistema, não dentro da casca;
//! - baixa e instala atualização da própria casca em segundo plano.
//!
//! O conteúdo é o app web: quando o dn.os publica, a casca mostra a versão
//! nova sem reinstalar. A casca só volta a ser distribuída quando ela mesma muda.

use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_deep_link::DeepLinkExt;

const URL_PADRAO: &str = "https://dnos.dnia.ai";

/// URL da instância: `DNOS_URL` no ambiente vence o padrão da dn.ia.
fn url_da_instancia() -> String {
    std::env::var("DNOS_URL")
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| URL_PADRAO.to_string())
}

/// Domínios que podem carregar DENTRO da casca. Tudo o mais abre no navegador.
fn navegacao_interna(url: &url::Url, host_da_instancia: &str) -> bool {
    let host = url.host_str().unwrap_or("");
    if url.scheme() == "tauri" || url.scheme() == "asset" || host == "tauri.localhost" {
        return true; // a página local (carregando / sem conexão)
    }
    host == host_da_instancia
        || host.ends_with(".supabase.co")
        || host.ends_with(".lovable.app")
        || host.ends_with(".lovableproject.com")
        || host == "accounts.google.com"
        || host == "localhost"
}

/// Injetado em toda página (local e remota) antes de qualquer script dela.
/// 1) diz à página qual é a instância; 2) manda links `target=_blank` e
/// `window.open` para o navegador do sistema, pela ponte do Tauri.
const SCRIPT_INICIAL: &str = r#"
(() => {
  const externo = (href) => {
    try {
      const u = new URL(href, location.href);
      if (u.origin === location.origin) return false;
      const t = window.__TAURI__;
      if (t && t.opener && t.opener.openUrl) { t.opener.openUrl(u.toString()); return true; }
    } catch {}
    return false;
  };
  const abrirOriginal = window.open;
  window.open = function (href, alvo, feats) {
    if (href && externo(String(href))) return null;
    return abrirOriginal.call(window, href, alvo, feats);
  };
  document.addEventListener("click", (e) => {
    const a = e.target && e.target.closest ? e.target.closest("a[href]") : null;
    if (!a) return;
    const alvoNovaAba = a.target === "_blank" || e.metaKey || e.ctrlKey;
    if (alvoNovaAba && externo(a.href)) { e.preventDefault(); e.stopPropagation(); }
  }, true);
  window.__DNOS_DESKTOP__ = { versao: "0.1.0" };
})();
"#;

/// `dnos://chat/lia` → `https://<instância>/chat/lia`. Sem caminho, abre a raiz.
fn destino_do_deep_link(base: &str, link: &str) -> Option<String> {
    let u = url::Url::parse(link).ok()?;
    if u.scheme() != "dnos" {
        return None;
    }
    let mut caminho = String::new();
    if let Some(h) = u.host_str() {
        caminho.push('/');
        caminho.push_str(h);
    }
    caminho.push_str(u.path());
    let mut alvo = format!("{}{}", base, if caminho.is_empty() { "/" } else { &caminho });
    if let Some(q) = u.query() {
        alvo.push('?');
        alvo.push_str(q);
    }
    Some(alvo)
}

fn tratar_deep_links(app: &tauri::AppHandle, links: Vec<String>) {
    let base = url_da_instancia();
    let _ = app.emit("dnos://deep-link", links.clone());
    if let Some(janela) = app.get_webview_window("main") {
        let _ = janela.show();
        let _ = janela.set_focus();
        if let Some(alvo) = links.iter().find_map(|l| destino_do_deep_link(&base, l)) {
            if let Ok(u) = url::Url::parse(&alvo) {
                let _ = janela.navigate(u);
            }
        }
    }
}

#[cfg(desktop)]
fn atualizar_em_segundo_plano(app: tauri::AppHandle) {
    use tauri_plugin_updater::UpdaterExt;
    tauri::async_runtime::spawn(async move {
        let updater = match app.updater() {
            Ok(u) => u,
            Err(_) => return,
        };
        match updater.check().await {
            Ok(Some(atualizacao)) => {
                let versao = atualizacao.version.clone();
                // Baixa e instala em silêncio; a versão nova entra na próxima abertura.
                // A página recebe o evento e pode oferecer "reiniciar agora".
                if atualizacao.download_and_install(|_, _| {}, || {}).await.is_ok() {
                    let _ = app.emit("dnos://atualizacao-pronta", versao);
                }
            }
            Ok(None) => {}
            Err(e) => eprintln!("[dnos-desktop] verificação de atualização falhou: {e}"),
        }
    });
}

pub fn run() {
    let mut builder = tauri::Builder::default()
        // Segunda instância (clicar no ícone de novo, ou deep link com o app
        // aberto no Windows/Linux): traz a janela e repassa os links.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            let links: Vec<String> = argv.into_iter().filter(|a| a.starts_with("dnos://")).collect();
            if links.is_empty() {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            } else {
                tratar_deep_links(app, links);
            }
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_os::init());

    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    }

    builder
        .setup(|app| {
            // No Windows e Linux o esquema `dnos://` precisa ser registrado em runtime
            // (no macOS o instalador já registra pelo Info.plist).
            #[cfg(any(windows, target_os = "linux"))]
            {
                let _ = app.deep_link().register_all();
            }

            let base = url_da_instancia();
            let host = url::Url::parse(&base)
                .ok()
                .and_then(|u| u.host_str().map(|h| h.to_string()))
                .unwrap_or_default();
            let script = format!("window.__DNOS_URL__ = {};{}", serde_json::to_string(&base)?, SCRIPT_INICIAL);

            let handle_nav = app.handle().clone();
            let host_nav = host.clone();
            WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("dn.os")
                .inner_size(1360.0, 860.0)
                .min_inner_size(900.0, 600.0)
                .initialization_script(&script)
                .on_navigation(move |url| {
                    if navegacao_interna(url, &host_nav) {
                        return true;
                    }
                    // Fora da instância: abre no navegador do sistema e não navega aqui.
                    let _ = tauri_plugin_opener::open_url(url.to_string(), None::<&str>);
                    let _ = &handle_nav;
                    false
                })
                .build()?;

            // Deep link com o app já aberto (macOS entrega por aqui).
            let handle = app.handle().clone();
            app.deep_link().on_open_url(move |evento| {
                let links: Vec<String> = evento.urls().iter().map(|u| u.to_string()).collect();
                tratar_deep_links(&handle, links);
            });
            // Deep link que ABRIU o app.
            if let Ok(Some(urls)) = app.deep_link().get_current() {
                let links: Vec<String> = urls.iter().map(|u| u.to_string()).collect();
                if !links.is_empty() {
                    tratar_deep_links(app.handle(), links);
                }
            }

            #[cfg(desktop)]
            atualizar_em_segundo_plano(app.handle().clone());

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("erro ao iniciar o dn.os Desktop");
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn deep_link_vira_rota_da_instancia() {
        assert_eq!(
            destino_do_deep_link("https://dnos.dnia.ai", "dnos://chat/lia").as_deref(),
            Some("https://dnos.dnia.ai/chat/lia")
        );
        assert_eq!(
            destino_do_deep_link("https://dnos.dnia.ai", "dnos://settings?tab=documentation").as_deref(),
            Some("https://dnos.dnia.ai/settings?tab=documentation")
        );
        assert_eq!(destino_do_deep_link("https://dnos.dnia.ai", "https://outro.com"), None);
    }

    #[test]
    fn so_a_instancia_e_o_login_carregam_dentro() {
        let h = "dnos.dnia.ai";
        assert!(navegacao_interna(&url::Url::parse("https://dnos.dnia.ai/chat").unwrap(), h));
        assert!(navegacao_interna(&url::Url::parse("https://zozy.supabase.co/auth/v1/verify").unwrap(), h));
        assert!(navegacao_interna(&url::Url::parse("https://accounts.google.com/o/oauth2").unwrap(), h));
        assert!(!navegacao_interna(&url::Url::parse("https://www.capcut.com/").unwrap(), h));
    }
}
