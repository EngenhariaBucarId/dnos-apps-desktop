//! Automação de Interface do Windows (UI Automation): o equivalente da
//! Acessibilidade do macOS. Lê a árvore de controles de uma janela, acha o
//! menu do app e diz se o que está sob o cursor ou com o foco é campo de senha.
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::Win32::Foundation::POINT;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationExpandCollapsePattern, IUIAutomationInvokePattern, IUIAutomationTreeWalker,
    IUIAutomationValuePattern, UIA_ExpandCollapsePatternId, UIA_InvokePatternId, UIA_ValuePatternId,
};

use super::janelas::hwnd_de;
use crate::logica::{self, Ret};

const MENU_BAR: i32 = 50010;
const MENU: i32 = 50009;
const MENU_ITEM: i32 = 50011;

pub struct Uia {
    a: IUIAutomation,
    andador: IUIAutomationTreeWalker,
}

fn nome(el: &IUIAutomationElement) -> String {
    unsafe { el.CurrentName().map(|b| b.to_string()).unwrap_or_default() }
}
fn tipo(el: &IUIAutomationElement) -> i32 {
    unsafe { el.CurrentControlType().map(|t| t.0).unwrap_or(0) }
}
fn senha(el: &IUIAutomationElement) -> bool {
    unsafe { el.CurrentIsPassword().map(|b| b.as_bool()).unwrap_or(false) }
}
fn fora_da_tela(el: &IUIAutomationElement) -> bool {
    unsafe { el.CurrentIsOffscreen().map(|b| b.as_bool()).unwrap_or(false) }
}
fn ativo(el: &IUIAutomationElement) -> bool {
    unsafe { el.CurrentIsEnabled().map(|b| b.as_bool()).unwrap_or(true) }
}
fn ret(el: &IUIAutomationElement) -> Option<Ret> {
    let r = unsafe { el.CurrentBoundingRectangle().ok()? };
    Some(Ret { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top })
}
fn valor(el: &IUIAutomationElement) -> String {
    unsafe {
        el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
            .ok()
            .and_then(|p| p.CurrentValue().ok())
            .map(|b| b.to_string())
            .unwrap_or_default()
    }
}
fn classe(el: &IUIAutomationElement) -> String {
    unsafe { el.CurrentClassName().map(|b| b.to_string()).unwrap_or_default() }
}
fn pid_de(el: &IUIAutomationElement) -> i32 {
    unsafe { el.CurrentProcessId().unwrap_or(0) }
}
fn ajuda(el: &IUIAutomationElement) -> String {
    unsafe { el.CurrentHelpText().map(|b| b.to_string()).unwrap_or_default() }
}

impl Uia {
    pub fn nova() -> Option<Uia> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let a: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
            let andador = a.ControlViewWalker().ok()?;
            Some(Uia { a, andador })
        }
    }

    fn filhos(&self, el: &IUIAutomationElement, maximo: usize) -> Vec<IUIAutomationElement> {
        let mut v = Vec::new();
        unsafe {
            let mut c = self.andador.GetFirstChildElement(el).ok();
            while let Some(x) = c {
                if v.len() >= maximo {
                    break;
                }
                c = self.andador.GetNextSiblingElement(&x).ok();
                v.push(x);
            }
        }
        v
    }

    fn da_janela(&self, hwnd: isize) -> Option<IUIAutomationElement> {
        unsafe { self.a.ElementFromHandle(hwnd_de(hwnd)).ok() }
    }

    /// Árvore de controles da janela, em largura: até 150 controles, 8 níveis,
    /// fila de 300 e o prazo `ate`. Campo de senha entra sem texto e faz `senha` virar verdadeiro.
    /// Devolve (controles, truncada, senha, achou_a_janela).
    pub fn arvore(&self, hwnd: isize, ate: Instant) -> (Vec<Value>, bool, bool, bool) {
        let Some(raiz) = self.da_janela(hwnd) else { return (vec![], false, false, false) };
        let mut fila: Vec<(IUIAutomationElement, Option<usize>, u32)> = vec![(raiz, None, 0)];
        let mut controles: Vec<Value> = Vec::new();
        let mut truncada = false;
        let mut achou_senha = false;
        let mut i = 0;
        while i < fila.len() {
            if controles.len() >= 150 || Instant::now() >= ate {
                truncada = true;
                break;
            }
            let (el, pai, nivel) = fila[i].clone();
            i += 1;
            if nivel > 0 && fora_da_tela(&el) {
                continue;
            }
            let protegido = senha(&el);
            achou_senha = achou_senha || protegido;
            let indice = controles.len();
            let mut c = json!({ "ref": indice, "papel": logica::papel_uia(tipo(&el)), "subpapel": "" });
            if let Some(p) = pai {
                c["pai"] = json!(p);
            }
            if let Some(r) = ret(&el) {
                c["ret"] = r.json();
            }
            if protegido {
                c["protegido"] = json!(true);
            } else {
                c["titulo"] = json!(logica::curto(&nome(&el), 240));
                c["descricao"] = json!(logica::curto(&ajuda(&el), 240));
                c["valor"] = json!(logica::curto(&valor(&el), 240));
            }
            controles.push(c);
            if protegido {
                continue;
            }
            let vagas = 300usize.saturating_sub(fila.len());
            let filhos = self.filhos(&el, vagas + 1);
            if nivel >= 8 {
                if !filhos.is_empty() {
                    truncada = true;
                }
                continue;
            }
            if filhos.len() > vagas {
                truncada = true;
            }
            for f in filhos.into_iter().take(vagas) {
                fila.push((f, Some(indice), nivel + 1));
            }
        }
        (controles, truncada, achou_senha, true)
    }

    /// O controle sob o ponto é campo de senha?
    pub fn senha_no_ponto(&self, x: i32, y: i32) -> bool {
        unsafe { self.a.ElementFromPoint(POINT { x, y }).map(|e| senha(&e)).unwrap_or(false) }
    }

    /// O controle com foco é campo de senha?
    pub fn foco_protegido(&self) -> bool {
        unsafe { self.a.GetFocusedElement().map(|e| senha(&e)).unwrap_or(false) }
    }

    fn achar_barra(&self, raiz: &IUIAutomationElement) -> Option<IUIAutomationElement> {
        let mut fila: Vec<(IUIAutomationElement, u32)> = vec![(raiz.clone(), 0)];
        let mut i = 0;
        while i < fila.len() && fila.len() < 900 {
            let (el, nivel) = fila[i].clone();
            i += 1;
            if tipo(&el) == MENU_BAR {
                return Some(el);
            }
            if nivel < 6 {
                for f in self.filhos(&el, 60) {
                    fila.push((f, nivel + 1));
                }
            }
        }
        None
    }

    fn itens_de_menu(&self, pai: &IUIAutomationElement) -> Vec<IUIAutomationElement> {
        let mut itens: Vec<IUIAutomationElement> = Vec::new();
        for f in self.filhos(pai, 80) {
            match tipo(&f) {
                MENU_ITEM => itens.push(f),
                // Submenu do Win32: o item tem um "Menu" filho com os itens dentro.
                MENU => itens.extend(self.filhos(&f, 80).into_iter().filter(|x| tipo(x) == MENU_ITEM)),
                _ => {}
            }
        }
        itens
    }

    /// Itens do submenu que acabou de abrir. No Win32 o pop-up (`#32768`) é uma janela
    /// própria: aparece como filho do item, como um "Menu" dentro da janela do app
    /// ou como janela de nível superior da mesma aplicação, conforme o app. Tenta os três.
    fn submenu_de(&self, item: &IUIAutomationElement, janela: &IUIAutomationElement, pid: i32) -> Vec<IUIAutomationElement> {
        let direto = self.itens_de_menu(item);
        if !direto.is_empty() {
            return direto;
        }
        // Dentro da janela do app: qualquer "Menu" ou pop-up que não seja a barra.
        let mut fila: Vec<(IUIAutomationElement, u32)> = vec![(janela.clone(), 0)];
        let mut i = 0;
        while i < fila.len() && fila.len() < 900 {
            let (el, nivel) = fila[i].clone();
            i += 1;
            if (tipo(&el) == MENU || classe(&el) == "#32768") && !fila.is_empty() {
                let v = self.itens_de_menu(&el);
                if !v.is_empty() {
                    return v;
                }
            }
            if nivel < 6 {
                for f in self.filhos(&el, 60) {
                    fila.push((f, nivel + 1));
                }
            }
        }
        // Janelas de nível superior da mesma aplicação (o pop-up mais recente vem primeiro).
        if let Ok(area) = unsafe { self.a.GetRootElement() } {
            for topo in self.filhos(&area, 300) {
                if pid_de(&topo) != pid {
                    continue;
                }
                if tipo(&topo) == MENU || classe(&topo) == "#32768" {
                    let v = self.itens_de_menu(&topo);
                    if !v.is_empty() {
                        return v;
                    }
                }
            }
        }
        Vec::new()
    }

    /// Diagnóstico (só para desenvolvimento e CI): abre o menu do topo `nome` e descreve o
    /// que o Windows expõe — a estrutura muda entre apps e versões do Windows.
    pub fn depurar_menu(&self, hwnd: isize, nome_menu: &str) -> Vec<String> {
        let mut out = Vec::new();
        let Some(raiz) = self.da_janela(hwnd) else { return vec!["sem janela".into()] };
        let Some(barra) = self.achar_barra(&raiz) else { return vec!["sem barra de menus".into()] };
        let itens = self.itens_de_menu(&barra);
        let alvo = logica::limpo(nome_menu);
        let Some(item) = itens.iter().find(|e| logica::limpo(&nome(e)) == alvo) else { return vec![format!("menu {nome_menu} não achado")] };
        out.push(format!("aberto={}", self.abrir(item)));
        sleep(Duration::from_millis(400));
        let linha = |n: u32, e: &IUIAutomationElement| format!("{}{} [{}] classe={} pid={} nome={:?}", "  ".repeat(n as usize), tipo(e), logica::papel_uia(tipo(e)), classe(e), pid_de(e), nome(e));
        out.push("— filhos do item:".into());
        for f in self.filhos(item, 20) {
            out.push(linha(1, &f));
        }
        out.push("— janela do app (Menu/MenuItem/#32768 até nível 6):".into());
        let mut fila: Vec<(IUIAutomationElement, u32)> = vec![(raiz.clone(), 0)];
        let mut i = 0;
        while i < fila.len() && fila.len() < 900 {
            let (el, nivel) = fila[i].clone();
            i += 1;
            let t = tipo(&el);
            if t == MENU || t == MENU_ITEM || classe(&el) == "#32768" {
                out.push(linha(nivel, &el));
            }
            if nivel < 6 {
                for f in self.filhos(&el, 60) {
                    fila.push((f, nivel + 1));
                }
            }
        }
        out.push("— topo da área de trabalho:".into());
        if let Ok(area) = unsafe { self.a.GetRootElement() } {
            for topo in self.filhos(&area, 60) {
                out.push(linha(1, &topo));
                if classe(&topo) == "#32768" || tipo(&topo) == MENU {
                    for f in self.filhos(&topo, 20) {
                        out.push(linha(2, &f));
                    }
                }
            }
        }
        super::entrada::escape();
        sleep(Duration::from_millis(80));
        super::entrada::escape();
        out
    }

    fn abrir(&self, item: &IUIAutomationElement) -> bool {
        unsafe {
            if let Ok(p) = item.GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(UIA_ExpandCollapsePatternId) {
                if p.Expand().is_ok() {
                    return true;
                }
            }
            if let Ok(p) = item.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId) {
                return p.Invoke().is_ok();
            }
        }
        false
    }

    /// Menu do app pelo nome (Arquivo › Exportar), sem coordenada. Só funciona
    /// quando o app expõe uma barra de menus padrão; apps com menu próprio
    /// (Electron, hambúrguer) devolvem `menu_indisponivel` e o agente usa atalho ou clique.
    /// `listar`: devolve os itens do nível seguinte sem acionar o último.
    /// Ok(nomes) traz a listagem quando `listar`; senão fica vazio.
    pub fn menu(&self, hwnd: isize, caminho: &[String], listar: bool) -> Result<Vec<String>, String> {
        let raiz = self.da_janela(hwnd).ok_or("menu_indisponivel")?;
        let barra = self.achar_barra(&raiz).ok_or("menu_indisponivel")?;
        let nomes = |v: &[IUIAutomationElement]| -> Vec<String> { v.iter().map(nome).filter(|s| !s.is_empty()).take(60).collect() };
        let mut itens = self.itens_de_menu(&barra);
        if itens.is_empty() {
            return Err("menu_indisponivel".into());
        }
        if caminho.is_empty() {
            return if listar { Ok(nomes(&itens)) } else { Err("menu_invalido".into()) };
        }
        let mut atual: Option<IUIAutomationElement> = None;
        let mut abriu = false;
        let pid = pid_de(&raiz);
        for (i, quer) in caminho.iter().enumerate() {
            let alvo = logica::limpo(quer);
            let achado = itens
                .iter()
                .find(|e| logica::limpo(&nome(e)) == alvo)
                .or_else(|| itens.iter().find(|e| !alvo.is_empty() && logica::limpo(&nome(e)).starts_with(&alvo)))
                .cloned();
            let Some(item) = achado else {
                let opcoes = nomes(&itens).join(" | ");
                if abriu {
                    super::entrada::escape();
                }
                return Err(format!("menu_nao_encontrado:{quer} · opções: {opcoes}"));
            };
            if logica::menu_bloqueado(&nome(&item)) {
                if abriu {
                    super::entrada::escape();
                }
                return Err("menu_bloqueado".into());
            }
            atual = Some(item.clone());
            let ultimo = i + 1 == caminho.len();
            if !ultimo || listar {
                if !self.abrir(&item) {
                    return Err("menu_nao_respondeu".into());
                }
                abriu = true;
                sleep(Duration::from_millis(300));
                itens = self.submenu_de(&item, &raiz, pid);
            }
        }
        if listar {
            let lista = nomes(&itens);
            super::entrada::escape();
            sleep(Duration::from_millis(80));
            super::entrada::escape();
            return Ok(lista);
        }
        let final_ = atual.ok_or("menu_invalido")?;
        if !ativo(&final_) {
            if abriu {
                super::entrada::escape();
            }
            return Err("menu_desativado".into());
        }
        if !self.abrir(&final_) {
            if abriu {
                super::entrada::escape();
            }
            return Err("menu_nao_respondeu".into());
        }
        Ok(vec![])
    }
}
