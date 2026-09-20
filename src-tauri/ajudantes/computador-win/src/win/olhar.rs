//! Explorar: observar um app (foto da tela inteira + árvore de controles) e agir
//! nele (clique, arrasto, rolagem, texto, tecla, menu). Contrato idêntico ao
//! `olhar-mac`: cada chamada devolve UMA linha JSON.
use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use super::captura::{self, Imagem};
use super::entrada;
use super::janelas::{self, hwnd_de, Janela};
use super::uia::Uia;
use crate::logica::{self, Ret};

pub fn permissoes() -> Value {
    // O Windows não pede permissão de Acessibilidade nem de Gravação de Tela a
    // apps de área de trabalho. Só confere que a Automação de Interface abre.
    json!({ "acessibilidade": Uia::nova().is_some(), "tela": true, "sistema": "windows" })
}

/// Apps com janela visível, um por executável. Sem terminais, cofres e shell.
pub fn apps() -> Value {
    let mut por_exe: BTreeMap<String, String> = BTreeMap::new();
    for j in janelas::listar(false) {
        if j.titulo.trim().is_empty() || logica::exe_barrado(&j.exe) || logica::exe_de_sistema(&j.exe) {
            continue;
        }
        por_exe.entry(j.exe.clone()).or_insert_with(|| logica::nome_do_exe(&j.exe));
    }
    let mut v: Vec<(String, String)> = por_exe.into_iter().collect();
    v.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
    json!({ "apps": v.into_iter().map(|(bundle, nome)| json!({ "bundle": bundle, "nome": nome })).collect::<Vec<_>>() })
}

pub fn soltar() -> Value {
    entrada::soltar_tudo();
    json!({ "ok": true })
}

fn agora_iso() -> String {
    logica::iso8601(SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0))
}

fn q3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// Onde a janela está na imagem (0..1), para o agente saber onde cada janela do app aparece.
fn na_imagem(r: Ret, base: Ret) -> Value {
    let w = base.w.max(1) as f64;
    let h = base.h.max(1) as f64;
    json!({ "x": q3((r.x - base.x) as f64 / w), "y": q3((r.y - base.y) as f64 / h), "w": q3(r.w as f64 / w), "h": q3(r.h as f64 / h) })
}

/// Pixels da janela dentro da imagem da tela, para a impressão. Vazio se estiver fora.
fn recorte(img: &Imagem, tela: Ret, jan: Ret) -> Vec<u8> {
    let x0 = (jan.x - tela.x).clamp(0, img.w as i32) as usize;
    let y0 = (jan.y - tela.y).clamp(0, img.h as i32) as usize;
    let x1 = (jan.x + jan.w - tela.x).clamp(0, img.w as i32) as usize;
    let y1 = (jan.y + jan.h - tela.y).clamp(0, img.h as i32) as usize;
    let mut out = Vec::new();
    for y in y0..y1 {
        let ini = (y * img.w as usize + x0) * 3;
        let fim = (y * img.w as usize + x1) * 3;
        out.extend_from_slice(&img.rgb[ini..fim]);
    }
    out
}

fn recusa(motivo: &str) -> Value {
    json!({ "ok": false, "motivo": motivo })
}

/// `exe`: executável do app (o "bundle" do Windows). `pedido`: a ação a executar
/// antes de observar (o JSON que a casca manda em `--acao`).
pub fn olhar(exe: &str, pedido: Option<Value>) -> Value {
    let inicio = Instant::now();
    let exe_l = exe.to_lowercase();
    if logica::exe_barrado(&exe_l) {
        return recusa("aplicativo_nao_permitido");
    }
    let mut pids = janelas::pids_do_exe(&exe_l);
    if pids.is_empty() {
        return recusa("app_nao_aberto");
    }
    // A autorização de sessão permite trazer SOMENTE o app escolhido para a frente.
    let Some(principal) = janelas::principal_do_exe(&exe_l) else { return recusa("app_nao_aberto") };
    janelas::ativar(hwnd_de(principal.hwnd));
    let mut na_frente = false;
    for _ in 0..10 {
        if janelas::app_na_frente(&exe_l) {
            na_frente = true;
            break;
        }
        entrada::dormir(200);
    }
    if !na_frente {
        return recusa("app_nao_esta_na_frente");
    }
    pids = janelas::pids_do_exe(&exe_l);

    let uia = Uia::nova();
    let mut saida = json!({
        "versao": 1, "capturado_em": agora_iso(), "sistema": "windows",
        "app": { "nome": logica::nome_do_exe(&exe_l), "bundle": exe_l, "pid": principal.pid },
        "permissoes": { "acessibilidade": uia.is_some(), "tela": true },
        "somente_leitura": true,
    });

    let app: Vec<Janela> = janelas::listar(false).into_iter().filter(|j| pids.contains(&j.pid) && j.r.w > 1 && j.r.h > 1).collect();
    let foco = janelas::da_frente().map(|(h, _)| janelas::numero(h));
    let Some(janela) = janelas::escolher(&app, foco) else {
        saida["ok"] = json!(false);
        saida["motivo"] = json!("sem_janela_visivel");
        return saida;
    };
    saida["janela"] = json!({ "numero": janela.hwnd, "titulo": janela.titulo, "ret": janela.r.json() });
    // A imagem é a TELA INTEIRA (o monitor onde a janela lida está), como uma pessoa vê.
    let captura_r = janelas::monitor_da_janela(hwnd_de(janela.hwnd)).unwrap_or(janela.r);
    saida["captura"] = json!({ "ret": captura_r.json(), "tela_inteira": true });
    saida["janelas_do_app"] = Value::Array(
        app.iter()
            .take(8)
            .map(|w| json!({ "titulo": w.titulo, "w": w.r.w, "h": w.r.h, "na_imagem": na_imagem(w.r, captura_r), "lida": w.hwnd == janela.hwnd }))
            .collect(),
    );

    let mut avisos: Vec<String> = Vec::new();
    let (controles, truncada, senha, achou) = match &uia {
        Some(u) => {
            let prazo = Instant::now() + if pedido.is_none() { Duration::from_secs(2) } else { Duration::from_millis(700) };
            let r = u.arvore(janela.hwnd, prazo);
            if !r.3 {
                avisos.push("janela_sem_arvore_correspondente".into());
            }
            r
        }
        None => {
            avisos.push("acessibilidade_nao_autorizada".into());
            (vec![], false, false, false)
        }
    };
    let estado = if uia.is_none() { "sem-permissao" } else if !achou { "indisponivel" } else if truncada { "parcial" } else { "ok" };
    saida["acessibilidade"] = json!({ "estado": estado, "truncada": truncada, "controles": controles });

    let mut impressao = String::new();
    let mut tem_foto = false;
    if senha {
        avisos.push("foto_omitida_campo_protegido".into());
    } else if let Some(img) = captura::capturar(captura_r) {
        impressao = Imagem::impressao_de(&recorte(&img, captura_r, janela.r));
        if let Some((dados, w, h)) = img.jpeg_base64(1600, 70) {
            saida["foto"] = json!({ "mime": "image/jpeg", "dados": dados, "largura": w, "altura": h, "ret": captura_r.json() });
            saida["impressao"] = json!(impressao);
            tem_foto = true;
        }
    }
    if !tem_foto && !senha {
        avisos.push("foto_indisponivel".into());
    }

    // Uma janela mudando durante a leitura não é evidência confiável para agir depois.
    let mudou = janelas::ret_da_janela(hwnd_de(janela.hwnd)).map(|r| !r.quase_igual(&janela.r)).unwrap_or(true);
    saida["consistente"] = json!(!mudou);
    if mudou {
        avisos.push("janela_mudou_durante_leitura".into());
        if let Some(o) = saida.as_object_mut() {
            o.remove("foto");
        }
        tem_foto = false;
    }
    let tem_controles = saida["acessibilidade"]["controles"].as_array().map(|c| !c.is_empty()).unwrap_or(false);
    saida["ok"] = json!(!mudou && (tem_foto || tem_controles));
    saida["avisos"] = json!(avisos);
    saida["duracao_ms"] = json!(inicio.elapsed().as_millis() as u64);

    let Some(pedido) = pedido else { return saida };
    agir(&pedido, &saida, &janela, captura_r, &pids, senha, mudou, &impressao, uia.as_ref())
}

#[allow(clippy::too_many_arguments)]
fn agir(pedido: &Value, saida: &Value, janela: &Janela, captura_r: Ret, pids: &[u32], senha: bool, mudou: bool, impressao: &str, uia: Option<&Uia>) -> Value {
    // Autônomo: a janela precisa ser a mesma e sem campo de senha, mas não pixel a
    // pixel (num editor de vídeo a tela muda sozinha). Acompanhado exige a impressão
    // idêntica à da observação aprovada.
    let autonomo = pedido["autonomo"].as_bool() == Some(true);
    let confere = pedido["impressao"].as_str().map(|i| i == impressao).unwrap_or(false);
    let mesma_janela = pedido["janela"].as_i64() == Some(janela.hwnd as i64);
    if mudou || senha || !mesma_janela || !(autonomo || confere) {
        return recusa("tela_mudou_observe_novamente");
    }
    let _ = saida;
    let tipo = pedido["tipo"].as_str().unwrap_or("");
    let base = pedido["captura"]
        .as_object()
        .and_then(|d| Some(Ret { x: d.get("x")?.as_f64()? as i32, y: d.get("y")?.as_f64()? as i32, w: d.get("w")?.as_f64()? as i32, h: d.get("h")?.as_f64()? as i32 }))
        .filter(|r| r.w > 0 && r.h > 0)
        .unwrap_or(captura_r);
    let ponto = |kx: &str, ky: &str| -> Option<(i32, i32)> { logica::ponto_na_area(base, pedido[kx].as_f64()?, pedido[ky].as_f64()?) };
    // O alvo tem de ser do app autorizado e não pode ser campo de senha.
    let conferir = |p: (i32, i32)| janelas::raiz_do_ponto_e_do_app(p.0, p.1, pids) && !uia.map(|u| u.senha_no_ponto(p.0, p.1)).unwrap_or(false);

    match tipo {
        "clique" | "passar" => {
            let Some(p) = ponto("x", "y").filter(|p| conferir(*p)) else { return recusa("alvo_fora_do_app_ou_protegido") };
            if tipo == "passar" {
                entrada::mover(p.0, p.1);
            } else {
                let direito = pedido["botao"].as_str() == Some("direito");
                let vezes = if pedido["cliques"].as_i64() == Some(2) { 2 } else { 1 };
                entrada::clicar(p.0, p.1, direito, vezes);
            }
        }
        "arrastar" => {
            let Some(a) = ponto("x", "y").filter(|p| conferir(*p)) else { return recusa("alvo_fora_do_app_ou_protegido") };
            let Some(b) = ponto("x2", "y2").filter(|p| conferir(*p)) else { return recusa("destino_fora_do_app") };
            entrada::arrastar(a, b);
        }
        "rolar" => {
            let Some(p) = ponto("x", "y").filter(|p| conferir(*p)) else { return recusa("rolagem_invalida") };
            let dy = pedido["dy"].as_i64().unwrap_or(0) as i32;
            let dx = pedido["dx"].as_i64().unwrap_or(0) as i32;
            if dy.abs() > 600 || dx.abs() > 600 || (dx == 0 && dy == 0) {
                return recusa("rolagem_invalida");
            }
            entrada::rolar(p.0, p.1, dy, dx);
        }
        "texto" | "tecla" => {
            // Sem foco (timeline, canvas) o atalho vai para o app, que já está na
            // frente; só não digita nem aperta tecla com campo de senha em foco.
            if uia.map(|u| u.foco_protegido()).unwrap_or(false) {
                return recusa("foco_protegido");
            }
            if tipo == "texto" {
                let valor = pedido["texto"].as_str().unwrap_or("");
                if valor.encode_utf16().count() > 1000 || valor.contains('\n') || valor.contains('\r') || valor.chars().any(|c| c.is_control()) {
                    return recusa("texto_invalido");
                }
                entrada::texto(valor);
            } else {
                let nome = pedido["tecla"].as_str().unwrap_or("").to_lowercase();
                let mods: Vec<String> = pedido["mods"].as_array().map(|m| m.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
                if logica::tecla_vk(&nome).is_none() && logica::tecla_de_pontuacao(&nome).is_none() {
                    return recusa("tecla_nao_permitida");
                }
                if logica::atalho_bloqueado(&mods, &nome) {
                    return recusa("atalho_bloqueado");
                }
                if entrada::tecla(&nome, &mods).is_err() {
                    return recusa("tecla_nao_permitida");
                }
            }
        }
        "menu" => {
            let listar = pedido["listar"].as_bool() == Some(true);
            let caminho: Vec<String> = pedido["caminho"].as_array().map(|c| c.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
            let limite = if listar { 0..=4 } else { 1..=4 };
            if !limite.contains(&caminho.len()) {
                return recusa("menu_invalido");
            }
            let Some(u) = uia else { return recusa("menu_indisponivel") };
            return match u.menu(janela.hwnd, &caminho, listar) {
                Ok(nomes) if listar => json!({ "ok": true, "executada": false, "menu": nomes }),
                Ok(_) => json!({ "ok": true, "executada": true }),
                Err(motivo) => recusa(&motivo),
            };
        }
        _ => return recusa("acao_nao_permitida"),
    }
    json!({ "ok": true, "executada": true })
}
