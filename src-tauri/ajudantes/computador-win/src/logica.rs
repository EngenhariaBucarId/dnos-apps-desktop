//! Regras puras do ajudante: sem Windows, sem COM. Rodam (e são testadas) em
//! qualquer sistema. Tudo o que decide "pode ou não pode" mora aqui, para que
//! um erro de regra seja pego por `cargo test` e não por um clique errado.
#![allow(dead_code)]

/// Retângulo em pixels físicos de tela.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ret {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Ret {
    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({ "x": self.x, "y": self.y, "w": self.w, "h": self.h })
    }
    pub fn grande(&self) -> bool {
        self.w >= 300 && self.h >= 200
    }
    pub fn contem(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
    /// Mesma janela, com folga de 2 px (o sistema arredonda bordas).
    pub fn quase_igual(&self, o: &Ret) -> bool {
        (self.x - o.x).abs() < 2 && (self.y - o.y).abs() < 2 && (self.w - o.w).abs() < 2 && (self.h - o.h).abs() < 2
    }
    pub fn uniao(&self, o: &Ret) -> Ret {
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        let x2 = (self.x + self.w).max(o.x + o.w);
        let y2 = (self.y + self.h).max(o.y + o.h);
        Ret { x, y, w: x2 - x, h: y2 - y }
    }
}

/// Modificadores que o Windows entende. `cmd` (Mac) vira Ctrl: é a convenção dos
/// apps multiplataforma, e permite que um roteiro de atalhos aprendido no Mac valha aqui.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mod {
    Ctrl,
    Alt,
    Shift,
}

pub fn mods_efetivos(mods: &[String]) -> Vec<Mod> {
    let tem = |n: &str| mods.iter().any(|m| m.eq_ignore_ascii_case(n));
    let mut v = Vec::new();
    if tem("cmd") || tem("ctrl") {
        v.push(Mod::Ctrl);
    }
    if tem("alt") {
        v.push(Mod::Alt);
    }
    if tem("shift") {
        v.push(Mod::Shift);
    }
    v
}

/// Nome de tecla (o vocabulário do contrato) → (código virtual, tecla estendida).
/// Letras e dígitos também; pontuação fica para o layout do teclado (`VkKeyScanW`).
pub fn tecla_vk(nome: &str) -> Option<(u16, bool)> {
    let n = nome.to_lowercase();
    let v = match n.as_str() {
        "escape" | "esc" => (0x1B, false),
        "enter" | "return" => (0x0D, false),
        "tab" => (0x09, false),
        "espaco" | "space" => (0x20, false),
        "esquerda" | "left" => (0x25, true),
        "cima" | "up" => (0x26, true),
        "direita" | "right" => (0x27, true),
        "baixo" | "down" => (0x28, true),
        "backspace" => (0x08, false),
        "delete" => (0x2E, true),
        "home" => (0x24, true),
        "end" => (0x23, true),
        "pageup" | "pgup" => (0x21, true),
        "pagedown" | "pgdown" => (0x22, true),
        _ => {
            if let Some(f) = n.strip_prefix('f') {
                if let Ok(k) = f.parse::<u16>() {
                    if (1..=12).contains(&k) {
                        return Some((0x6F + k, false));
                    }
                }
            }
            let mut cs = n.chars();
            if let (Some(c), None) = (cs.next(), cs.next()) {
                if c.is_ascii_lowercase() {
                    return Some((c.to_ascii_uppercase() as u16, false));
                }
                if c.is_ascii_digit() {
                    return Some((c as u16, false));
                }
            }
            return None;
        }
    };
    Some(v)
}

/// Uma tecla de um só caractere que não é letra nem dígito (pontuação).
pub fn tecla_de_pontuacao(nome: &str) -> Option<char> {
    let mut cs = nome.chars();
    match (cs.next(), cs.next()) {
        (Some(c), None) if !c.is_ascii_alphanumeric() && "=-[];',./\\`".contains(c) => Some(c),
        _ => None,
    }
}

/// Atalhos que nunca são enviados: sair do app, trocar de app, abrir o menu do
/// sistema, encerrar tarefa. Vale para os nomes do Mac (cmd+q…) e do Windows.
pub fn atalho_bloqueado(mods: &[String], tecla: &str) -> bool {
    let t = tecla.to_lowercase();
    let tem = |n: &str| mods.iter().any(|m| m.eq_ignore_ascii_case(n));
    let cmd = tem("cmd");
    let ctrl = tem("ctrl");
    let alt = tem("alt");
    let shift = tem("shift");
    if cmd && (["q", "tab", "espaco", "space", "h"].contains(&t.as_str()) || (alt && ["escape", "esc"].contains(&t.as_str())) || (shift && t == "q")) {
        return true;
    }
    if alt && ["f4", "tab", "escape", "esc"].contains(&t.as_str()) {
        return true;
    }
    if ctrl && alt && t == "delete" {
        return true;
    }
    if ctrl && shift && ["escape", "esc"].contains(&t.as_str()) {
        return true;
    }
    false
}

/// Texto de item de menu sem reticências, acelerador (&), atalho ("\tCtrl+Z") e caixa.
pub fn limpo(s: &str) -> String {
    let sem_atalho = s.split('\t').next().unwrap_or("");
    sem_atalho.to_lowercase().replace('\u{2026}', "").replace("...", "").replace('&', "").trim().to_string()
}

/// Espaços colapsados e caixa baixa, para comparar títulos.
pub fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// Itens de menu que encerram o app ou a sessão: nunca são acionados.
pub fn menu_bloqueado(titulo: &str) -> bool {
    let t = limpo(titulo);
    ["exit", "sair", "quit", "encerrar", "desligar", "reiniciar", "shut down", "restart", "sign out", "log off", "force", "forçar"].iter().any(|p| t.starts_with(p))
}

/// Programas que a exploração nunca controla: terminais, cofres de senha,
/// Configurações do Windows, editor de registro. (O mesmo espírito do Mac:
/// qualquer app, menos terminal, senha e Ajustes.)
pub fn exe_barrado(exe: &str) -> bool {
    let e = exe.to_lowercase();
    const EXATOS: &[&str] = &[
        "cmd.exe", "powershell.exe", "powershell_ise.exe", "pwsh.exe", "wt.exe", "windowsterminal.exe", "conhost.exe", "mintty.exe",
        "wsl.exe", "bash.exe", "regedit.exe", "mmc.exe", "taskmgr.exe", "systemsettings.exe", "control.exe", "credwiz.exe", "wscript.exe", "cscript.exe",
        "keepass.exe", "keepassxc.exe", "bitwarden.exe", "1password.exe", "lastpass.exe", "dashlane.exe", "enpass.exe", "nordpass.exe",
    ];
    if EXATOS.contains(&e.as_str()) {
        return true;
    }
    ["dnos", "dn.os", "password", "senha", "keychain", "terminal"].iter().any(|p| e.contains(p))
}

/// Programas de sistema que aparecem com janela mas não são "apps" para explorar.
pub fn exe_de_sistema(exe: &str) -> bool {
    let e = exe.to_lowercase();
    ["applicationframehost.exe", "textinputhost.exe", "searchhost.exe", "startmenuexperiencehost.exe", "shellexperiencehost.exe", "lockapp.exe", "systemsettings.exe", "widgets.exe", "dwm.exe", "csrss.exe", "winlogon.exe"].contains(&e.as_str())
}

/// Nome legível a partir do executável: `capcut.exe` → `capcut`.
pub fn nome_do_exe(exe: &str) -> String {
    let e = exe.trim();
    match e.rfind('.') {
        Some(i) if e[i..].eq_ignore_ascii_case(".exe") => e[..i].to_string(),
        _ => e.to_string(),
    }
}

/// Ponto da tela a partir de x/y em 0..1 relativos a uma área.
pub fn ponto_na_area(area: Ret, x: f64, y: f64) -> Option<(i32, i32)> {
    if !(x.is_finite() && y.is_finite() && (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y)) || area.w <= 0 || area.h <= 0 {
        return None;
    }
    Some((area.x + (x * area.w as f64).round() as i32, area.y + (y * area.h as f64).round() as i32))
}

/// Reduz mantendo a proporção até o maior lado ser `max`.
pub fn escala_para(w: u32, h: u32, max: u32) -> (u32, u32) {
    let maior = w.max(h).max(1);
    if maior <= max {
        return (w.max(1), h.max(1));
    }
    let f = max as f64 / maior as f64;
    (((w as f64 * f).round() as u32).max(1), ((h as f64 * f).round() as u32).max(1))
}

/// Pixels de rolagem (convenção do Mac) → unidades da roda do Windows (120 = um "clique").
pub fn delta_da_roda(pixels: i32) -> i32 {
    (pixels as f64 * 1.2).round() as i32
}

/// Tipo de controle da Automação de Interface → nome curto para o agente.
pub fn papel_uia(id: i32) -> &'static str {
    match id {
        50000 => "Button",
        50001 => "Calendar",
        50002 => "CheckBox",
        50003 => "ComboBox",
        50004 => "Edit",
        50005 => "Hyperlink",
        50006 => "Image",
        50007 => "ListItem",
        50008 => "List",
        50009 => "Menu",
        50010 => "MenuBar",
        50011 => "MenuItem",
        50012 => "ProgressBar",
        50013 => "RadioButton",
        50014 => "ScrollBar",
        50015 => "Slider",
        50016 => "Spinner",
        50017 => "StatusBar",
        50018 => "Tab",
        50019 => "TabItem",
        50020 => "Text",
        50021 => "ToolBar",
        50022 => "ToolTip",
        50023 => "Tree",
        50024 => "TreeItem",
        50025 => "Custom",
        50026 => "Group",
        50027 => "Thumb",
        50028 => "DataGrid",
        50029 => "DataItem",
        50030 => "Document",
        50031 => "SplitButton",
        50032 => "Window",
        50033 => "Pane",
        50034 => "Header",
        50035 => "HeaderItem",
        50036 => "Table",
        50037 => "TitleBar",
        50038 => "Separator",
        _ => "Unknown",
    }
}

/// Data e hora UTC em ISO 8601 (`2026-09-20T18:30:05Z`), sem depender de crate de datas.
pub fn iso8601(segundos: u64) -> String {
    let dias = (segundos / 86_400) as i64;
    let resto = segundos % 86_400;
    // Algoritmo de "days from civil" (Howard Hinnant), invertido.
    let z = dias + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let ano = if m <= 2 { y + 1 } else { y };
    format!("{ano:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", resto / 3600, (resto % 3600) / 60, resto % 60)
}

/// Corta em `n` caracteres e tira controles.
pub fn curto(s: &str, n: usize) -> String {
    s.chars().filter(|c| !c.is_control()).take(n).collect()
}

#[cfg(test)]
mod testes {
    use super::*;

    fn m(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn cmd_vira_ctrl_sem_duplicar() {
        assert_eq!(mods_efetivos(&m(&["cmd"])), vec![Mod::Ctrl]);
        assert_eq!(mods_efetivos(&m(&["cmd", "ctrl", "shift"])), vec![Mod::Ctrl, Mod::Shift]);
        assert_eq!(mods_efetivos(&m(&["alt"])), vec![Mod::Alt]);
        assert!(mods_efetivos(&[]).is_empty());
    }

    #[test]
    fn teclas_nomeadas_e_letras() {
        assert_eq!(tecla_vk("escape"), Some((0x1B, false)));
        assert_eq!(tecla_vk("Esc"), Some((0x1B, false)));
        assert_eq!(tecla_vk("baixo"), Some((0x28, true)));
        assert_eq!(tecla_vk("b"), Some((0x42, false)));
        assert_eq!(tecla_vk("5"), Some((0x35, false)));
        assert_eq!(tecla_vk("f1"), Some((0x70, false)));
        assert_eq!(tecla_vk("f12"), Some((0x7B, false)));
        assert_eq!(tecla_vk("f13"), None);
        assert_eq!(tecla_vk("rm -rf"), None);
        assert_eq!(tecla_vk(","), None);
        assert_eq!(tecla_de_pontuacao(","), Some(','));
        assert_eq!(tecla_de_pontuacao("a"), None);
    }

    #[test]
    fn atalhos_perigosos_sao_bloqueados() {
        assert!(atalho_bloqueado(&m(&["cmd"]), "q"));
        assert!(atalho_bloqueado(&m(&["cmd"]), "tab"));
        assert!(atalho_bloqueado(&m(&["cmd", "shift"]), "q"));
        assert!(atalho_bloqueado(&m(&["alt"]), "f4"));
        assert!(atalho_bloqueado(&m(&["alt"]), "tab"));
        assert!(atalho_bloqueado(&m(&["ctrl", "alt"]), "delete"));
        assert!(atalho_bloqueado(&m(&["ctrl", "shift"]), "escape"));
        assert!(!atalho_bloqueado(&m(&["cmd"]), "b"));
        assert!(!atalho_bloqueado(&m(&["ctrl"]), "e"));
        assert!(!atalho_bloqueado(&m(&[]), "escape"));
    }

    #[test]
    fn menu_limpo_e_bloqueio() {
        assert_eq!(limpo("&Arquivo…"), "arquivo");
        assert_eq!(limpo("Exportar..."), "exportar");
        assert_eq!(limpo("Time/Date\tF5"), "time/date");
        assert_eq!(limpo("Undo\tCtrl+Z"), "undo");
        assert!(menu_bloqueado("E&xit"));
        assert!(menu_bloqueado("Sair do CapCut"));
        assert!(!menu_bloqueado("Exportar"));
        assert!(!menu_bloqueado("Fechar projeto"));
    }

    #[test]
    fn barra_terminais_e_cofres() {
        assert!(exe_barrado("cmd.exe"));
        assert!(exe_barrado("PowerShell.exe"));
        assert!(exe_barrado("WindowsTerminal.exe"));
        assert!(exe_barrado("KeePassXC.exe"));
        assert!(exe_barrado("dnos-desktop.exe"));
        assert!(!exe_barrado("CapCut.exe"));
        assert!(!exe_barrado("notepad.exe"));
        assert!(!exe_barrado("explorer.exe"));
    }

    #[test]
    fn nome_e_ponto() {
        assert_eq!(nome_do_exe("CapCut.exe"), "CapCut");
        assert_eq!(nome_do_exe("app"), "app");
        let a = Ret { x: 100, y: 50, w: 1000, h: 500 };
        assert_eq!(ponto_na_area(a, 0.5, 0.5), Some((600, 300)));
        assert_eq!(ponto_na_area(a, 0.0, 1.0), Some((100, 550)));
        assert_eq!(ponto_na_area(a, 1.5, 0.5), None);
        assert_eq!(ponto_na_area(a, f64::NAN, 0.5), None);
    }

    #[test]
    fn escala_e_roda() {
        assert_eq!(escala_para(3200, 1800, 1600), (1600, 900));
        assert_eq!(escala_para(800, 600, 1600), (800, 600));
        assert_eq!(delta_da_roda(300), 360);
        assert_eq!(delta_da_roda(-100), -120);
    }

    #[test]
    fn data_iso() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601(1_789_000_000), "2026-09-10T00:26:40Z");
        assert_eq!(iso8601(951_782_400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn retangulos() {
        let a = Ret { x: 0, y: 0, w: 400, h: 300 };
        assert!(a.grande());
        assert!(!Ret { x: 0, y: 0, w: 183, h: 88 }.grande());
        assert!(a.quase_igual(&Ret { x: 1, y: 0, w: 401, h: 300 }));
        assert!(!a.quase_igual(&Ret { x: 5, y: 0, w: 400, h: 300 }));
        assert_eq!(a.uniao(&Ret { x: 300, y: 200, w: 300, h: 300 }), Ret { x: 0, y: 0, w: 600, h: 500 });
        assert!(a.contem(399, 299) && !a.contem(400, 0));
    }
}
