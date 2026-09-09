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

use tauri::{Emitter, Listener, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_deep_link::DeepLinkExt;

mod barra;
mod gravador;
mod meu_chrome;
mod roteiro;
mod voz;

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
    // O filtro roda para TODO quadro, não só a janela (wry no macOS não separa
    // iframe de página). O artefato do dn.os é um iframe `srcdoc` — URL
    // `about:srcdoc`, sem host — e era cancelado aqui: o painel abria vazio
    // (06/09). Esquemas internos do próprio documento passam sempre.
    if matches!(url.scheme(), "about" | "blob" | "data" | "javascript") {
        return true;
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
  // Abre no navegador do sistema por três caminhos, do mais direto ao mais
  // robusto: API do plugin (quando injetada), invoke do core (sempre existe
  // com withGlobalTauri), evento que o Rust ouve (dnos://abrir-url). Cada
  // tentativa vai para o diário (meu-chrome.log) — em 08/09 os links não
  // abriam e não havia rastro de qual degrau falhou.
  const diario = (o) => { try { window.__TAURI__.event.emit("dnos://diario", o); } catch {} };
  // Rastro de erro da página no diário (08/09: a casca 0.5.7 abriu em branco e
  // não havia como saber o que o WKWebView recusou). Erro de script, promessa
  // rejeitada e recurso que não carregou vão para meu-chrome.log.
  window.addEventListener("error", (e) => {
    const alvo = e && e.target && e.target !== window && e.target.tagName ? (e.target.tagName + " " + (e.target.src || e.target.href || "")) : null;
    diario({ erroPagina: alvo ? ("recurso não carregou: " + alvo) : String(e && e.message || e), onde: e && e.filename ? (e.filename + ":" + e.lineno) : undefined });
  }, true);
  window.addEventListener("unhandledrejection", (e) => { diario({ promessaRejeitada: String(e && e.reason && (e.reason.message || e.reason) || e) }); });
  const abrirFora = (url) => {
    const t = window.__TAURI__;
    // Primeiro o Rust (evento ouvido em lib.rs): o `open_url` nativo não passa
    // pelo escopo de URLs da capability — a 0.5.7 morria aqui com "Not allowed
    // to open url", rejeição assíncrona que o script não via (diário 08/09).
    try { if (t && t.event && t.event.emit) { t.event.emit("dnos://abrir-url", url); diario({ link: url, via: "evento" }); return true; } } catch (e) { diario({ link: url, via: "evento", erro: String(e) }); }
    try { if (t && t.opener && t.opener.openUrl) { t.opener.openUrl(url).catch((e) => diario({ link: url, via: "opener", erro: String(e) })); diario({ link: url, via: "opener" }); return true; } } catch (e) { diario({ link: url, via: "opener", erro: String(e) }); }
    try { if (t && t.core && t.core.invoke) { t.core.invoke("plugin:opener|open_url", { url }).catch((e) => diario({ link: url, via: "invoke", erro: String(e) })); diario({ link: url, via: "invoke" }); return true; } } catch (e) { diario({ link: url, via: "invoke", erro: String(e) }); }
    // Último recurso: navega nesta janela; o on_navigation do Rust cancela e
    // abre no navegador. Nunca fica no vazio como o target=_blank do WKWebView.
    try { location.assign(url); diario({ link: url, via: "navegacao" }); return true; } catch {}
    return false;
  };
  const externo = (href) => {
    try {
      const u = new URL(href, location.href);
      if (u.origin === location.origin) return false;
      if (!/^https?:$|^mailto:$|^tel:$/.test(u.protocol)) return false;
      return abrirFora(u.toString());
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
  window.__DNOS_DESKTOP__ = { versao: "0.5.9", meuChrome: true, gravador: true, roteiro: true };
})();
"#;

/// Rodado ao fim de cada carga de página (ver `on_page_load`).
const SCRIPT_APRESENTACAO: &str = r#"
(() => {
  // O script inicial rodou nesta página? E a ponte do Tauri chegou?
  const tinhaFlag = !!window.__DNOS_DESKTOP__, temTauri = !!window.__TAURI__;
  window.__DNOS_DESKTOP__ = Object.assign({ versao: "0.5.9", meuChrome: true, gravador: true, roteiro: true }, window.__DNOS_DESKTOP__ || {}, { meuChrome: true, gravador: true, roteiro: true });
  try { window.dispatchEvent(new CustomEvent("dnos-desktop", { detail: window.__DNOS_DESKTOP__ })); } catch {}
  let recarregou = false;
  if ((!tinhaFlag || !temTauri) && location.protocol.startsWith("http")) {
    try {
      if (!sessionStorage.getItem("dnos-recarregou")) { sessionStorage.setItem("dnos-recarregou", "1"); recarregou = true; }
    } catch {}
  }
  try { if (temTauri) window.__TAURI__.event.emit("dnos://diario", { pagina: location.pathname, tinhaFlag, temTauri, recarregou }); } catch {}
  // 4 s depois: o app montou? (#root com filhos). Se não, o diário diz quantos
  // scripts a página tem e o que o WKWebView contou — é o rastro do "abriu em branco".
  if (temTauri && location.protocol.startsWith("http")) setTimeout(() => {
    try {
      const raiz = document.getElementById("root");
      const scripts = [...document.scripts].map((s) => (s.src || "inline").replace(location.origin, "")).slice(0, 6);
      const montou = !!(raiz && raiz.childElementCount);
      window.__TAURI__.event.emit("dnos://diario", { montou, filhosDoRoot: raiz ? raiz.childElementCount : -1, scripts, titulo: document.title, ua: navigator.userAgent.slice(0, 80) });
      // Não montou: recarrega sem cache até 2 vezes (um service worker ou um
      // bundle pela metade não pode deixar a casca em branco para sempre).
      if (!montou) {
        const n = Number(sessionStorage.getItem("dnos-tentativas") || "0");
        if (n < 2) { sessionStorage.setItem("dnos-tentativas", String(n + 1)); window.__TAURI__.event.emit("dnos://diario", { recarregandoPorVazio: n + 1 }); setTimeout(() => location.replace(location.pathname + "?_r=" + Date.now()), 300); }
      } else { sessionStorage.removeItem("dnos-tentativas"); }
    } catch {}
  }, 4000);
  if (recarregou) setTimeout(() => location.reload(), 50);
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
                // Arrastar arquivo para o chat (07/09): por padrão a casca
                // captura o drop e o transforma em evento próprio, e a página
                // nunca recebe o `drop` do HTML5 — no web funcionava, no app
                // não. Desligado, o drop chega à página como no navegador.
                .disable_drag_drop_handler()
                .initialization_script(&script)
                // Rede de segurança (05/09): na primeira carga da instância a
                // página às vezes não enxergou __DNOS_DESKTOP__ (o botão Meu
                // Chrome só aparecia depois de um deep link). Ao terminar de
                // carregar qualquer página, a casca se apresenta de novo por
                // eval e avisa a página; se nem o __TAURI__ chegou, recarrega
                // uma vez.
                .on_page_load(|webview, payload| {
                    if matches!(payload.event(), tauri::webview::PageLoadEvent::Finished) {
                        let _ = webview.eval(SCRIPT_APRESENTACAO);
                    }
                })
                .on_navigation(move |url| {
                    if navegacao_interna(url, &host_nav) {
                        return true;
                    }
                    // Fora da instância: abre no navegador do sistema e não navega aqui.
                    if let Err(e) = tauri_plugin_opener::open_url(url.to_string(), None::<&str>) {
                        eprintln!("[dnos-desktop] não abriu {url}: {e}");
                    }
                    let _ = &handle_nav;
                    false
                })
                .build()?;

            // Badge de não lidas no ícone: a página emite `dnos://badge` com o
            // número e a casca marca o dock / barra de tarefas. Feito aqui, em
            // Rust, porque o caminho pela API JavaScript dependia de versão e
            // permissão e não apareceu (05/09).
            let handle_badge = app.handle().clone();
            app.listen_any("dnos://badge", move |evento| {
                let n: u64 = serde_json::from_str::<serde_json::Value>(evento.payload())
                    .ok()
                    .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
                    .unwrap_or(0);
                if let Some(w) = handle_badge.get_webview_window("main") {
                    let _ = w.set_badge_count(if n > 0 { Some(n as i64) } else { None });
                }
            });

            // Meu Chrome (fase 2): a página liga/desliga por evento; a casca
            // abre o Chrome com perfil próprio e mantém a ponte com a VPS.
            meu_chrome::instalar(app.handle());
            // Aprenda comigo (fase 3a): gravador de demonstrações no Chrome do dn.os.
            gravador::instalar(app.handle());
            // Barra dentro do Chrome da pessoa e voz para notas.
            barra::instalar(app.handle());
            voz::instalar(app.handle());
            // Roteiro: executa os passos mecânicos de uma habilidade direto no Chrome da pessoa.
            roteiro::instalar(app.handle());

            // Diário: a página conta como cada carga chegou (script inicial
            // rodou? ponte do Tauri presente?) — vai para meu-chrome.log.
            let handle_diario = app.handle().clone();
            app.listen_any("dnos://diario", move |evento| {
                meu_chrome::registrar(&handle_diario, &format!("carga: {}", evento.payload()));
            });

            // Link externo pedido pela página (terceiro caminho do script inicial,
            // quando a API JS do opener não está disponível na página remota).
            app.listen_any("dnos://abrir-url", move |evento| {
                let url = serde_json::from_str::<serde_json::Value>(evento.payload())
                    .ok()
                    .and_then(|v| v.as_str().map(|s| s.to_string()))
                    .unwrap_or_else(|| evento.payload().trim_matches('"').to_string());
                if url.starts_with("http://") || url.starts_with("https://") || url.starts_with("mailto:") || url.starts_with("tel:") {
                    if let Err(e) = tauri_plugin_opener::open_url(url.clone(), None::<&str>) {
                        eprintln!("[dnos-desktop] não abriu {url}: {e}");
                    }
                }
            });

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
        .build(tauri::generate_context!())
        .expect("erro ao iniciar o dn.os Desktop")
        .run(|app, evento| {
            // Fechou o app: o Chrome do dn.os fecha junto e a ponte cai.
            if let tauri::RunEvent::Exit = evento {
                if let Some(estado) = app.try_state::<meu_chrome::Compartilhado>() {
                    meu_chrome::parar(app, &estado, "o dn.os fechou");
                }
            }
        });
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
