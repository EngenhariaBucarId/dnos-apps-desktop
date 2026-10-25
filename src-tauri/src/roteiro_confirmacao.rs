//! Confirmação local antes de executar um roteiro (FD-07, 25/10/2026).
//!
//! 25/10: a página emitia `dnos://roteiro/executar` e a casca saía clicando e
//! digitando no computador da pessoa (CapCut, Finder, Chrome…) na hora. Com
//! qualquer falha de script na página (XSS), quem a explorasse mexia no
//! computador. Agora TODO roteiro — da página ou do mind pelo relay (nó do
//! Mac) — passa por aqui antes de rodar: a casca mostra um diálogo NATIVO do
//! sistema (plugin de diálogo do Tauri: NSAlert no Mac, TaskDialog no Windows,
//! zenity no Linux), que a página não desenha nem clica.
//!
//! Não há exceção para "a pessoa acabou de clicar em Executar": o evento chega
//! igual de um clique e de um script, e a casca não tem como saber qual foi.
//! Para não cansar no uso normal, "Permitir sempre" libera esta habilidade com
//! EXATAMENTE os mesmos passos até fechar o app (só na memória da casca).
//!
//! Quem executa (`roteiro::executar` e `maquina::executar`) exige um
//! `Autorizado`, que só este módulo cria: um caminho novo que esqueça a
//! confirmação não compila.

use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind, MessageDialogResult};

/// Motivo devolvido à página (e ao agente, pelo relay) quando a pessoa recusa.
pub const RECUSADO: &str = "recusado pela pessoa";
/// Motivo quando já há um diálogo aberto esperando resposta.
pub const JA_ESPERANDO: &str = "já há um pedido de roteiro esperando a sua confirmação no computador";

const BOTAO_PERMITIR: &str = "Permitir";
const BOTAO_SEMPRE: &str = "Permitir sempre (até fechar o app)";
const BOTAO_RECUSAR: &str = "Recusar";

/// Prova de que a pessoa permitiu (ou já tinha permitido nesta sessão).
/// O campo privado impede criar um fora deste módulo.
pub struct Autorizado { _p: () }

/// De onde veio o pedido. Só muda o texto do diálogo: os dois confirmam igual.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origem {
    /// `dnos://roteiro/executar` emitido pela página (ou por qualquer janela).
    Pagina,
    /// `{t:"roteiro"}` vindo do relay da VPS pelo nó do Mac (maquina.rs).
    Relay,
}

#[derive(Default)]
pub struct Confirmacoes {
    /// Um diálogo por vez: enquanto um espera, os outros pedidos voltam com erro
    /// (25/10: evita uma pilha de diálogos para a pessoa aceitar sem ler).
    esperando: bool,
    /// "Permitir sempre": chave exata do roteiro (ver `chave`). Some ao fechar o app.
    liberados: HashSet<String>,
}
pub type Compartilhado = Arc<Mutex<Confirmacoes>>;

pub fn instalar(app: &AppHandle) {
    app.manage::<Compartilhado>(Arc::new(Mutex::new(Confirmacoes::default())));
}

#[derive(Debug, PartialEq, Eq)]
enum Resposta { Permitir, PermitirSempre, Recusar }

/// Só os rótulos exatos de permitir valem; qualquer outra coisa (fechar a
/// janela, Esc, zenity ausente no Linux, código inesperado) é recusa.
fn interpretar(r: &MessageDialogResult) -> Resposta {
    match r {
        MessageDialogResult::Custom(s) if s == BOTAO_PERMITIR => Resposta::Permitir,
        MessageDialogResult::Custom(s) if s == BOTAO_SEMPRE => Resposta::PermitirSempre,
        _ => Resposta::Recusar,
    }
}

fn eh_mac(pedido: &Value) -> bool { pedido["modo"].as_str() == Some("mac") }

/// Passos que de fato vão rodar. No Chrome, `roteiro::executar` só aceita estes
/// tipos; no computador inteiro, o ajudante recebe todos.
fn passos_executaveis(pedido: &Value) -> Vec<&Value> {
    let todos = pedido["passos"].as_array().map(|a| a.iter().collect::<Vec<_>>()).unwrap_or_default();
    if eh_mac(pedido) { return todos; }
    todos.into_iter().filter(|p| matches!(p["t"].as_str(), Some("navegar" | "aba" | "clique" | "digitar" | "tecla" | "enviar"))).collect()
}

/// Identidade do roteiro para "Permitir sempre": modo, nome, app e os passos
/// inteiros. Mudou um passo (ou o texto digitado), pergunta de novo.
fn chave(pedido: &Value) -> String {
    json!({
        "modo": if eh_mac(pedido) { "mac" } else { "chrome" },
        "nome": pedido["nome"], "app": pedido["app"], "bundle": pedido["bundle"],
        "passos": passos_executaveis(pedido),
    }).to_string()
}

fn precisa_confirmar(c: &Confirmacoes, pedido: &Value) -> bool {
    !passos_executaveis(pedido).is_empty() && !c.liberados.contains(&chave(pedido))
}

/// Texto vindo da página/relay no diálogo: sem quebras de linha, controles,
/// caracteres invisíveis nem inversão de direção (25/10: um nome com "\n\n" ou
/// U+202E poderia forjar linhas do diálogo), e curto.
fn limpo(s: &str, max: usize) -> String {
    let invisivel = |c: char| matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}');
    let mut saida = String::new();
    let mut espaco = false;
    for c in s.chars() {
        if c.is_control() || c.is_whitespace() { espaco = true; continue; }
        if invisivel(c) { continue; }
        if espaco && !saida.is_empty() { saida.push(' '); }
        espaco = false;
        saida.push(c);
    }
    if saida.chars().count() > max { saida = saida.chars().take(max.saturating_sub(1)).collect::<String>() + "…"; }
    saida
}

/// Lista curta: "a, b, c e mais 2".
fn lista(itens: &[String], max: usize) -> String {
    let mut s = itens.iter().take(max).cloned().collect::<Vec<_>>().join(", ");
    if itens.len() > max { s += &format!(" e mais {}", itens.len() - max); }
    s
}

fn sem_repetir(v: &mut Vec<String>, item: String) {
    if !item.is_empty() && !v.iter().any(|x| x.to_lowercase() == item.to_lowercase()) { v.push(item); }
}

/// Título e corpo do diálogo.
fn texto_do_dialogo(pedido: &Value, origem: Origem) -> (String, String) {
    let passos = passos_executaveis(pedido);
    let nome = limpo(pedido["nome"].as_str().unwrap_or(""), 80);
    let nome = if nome.is_empty() { "(sem nome)".to_string() } else { nome };
    let agente = limpo(pedido["agente"].as_str().unwrap_or(""), 40);
    let quem = match (agente.is_empty(), origem) {
        (false, Origem::Relay) => format!("{agente} (mind, pela conversa no dn.os)"),
        (false, Origem::Pagina) => format!("{agente} (mind)"),
        (true, Origem::Relay) => "um mind do dn.os, pela conversa".to_string(),
        (true, Origem::Pagina) => "a tela do dn.os".to_string(),
    };

    let mut linhas = vec![format!("Quem pediu: {quem}"), format!("Habilidade: {nome}")];
    if eh_mac(pedido) {
        // O roteiro pode trocar de app no meio: lista todos os que aparecem.
        let mut apps = Vec::new();
        sem_repetir(&mut apps, limpo(pedido["app"].as_str().unwrap_or(""), 40));
        for p in &passos { sem_repetir(&mut apps, limpo(p["app"]["nome"].as_str().or(p["app"]["bundle"].as_str()).unwrap_or(""), 40)); }
        if apps.is_empty() { sem_repetir(&mut apps, limpo(pedido["bundle"].as_str().unwrap_or(""), 60)); }
        let rotulo = if apps.len() > 1 { "Programas" } else { "Programa" };
        linhas.push(format!("{rotulo}: {}", if apps.is_empty() { "o app que estiver na frente".to_string() } else { lista(&apps, 4) }));
    } else {
        linhas.push("Programa: Google Chrome (Meu navegador)".to_string());
        let mut sites = Vec::new();
        for p in &passos {
            if let Some(h) = p["url"].as_str().and_then(|u| url::Url::parse(u).ok()).and_then(|u| u.host_str().map(str::to_owned)) { sem_repetir(&mut sites, limpo(&h, 60)); }
        }
        if !sites.is_empty() { linhas.push(format!("Sites: {}", lista(&sites, 3))); }
    }
    linhas.push(format!("Passos: {} (cliques, teclas e digitação automáticos)", passos.len()));
    let digita: Vec<String> = passos.iter()
        .filter(|p| p["t"] == "digitar" && !p["senha"].as_bool().unwrap_or(false))
        .map(|p| limpo(p["valor"].as_str().unwrap_or(""), 40))
        .filter(|s| !s.is_empty())
        .map(|s| format!("\"{s}\""))
        .collect();
    if !digita.is_empty() { linhas.push(format!("Vai digitar: {}", lista(&digita, 3))); }

    let titulo = "Permitir que o dn.os use o seu computador?".to_string();
    let corpo = format!(
        "{}\n\nO dn.os vai clicar e digitar sozinho no seu computador. Se você não pediu isto agora, clique em Recusar.\n\n\"Permitir sempre\" vale só para esta habilidade, com estes mesmos passos, até você fechar o dn.os.",
        linhas.join("\n")
    );
    (titulo, corpo)
}

/// Libera o `esperando` mesmo se a tarefa for cancelada no meio.
struct Esperando(Compartilhado);
impl Drop for Esperando {
    fn drop(&mut self) { if let Ok(mut g) = self.0.lock() { g.esperando = false; } }
}

/// Ponto único antes de qualquer execução de roteiro. `Err` traz o motivo para
/// devolver à página/agente no mesmo formato de erro do roteiro.
pub async fn autorizar(app: &AppHandle, pedido: &Value, origem: Origem) -> Result<Autorizado, &'static str> {
    let Some(estado) = app.try_state::<Compartilhado>().map(|e| e.inner().clone()) else { return Err(RECUSADO) };
    {
        let Ok(mut g) = estado.lock() else { return Err(RECUSADO) };
        if !precisa_confirmar(&g, pedido) { return Ok(Autorizado { _p: () }); }
        if g.esperando { return Err(JA_ESPERANDO); }
        g.esperando = true;
    }
    let _esperando = Esperando(estado.clone());
    let (titulo, corpo) = texto_do_dialogo(pedido, origem);
    crate::meu_chrome::registrar(app, &format!("roteiro: pedindo confirmação ({:?}) para \"{}\"", origem, limpo(pedido["nome"].as_str().unwrap_or(""), 60)));

    // Recusar no lugar de "cancelar": fechar a janela, Esc ou falha do diálogo caem nele.
    let mut dialogo = app.dialog().message(corpo).title(titulo).kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::YesNoCancelCustom(BOTAO_PERMITIR.into(), BOTAO_SEMPRE.into(), BOTAO_RECUSAR.into()));
    if let Some(janela) = app.get_webview_window("main") {
        // 25/10: aparece preso à janela do dn.os (folha no Mac), que volta a ficar
        // visível; sem roubar o foco de quem está digitando em outro app — o ícone
        // pede atenção (Dock/barra de tarefas) até a pessoa olhar.
        let _ = janela.unminimize();
        let _ = janela.show();
        let _ = janela.request_user_attention(Some(tauri::UserAttentionType::Critical));
        dialogo = dialogo.parent(&janela);
    }
    let (tx, rx) = tokio::sync::oneshot::channel::<MessageDialogResult>();
    dialogo.show_with_result(move |r| { let _ = tx.send(r); });
    let resposta = rx.await.map(|r| interpretar(&r)).unwrap_or(Resposta::Recusar);
    if let Some(janela) = app.get_webview_window("main") { let _ = janela.request_user_attention(None); }
    crate::meu_chrome::registrar(app, &format!("roteiro: confirmação → {resposta:?}"));
    match resposta {
        Resposta::Permitir => Ok(Autorizado { _p: () }),
        Resposta::PermitirSempre => {
            if let Ok(mut g) = estado.lock() { g.liberados.insert(chave(pedido)); }
            Ok(Autorizado { _p: () })
        }
        Resposta::Recusar => Err(RECUSADO),
    }
}

#[cfg(test)]
mod testes {
    use super::*;

    fn pedido_mac() -> Value {
        json!({ "modo": "mac", "nome": "Exportar vídeo", "agente": "Lia", "app": "CapCut", "bundle": "com.lemon.lvoverseas",
            "passos": [
                { "t": "app", "app": { "nome": "CapCut", "bundle": "com.lemon.lvoverseas" } },
                { "t": "clique", "ax": { "titulo": "Exportar" }, "app": { "nome": "CapCut" } },
                { "t": "digitar", "valor": "final.mp4", "app": { "nome": "Finder" } },
                { "t": "digitar", "valor": "segredo", "senha": true },
            ] })
    }

    #[test]
    fn sempre_confirma_por_padrao_inclusive_com_campo_de_clique() {
        let c = Confirmacoes::default();
        assert!(precisa_confirmar(&c, &pedido_mac()));
        // A casca não confia em campo da página dizendo que foi clique.
        let mut p = pedido_mac(); p["clique"] = json!(true); p["confirmado"] = json!(true); p["origem"] = json!("botao");
        assert!(precisa_confirmar(&c, &p));
        let chrome = json!({ "nome": "x", "passos": [{ "t": "navegar", "url": "https://a.com" }] });
        assert!(precisa_confirmar(&c, &chrome));
    }

    #[test]
    fn sem_passos_executaveis_nao_pergunta() {
        let c = Confirmacoes::default();
        assert!(!precisa_confirmar(&c, &json!({ "nome": "x", "passos": [] })));
        // No Chrome, tipo desconhecido não roda: nada a confirmar.
        assert!(!precisa_confirmar(&c, &json!({ "nome": "x", "passos": [{ "t": "observar" }] })));
        // No computador inteiro, o ajudante recebe tudo: confirma.
        assert!(precisa_confirmar(&c, &json!({ "modo": "mac", "nome": "x", "passos": [{ "t": "observar" }] })));
    }

    #[test]
    fn permitir_sempre_vale_so_para_os_mesmos_passos() {
        let mut c = Confirmacoes::default();
        c.liberados.insert(chave(&pedido_mac()));
        assert!(!precisa_confirmar(&c, &pedido_mac()));
        // id e agente mudam a cada pedido do relay: não contam.
        let mut p = pedido_mac(); p["id"] = json!("abc"); p["agente"] = json!("Kira");
        assert!(!precisa_confirmar(&c, &p));
        // Mesmo nome com um texto digitado diferente: pergunta de novo.
        let mut p = pedido_mac(); p["passos"][2]["valor"] = json!("rm -rf ~");
        assert!(precisa_confirmar(&c, &p));
        // Mesmos passos em outro modo (Chrome × computador): pergunta de novo.
        let mut p = pedido_mac(); p["modo"] = json!("chrome");
        assert!(precisa_confirmar(&c, &p));
        let mut p = pedido_mac(); p["bundle"] = json!("com.apple.Terminal");
        assert!(precisa_confirmar(&c, &p));
    }

    #[test]
    fn so_os_rotulos_de_permitir_liberam() {
        assert_eq!(interpretar(&MessageDialogResult::Custom(BOTAO_PERMITIR.into())), Resposta::Permitir);
        assert_eq!(interpretar(&MessageDialogResult::Custom(BOTAO_SEMPRE.into())), Resposta::PermitirSempre);
        assert_eq!(interpretar(&MessageDialogResult::Custom(BOTAO_RECUSAR.into())), Resposta::Recusar);
        for r in [MessageDialogResult::Ok, MessageDialogResult::Yes, MessageDialogResult::No, MessageDialogResult::Cancel, MessageDialogResult::Custom("permitir".into())] {
            assert_eq!(interpretar(&r), Resposta::Recusar, "{r:?}");
        }
    }

    #[test]
    fn texto_mostra_mind_habilidade_programas_e_digitacao() {
        let (titulo, corpo) = texto_do_dialogo(&pedido_mac(), Origem::Relay);
        assert!(titulo.contains("computador"));
        assert!(corpo.contains("Quem pediu: Lia (mind, pela conversa no dn.os)"));
        assert!(corpo.contains("Habilidade: Exportar vídeo"));
        assert!(corpo.contains("Programas: CapCut, Finder"));
        assert!(corpo.contains("Passos: 4"));
        assert!(corpo.contains("Vai digitar: \"final.mp4\""));
        assert!(!corpo.contains("segredo"), "senha não aparece");
        let (_, corpo) = texto_do_dialogo(&json!({ "nome": "Busca", "passos": [{ "t": "navegar", "url": "https://www.google.com/x" }, { "t": "clique" }] }), Origem::Pagina);
        assert!(corpo.contains("Quem pediu: a tela do dn.os"));
        assert!(corpo.contains("Programa: Google Chrome (Meu navegador)"));
        assert!(corpo.contains("Sites: www.google.com"));
    }

    #[test]
    fn texto_da_pagina_nao_forja_linhas_do_dialogo() {
        let p = json!({ "modo": "mac", "nome": "Oi\n\nQuem pediu: você\u{202E}gnp.exe\u{200B}", "agente": "Lia\r\nPrograma: Notas", "passos": [{ "t": "clique" }] });
        let (_, corpo) = texto_do_dialogo(&p, Origem::Pagina);
        let primeira = corpo.lines().next().unwrap();
        assert_eq!(primeira, "Quem pediu: Lia Programa: Notas (mind)");
        assert!(corpo.contains("Habilidade: Oi Quem pediu: vocêgnp.exe\n"));
        assert!(!corpo.contains('\u{202E}') && !corpo.contains('\u{200B}'));
        assert_eq!(limpo(&"a".repeat(100), 10).chars().count(), 10);
    }
}
