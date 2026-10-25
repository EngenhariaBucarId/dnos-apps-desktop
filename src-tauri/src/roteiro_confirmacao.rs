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
//!
//! 25/10 (0.8.22, cartão seg-explorar-sem-confirmar): a sessão de exploração
//! (`exploracao.rs`, o mind usando o computador sozinho por até 60 min) passa
//! pelo mesmo tipo de diálogo, só com Permitir / Recusar — autonomia por tempo
//! não tem "Permitir sempre". A sessão só nasce com uma `ExploracaoPermitida`.
//!
//! O agente não aprova o próprio diálogo: diálogo de confirmação e exploração
//! ativa nunca coexistem. Há uma vaga única de diálogo (`Vaga`) para os dois
//! tipos; a exploração só é criada segurando a vaga, e todo pedido de roteiro
//! que chega com uma exploração ativa é recusado sem abrir diálogo (nem o
//! "Permitir sempre" vale nesse tempo). Como reforço, a exploração não injeta
//! clique nem tecla enquanto a vaga estiver ocupada (`dialogo_aberto`).

use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind, MessageDialogResult};

/// Motivo devolvido à página (e ao agente, pelo relay) quando a pessoa recusa.
pub const RECUSADO: &str = "recusado pela pessoa";
/// Motivo quando já há um diálogo aberto esperando resposta.
pub const JA_ESPERANDO: &str = "já há um pedido esperando a sua confirmação no computador";
/// Roteiro pedido com uma exploração ativa: recusado sem diálogo (o agente,
/// com o mouse e o teclado, poderia responder o diálogo no lugar da pessoa).
pub const EXPLORACAO_ATIVA: &str = "há um mind usando o computador agora (exploração ativa); o roteiro só pode ser pedido depois que ela terminar";

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

/// Prova de que a pessoa permitiu ESTA sessão de exploração. Fica guardada na
/// sessão (`exploracao::Sessao`): sem ela a sessão não se monta.
pub struct ExploracaoPermitida { _p: () }

/// Resposta "Permitir" à exploração. Segura a vaga do diálogo até a sessão ser
/// criada: entre o clique e a sessão existir, nenhum outro diálogo abre.
pub struct AutorizadoExploracao { permitida: ExploracaoPermitida, _vaga: Vaga }
impl AutorizadoExploracao {
    /// A marca vai para a sessão; a vaga é solta quando o chamador a largar
    /// (depois de a sessão estar registrada).
    pub fn separar(self) -> (ExploracaoPermitida, Vaga) { (self.permitida, self._vaga) }
}

/// Um diálogo por vez, de roteiro OU de exploração (25/10: evita uma pilha de
/// diálogos para a pessoa aceitar sem ler, e é a trava que impede diálogo e
/// exploração ativa ao mesmo tempo). Atômico, e não dentro de um Mutex, para
/// soltar sem trava e para a exploração consultar sem risco de deadlock.
static VAGA: AtomicBool = AtomicBool::new(false);

/// Dono da vaga do diálogo; soltá-la é largar este valor.
pub struct Vaga { _p: () }
impl Drop for Vaga {
    fn drop(&mut self) { VAGA.store(false, Ordering::SeqCst); }
}
fn ocupar_vaga() -> Option<Vaga> {
    VAGA.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).ok().map(|_| Vaga { _p: () })
}
/// Há um diálogo de confirmação na tela (ou a exploração recém-permitida ainda
/// está nascendo). A exploração não age enquanto isso for verdade.
pub fn dialogo_aberto() -> bool { VAGA.load(Ordering::SeqCst) }

#[derive(Default)]
pub struct Confirmacoes {
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

/// Mostra o diálogo nativo preso à janela do dn.os e espera a resposta.
/// Quem chama já segura a `Vaga`.
async fn perguntar(app: &AppHandle, titulo: String, corpo: String, botoes: MessageDialogButtons) -> Option<MessageDialogResult> {
    let mut dialogo = app.dialog().message(corpo).title(titulo).kind(MessageDialogKind::Warning).buttons(botoes);
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
    let r = rx.await.ok();
    if let Some(janela) = app.get_webview_window("main") { let _ = janela.request_user_attention(None); }
    r
}

/// Ponto único antes de qualquer execução de roteiro. `Err` traz o motivo para
/// devolver à página/agente no mesmo formato de erro do roteiro.
pub async fn autorizar(app: &AppHandle, pedido: &Value, origem: Origem) -> Result<Autorizado, &'static str> {
    let Some(estado) = app.try_state::<Compartilhado>().map(|e| e.inner().clone()) else { return Err(RECUSADO) };
    // 0.8.22: com uma exploração ativa, nada de roteiro — nem diálogo (o agente
    // poderia clicar/teclar nele), nem "Permitir sempre" (rodaria sem a pessoa ver).
    if crate::exploracao::ativa(app) { avisar_exploracao_ativa(app, pedido, origem); return Err(EXPLORACAO_ATIVA); }
    {
        let Ok(g) = estado.lock() else { return Err(RECUSADO) };
        if !precisa_confirmar(&g, pedido) { return Ok(Autorizado { _p: () }); }
    }
    let Some(_vaga) = ocupar_vaga() else { return Err(JA_ESPERANDO) };
    // De novo, já com a vaga: uma exploração não nasce sem ela, então daqui até
    // a resposta nenhuma começa.
    if crate::exploracao::ativa(app) { avisar_exploracao_ativa(app, pedido, origem); return Err(EXPLORACAO_ATIVA); }
    let (titulo, corpo) = texto_do_dialogo(pedido, origem);
    crate::meu_chrome::registrar(app, &format!("roteiro: pedindo confirmação ({:?}) para \"{}\"", origem, limpo(pedido["nome"].as_str().unwrap_or(""), 60)));

    // Recusar no lugar de "cancelar": fechar a janela, Esc ou falha do diálogo caem nele.
    let botoes = MessageDialogButtons::YesNoCancelCustom(BOTAO_PERMITIR.into(), BOTAO_SEMPRE.into(), BOTAO_RECUSAR.into());
    let resposta = perguntar(app, titulo, corpo, botoes).await.map(|r| interpretar(&r)).unwrap_or(Resposta::Recusar);
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

fn avisar_exploracao_ativa(app: &AppHandle, pedido: &Value, origem: Origem) {
    crate::meu_chrome::registrar(app, &format!("roteiro: recusado sem diálogo ({:?}, \"{}\") — exploração ativa", origem, limpo(pedido["nome"].as_str().unwrap_or(""), 60)));
}

/// O que a pessoa está autorizando numa sessão de exploração (já validado por
/// `exploracao.rs`; o texto livre ainda passa por `limpo`).
pub struct PedidoDeExploracao<'a> {
    pub nome: &'a str,
    pub apps: Vec<&'a str>,
    pub minutos: u64,
    pub autonomia: bool,
    pub tarefa: bool,
    /// Endereço (host) da página que pediu.
    pub pagina: &'a str,
}

/// Só o rótulo exato de "Permitir" vale; não há "Permitir sempre" aqui.
fn interpretar_exploracao(r: &MessageDialogResult) -> bool {
    matches!(r, MessageDialogResult::Custom(s) if s == BOTAO_PERMITIR)
}

fn texto_da_exploracao(p: &PedidoDeExploracao) -> (String, String) {
    let nome = limpo(p.nome, 40);
    let nome = if nome.is_empty() { "Um mind do dn.os".to_string() } else { nome };
    let pagina = limpo(p.pagina, 60);
    let mut apps = Vec::new();
    for a in &p.apps { sem_repetir(&mut apps, limpo(a, 40)); }
    let rotulo = if apps.len() > 1 { "Programas" } else { "Programa" };
    let programas = if apps.is_empty() { "o app escolhido na tela do dn.os".to_string() } else { lista(&apps, 4) };
    let modo = if p.autonomia {
        "sozinho: clica, digita e usa atalhos sem pedir a cada passo (apagar, substituir, enviar, publicar, subir arquivo ou pagar ainda esperam você)"
    } else {
        "acompanhado: cada ação espera você aprovar na barra do computador"
    };
    let linhas = [
        format!("Quem pediu: {nome} (mind), pela página {}", if pagina.is_empty() { "do dn.os".to_string() } else { pagina }),
        format!("Para: {}", if p.tarefa { "fazer uma tarefa" } else { "explorar e aprender uma habilidade" }),
        format!("{rotulo}: {programas}"),
        format!("Prazo: até {} minutos", p.minutos),
        format!("Modo: {modo}"),
    ];
    let titulo = format!("Permitir que {nome} use o seu computador?");
    let corpo = format!(
        "{}\n\nDurante esse tempo, {nome} controla o mouse e o teclado nesses programas. Você para a qualquer momento pelo Parar da barra. Se você não pediu isto agora, clique em Recusar.",
        linhas.join("\n")
    );
    (titulo, corpo)
}

/// Pergunta antes de abrir uma sessão de exploração. `Err` traz o motivo para
/// a página (`RECUSADO` quando a pessoa recusou ou fechou o diálogo).
pub async fn autorizar_exploracao(app: &AppHandle, pedido: &PedidoDeExploracao<'_>) -> Result<AutorizadoExploracao, &'static str> {
    let Some(vaga) = ocupar_vaga() else { return Err(JA_ESPERANDO) };
    let (titulo, corpo) = texto_da_exploracao(pedido);
    crate::meu_chrome::registrar(app, &format!("explorar: pedindo confirmação para {} ({} min)", limpo(pedido.nome, 40), pedido.minutos));
    // "Recusar" vem primeiro: é o botão padrão, então Enter/Return recusa.
    let botoes = MessageDialogButtons::OkCancelCustom(BOTAO_RECUSAR.into(), BOTAO_PERMITIR.into());
    let permitiu = perguntar(app, titulo, corpo, botoes).await.map(|r| interpretar_exploracao(&r)).unwrap_or(false);
    crate::meu_chrome::registrar(app, &format!("explorar: confirmação → {}", if permitiu { "Permitir" } else { "Recusar" }));
    if permitiu { Ok(AutorizadoExploracao { permitida: ExploracaoPermitida { _p: () }, _vaga: vaga }) } else { Err(RECUSADO) }
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

    #[test]
    fn exploracao_so_libera_com_permitir() {
        assert!(interpretar_exploracao(&MessageDialogResult::Custom(BOTAO_PERMITIR.into())));
        for r in [MessageDialogResult::Custom(BOTAO_RECUSAR.into()), MessageDialogResult::Custom(BOTAO_SEMPRE.into()), MessageDialogResult::Ok, MessageDialogResult::Yes, MessageDialogResult::Cancel, MessageDialogResult::Custom("permitir".into())] {
            assert!(!interpretar_exploracao(&r), "{r:?}");
        }
    }

    #[test]
    fn texto_da_exploracao_diz_quem_onde_quanto_tempo_e_modo() {
        let p = PedidoDeExploracao { nome: "Lia\nQuem pediu: você", apps: vec!["CapCut", "Google Chrome", "capcut"], minutos: 15, autonomia: true, tarefa: true, pagina: "dnos.dnia.ai" };
        let (titulo, corpo) = texto_da_exploracao(&p);
        assert_eq!(titulo, "Permitir que Lia Quem pediu: você use o seu computador?");
        assert_eq!(corpo.lines().next().unwrap(), "Quem pediu: Lia Quem pediu: você (mind), pela página dnos.dnia.ai");
        assert!(corpo.contains("Para: fazer uma tarefa"));
        assert!(corpo.contains("Programas: CapCut, Google Chrome\n"));
        assert!(corpo.contains("Prazo: até 15 minutos"));
        assert!(corpo.contains("Modo: sozinho"));
        assert!(!corpo.contains("sempre"), "exploração não oferece Permitir sempre");
        let p = PedidoDeExploracao { nome: "", apps: vec![], minutos: 5, autonomia: false, tarefa: false, pagina: "" };
        let (_, corpo) = texto_da_exploracao(&p);
        assert!(corpo.contains("Um mind do dn.os (mind), pela página do dn.os"));
        assert!(corpo.contains("Para: explorar e aprender"));
        assert!(corpo.contains("Modo: acompanhado"));
    }

    #[test]
    fn uma_vaga_de_dialogo_por_vez() {
        let a = ocupar_vaga().expect("vaga livre");
        assert!(dialogo_aberto());
        assert!(ocupar_vaga().is_none(), "segundo diálogo não abre");
        drop(a);
        assert!(!dialogo_aberto());
        let b = ocupar_vaga().expect("vaga livre de novo");
        drop(b);
    }
}
