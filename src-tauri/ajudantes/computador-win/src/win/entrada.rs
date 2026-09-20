//! Mouse e teclado por `SendInput`: entrada de verdade, como a de uma pessoa.
//! Mesma cadência do Mac (o cursor chega antes; o botão desce e sobe com
//! intervalo), porque descer e subir no mesmo instante só realça o botão.
use std::thread::sleep;
use std::time::Duration;

use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, VkKeyScanW, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEINPUT, MOUSE_EVENT_FLAGS, VIRTUAL_KEY, VK_CONTROL, VK_ESCAPE, VK_MENU,
    VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SetCursorPos, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN};

use crate::logica::{self, Mod};

pub fn dormir(ms: u64) {
    sleep(Duration::from_millis(ms));
}

fn enviar(entradas: &[INPUT]) {
    unsafe {
        SendInput(entradas, std::mem::size_of::<INPUT>() as i32);
    }
}

fn mouse(dx: i32, dy: i32, dados: u32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: dados, dwFlags: flags, time: 0, dwExtraInfo: 0 } } }
}

fn teclado(vk: u16, estendida: bool, sobe: bool) -> INPUT {
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if estendida {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if sobe {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } } }
}

/// Move o cursor para um ponto do desktop virtual (pixels físicos).
pub fn mover(x: i32, y: i32) {
    unsafe {
        let _ = SetCursorPos(x, y);
        let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN).max(2);
        let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN).max(2);
        let nx = (((x - vx) as f64) * 65535.0 / (vw - 1) as f64).round() as i32;
        let ny = (((y - vy) as f64) * 65535.0 / (vh - 1) as f64).round() as i32;
        enviar(&[mouse(nx, ny, 0, MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK)]);
    }
}

pub fn botao(direito: bool, baixo: bool) {
    let f = match (direito, baixo) {
        (false, true) => MOUSEEVENTF_LEFTDOWN,
        (false, false) => MOUSEEVENTF_LEFTUP,
        (true, true) => MOUSEEVENTF_RIGHTDOWN,
        (true, false) => MOUSEEVENTF_RIGHTUP,
    };
    enviar(&[mouse(0, 0, 0, f)]);
}

/// Clique (ou duplo, ou botão direito) no ponto, com a cadência do Mac.
pub fn clicar(x: i32, y: i32, direito: bool, vezes: u32) {
    mover(x, y);
    dormir(60);
    for n in 1..=vezes {
        botao(direito, true);
        dormir(40);
        botao(direito, false);
        if vezes == 2 && n == 1 {
            dormir(90);
        }
    }
}

/// Arrasta em 20 passos, como uma mão.
pub fn arrastar(de: (i32, i32), para: (i32, i32)) {
    mover(de.0, de.1);
    dormir(60);
    botao(false, true);
    dormir(80);
    for i in 1..=20 {
        let f = i as f64 / 20.0;
        mover(de.0 + ((para.0 - de.0) as f64 * f).round() as i32, de.1 + ((para.1 - de.1) as f64 * f).round() as i32);
        dormir(15);
    }
    dormir(50);
    botao(false, false);
}

/// Rola: `dy` e `dx` em pixels (convenção do Mac); positivo = para cima / para a direita.
pub fn rolar(x: i32, y: i32, dy: i32, dx: i32) {
    mover(x, y);
    dormir(30);
    if dy != 0 {
        enviar(&[mouse(0, 0, logica::delta_da_roda(dy) as u32, MOUSEEVENTF_WHEEL)]);
    }
    if dx != 0 {
        enviar(&[mouse(0, 0, logica::delta_da_roda(dx) as u32, MOUSEEVENTF_HWHEEL)]);
    }
}

/// Digita texto Unicode (sem quebra de linha) sem depender do layout do teclado.
pub fn texto(s: &str) {
    for unidade in s.encode_utf16() {
        let mk = |sobe: bool| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0),
                    wScan: unidade,
                    dwFlags: if sobe { KEYEVENTF_UNICODE | KEYEVENTF_KEYUP } else { KEYEVENTF_UNICODE },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        enviar(&[mk(false), mk(true)]);
        dormir(4);
    }
}

fn vk_do_modificador(m: Mod) -> u16 {
    match m {
        Mod::Ctrl => VK_CONTROL.0,
        Mod::Alt => VK_MENU.0,
        Mod::Shift => VK_SHIFT.0,
    }
}

/// Aperta uma tecla pelo nome do contrato, com modificadores. `Err(motivo)` se a
/// tecla não existe. Não confere bloqueios: quem chama usa `logica::atalho_bloqueado`.
pub fn tecla(nome: &str, mods: &[String]) -> Result<(), &'static str> {
    let mut mods_vk: Vec<u16> = logica::mods_efetivos(mods).into_iter().map(vk_do_modificador).collect();
    let (vk, estendida) = if let Some(k) = logica::tecla_vk(nome) {
        k
    } else if let Some(c) = logica::tecla_de_pontuacao(nome) {
        // Pontuação depende do layout: o Windows diz qual tecla (e se precisa de Shift).
        let r = unsafe { VkKeyScanW(c as u16) };
        if r == -1 {
            return Err("tecla_nao_permitida");
        }
        let vk = (r & 0xFF) as u16;
        let estados = ((r >> 8) & 0xFF) as u8;
        if estados & 1 != 0 && !mods_vk.contains(&VK_SHIFT.0) {
            mods_vk.push(VK_SHIFT.0);
        }
        (vk, false)
    } else {
        return Err("tecla_nao_permitida");
    };
    for m in &mods_vk {
        enviar(&[teclado(*m, false, false)]);
    }
    if !mods_vk.is_empty() {
        dormir(20);
    }
    enviar(&[teclado(vk, estendida, false)]);
    dormir(30);
    enviar(&[teclado(vk, estendida, true)]);
    for m in mods_vk.iter().rev() {
        enviar(&[teclado(*m, false, true)]);
    }
    Ok(())
}

/// Um toque de Alt: libera o Windows para trocar o foco (ver `janelas::ativar`).
pub fn toque_de_alt() {
    enviar(&[teclado(VK_MENU.0, false, false), teclado(VK_MENU.0, false, true)]);
    dormir(30);
}

pub fn escape() {
    enviar(&[teclado(VK_ESCAPE.0, false, false), teclado(VK_ESCAPE.0, false, true)]);
}

/// Solta tudo: botão do mouse e modificadores que ficaram presos numa ação cancelada.
pub fn soltar_tudo() {
    botao(false, false);
    botao(true, false);
    let mut v = Vec::new();
    for vk in [VK_CONTROL.0, VK_MENU.0, VK_SHIFT.0, VK_ESCAPE.0, 0x0D, 0x09, 0x20, 0x25, 0x26, 0x27, 0x28, 0x08, 0x2E] {
        v.push(teclado(vk, false, true));
    }
    enviar(&v);
}
