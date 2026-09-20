//! Janelas e processos: quem está aberto, qual está na frente, onde fica cada
//! janela e como trazer um app para a frente (ou abri-lo).
use std::collections::HashMap;
use std::ffi::c_void;
use std::time::{Duration, Instant};

use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{BOOL, CloseHandle, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, GetClassNameW, GetForegroundWindow, GetWindowLongPtrW, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId,
    IsIconic, IsWindowVisible, SetForegroundWindow, ShowWindow, GWL_EXSTYLE, SW_RESTORE, SW_SHOWNORMAL, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
};

use crate::logica::{self, Ret};

pub fn hwnd_de(n: isize) -> HWND {
    HWND(n as *mut c_void)
}
pub fn numero(h: HWND) -> isize {
    h.0 as isize
}

#[derive(Clone, Debug)]
pub struct Janela {
    pub hwnd: isize,
    pub pid: u32,
    pub titulo: String,
    pub r: Ret,
    pub exe: String,
    pub minimizada: bool,
}

unsafe extern "system" fn coletar(h: HWND, l: LPARAM) -> BOOL {
    let v = &mut *(l.0 as *mut Vec<HWND>);
    v.push(h);
    BOOL(1)
}

pub fn pid_da_janela(h: HWND) -> u32 {
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(h, Some(&mut pid));
    }
    pid
}

/// Nome do executável (minúsculo, sem pasta) de um processo.
pub fn exe_do_pid(pid: u32, cache: &mut HashMap<u32, String>) -> String {
    if let Some(e) = cache.get(&pid) {
        return e.clone();
    }
    let mut nome = String::new();
    unsafe {
        if let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            let mut buf = vec![0u16; 1024];
            let mut n = buf.len() as u32;
            if QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut n).is_ok() {
                let caminho = String::from_utf16_lossy(&buf[..n as usize]);
                nome = caminho.rsplit(['\\', '/']).next().unwrap_or("").to_lowercase();
            }
            let _ = CloseHandle(h);
        }
    }
    cache.insert(pid, nome.clone());
    nome
}

fn titulo_de(h: HWND) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { GetWindowTextW(h, &mut buf) };
    if n <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..n as usize])
}

fn classe_de(h: HWND) -> String {
    let mut buf = [0u16; 128];
    let n = unsafe { GetClassNameW(h, &mut buf) };
    if n <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..n as usize])
}

pub fn ret_da_janela(h: HWND) -> Option<Ret> {
    let mut r = RECT::default();
    unsafe { GetWindowRect(h, &mut r).ok()? };
    Some(Ret { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top })
}

fn escondida_pelo_sistema(h: HWND) -> bool {
    let mut c = 0u32;
    unsafe {
        let _ = DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut c as *mut u32 as *mut c_void, 4);
    }
    c != 0
}

/// Janelas de nível superior, da frente para trás (a ordem do sistema). Só as
/// que uma pessoa vê como janela de app: visíveis, não "encobertas" (outras
/// áreas de trabalho virtuais), sem ser barra de ferramentas ou shell.
pub fn listar(incluir_minimizadas: bool) -> Vec<Janela> {
    let mut hs: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(coletar), LPARAM(&mut hs as *mut Vec<HWND> as isize));
    }
    let mut cache = HashMap::new();
    let mut out = Vec::new();
    for h in hs {
        unsafe {
            if !IsWindowVisible(h).as_bool() || escondida_pelo_sistema(h) {
                continue;
            }
            let minimizada = IsIconic(h).as_bool();
            if minimizada && !incluir_minimizadas {
                continue;
            }
            let estilo = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
            if estilo & WS_EX_TOOLWINDOW.0 != 0 && estilo & WS_EX_APPWINDOW.0 == 0 {
                continue;
            }
        }
        let classe = classe_de(h);
        if ["Progman", "WorkerW", "Shell_TrayWnd", "Shell_SecondaryTrayWnd"].contains(&classe.as_str()) {
            continue;
        }
        let Some(r) = ret_da_janela(h) else { continue };
        if !minimizada_ok(&r, unsafe { IsIconic(h).as_bool() }) {
            continue;
        }
        let pid = pid_da_janela(h);
        let exe = exe_do_pid(pid, &mut cache);
        if exe.is_empty() {
            continue;
        }
        out.push(Janela { hwnd: numero(h), pid, titulo: titulo_de(h), r, exe, minimizada: unsafe { IsIconic(h).as_bool() } });
    }
    out
}

/// Janela minimizada tem retângulo fora da tela (-32000); não conta como "tamanho".
fn minimizada_ok(r: &Ret, minimizada: bool) -> bool {
    minimizada || (r.w > 1 && r.h > 1)
}

pub fn da_frente() -> Option<(HWND, u32)> {
    let h = unsafe { GetForegroundWindow() };
    if h.0.is_null() {
        return None;
    }
    Some((h, pid_da_janela(h)))
}

/// Executável (minúsculo) do app que está na frente.
pub fn exe_da_frente() -> Option<String> {
    let (_, pid) = da_frente()?;
    let mut c = HashMap::new();
    let e = exe_do_pid(pid, &mut c);
    if e.is_empty() {
        None
    } else {
        Some(e)
    }
}

/// Todos os pids de um executável (apps Chromium/Electron têm vários processos).
pub fn pids_do_exe(exe: &str) -> Vec<u32> {
    let alvo = exe.to_lowercase();
    let mut pids = Vec::new();
    for j in listar(true) {
        if j.exe == alvo && !pids.contains(&j.pid) {
            pids.push(j.pid);
        }
    }
    pids
}

/// Monitor onde a janela está, em pixels físicos.
pub fn monitor_da_janela(h: HWND) -> Option<Ret> {
    unsafe {
        let m = MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(m, &mut mi).as_bool() {
            return None;
        }
        let r = mi.rcMonitor;
        Some(Ret { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top })
    }
}

/// Traz a janela para a frente. O Windows recusa `SetForegroundWindow` de quem
/// não tem o foco; o truque é anexar a fila de entrada da janela da frente por
/// um instante, e, se ainda assim negar, um toque de Alt libera a troca.
pub fn ativar(h: HWND) {
    unsafe {
        if IsIconic(h).as_bool() {
            let _ = ShowWindow(h, SW_RESTORE);
        }
        let frente = GetForegroundWindow();
        let fio_frente = if frente.0.is_null() { 0 } else { GetWindowThreadProcessId(frente, None) };
        let meu = GetCurrentThreadId();
        let anexou = fio_frente != 0 && fio_frente != meu && AttachThreadInput(meu, fio_frente, true).as_bool();
        let _ = BringWindowToTop(h);
        let mut ok = SetForegroundWindow(h).as_bool();
        if anexou {
            let _ = AttachThreadInput(meu, fio_frente, false);
        }
        if !ok || GetForegroundWindow() != h {
            super::entrada::toque_de_alt();
            ok = SetForegroundWindow(h).as_bool();
        }
        let _ = ok;
    }
}

/// A melhor janela do app para trazer: a maior visível, senão a maior minimizada.
pub fn principal_do_exe(exe: &str) -> Option<Janela> {
    let alvo = exe.to_lowercase();
    let mut todas: Vec<Janela> = listar(true).into_iter().filter(|j| j.exe == alvo).collect();
    todas.sort_by_key(|j| (j.minimizada, -(j.r.w as i64 * j.r.h as i64)));
    todas.into_iter().next()
}

/// O app do executável está na frente?
pub fn app_na_frente(exe: &str) -> bool {
    exe_da_frente().map(|e| e == exe.to_lowercase()).unwrap_or(false)
}

/// Leva o app para a frente, abrindo-o se estiver fechado. Espera até `espera`.
pub fn ativar_app(exe: &str, espera: Duration) -> bool {
    if exe.is_empty() {
        return false;
    }
    if principal_do_exe(exe).is_none() {
        // Fechado: o Windows resolve o nome pelo PATH e pela chave "App Paths".
        let mut arq: Vec<u16> = exe.encode_utf16().collect();
        arq.push(0);
        unsafe {
            let r = ShellExecuteW(None, w!("open"), PCWSTR(arq.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
            if (r.0 as isize) <= 32 {
                return false;
            }
        }
    }
    let fim = Instant::now() + espera;
    let mut ultimo = Instant::now() - Duration::from_secs(10);
    while Instant::now() < fim {
        if app_na_frente(exe) {
            std::thread::sleep(Duration::from_millis(250));
            return true;
        }
        if ultimo.elapsed() > Duration::from_millis(1500) {
            if let Some(j) = principal_do_exe(exe) {
                ativar(hwnd_de(j.hwnd));
            }
            ultimo = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    false
}

/// Janela do app que está com o foco, se for grande; senão a grande mais à
/// frente; senão a primeira. (Mesma regra do olhar-mac: diálogo separado, com
/// foco, é onde a pessoa está agindo; painel flutuante pequeno não conta.)
pub fn escolher(app: &[Janela], foco: Option<isize>) -> Option<Janela> {
    if let Some(f) = foco {
        if let Some(j) = app.iter().find(|j| j.hwnd == f && j.r.grande()) {
            return Some(j.clone());
        }
    }
    if let Some(j) = app.iter().find(|j| j.r.grande()) {
        return Some(j.clone());
    }
    app.first().cloned()
}

#[allow(dead_code)]
pub fn titulo_da(h: HWND) -> String {
    titulo_de(h)
}

/// Título da janela da frente (para o resumo de "concluído").
pub fn titulo_da_frente() -> String {
    da_frente().map(|(h, _)| titulo_de(h)).unwrap_or_default()
}

/// A janela `hwnd` (ou a raiz de quem possui o ponto) pertence a um dos pids?
pub fn raiz_do_ponto_e_do_app(x: i32, y: i32, pids: &[u32]) -> bool {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, WindowFromPoint, GA_ROOT};
    unsafe {
        let h = WindowFromPoint(POINT { x, y });
        if h.0.is_null() {
            return false;
        }
        let raiz = GetAncestor(h, GA_ROOT);
        let alvo = if raiz.0.is_null() { h } else { raiz };
        pids.contains(&pid_da_janela(alvo))
    }
}

#[allow(dead_code)]
pub fn limpar_exe(e: &str) -> String {
    logica::nome_do_exe(e)
}
