//! A chamada nativa do Dock (macOS), sem nenhuma dependência do Tauri — assim dá para provar
//! sozinha, com um app de verdade, que o Dock troca de imagem (ver `icone.rs`).

use std::ffi::c_void;
type P = *mut c_void;
#[link(name = "objc", kind = "dylib")]
extern "C" {
    fn objc_getClass(nome: *const std::os::raw::c_char) -> P;
    fn sel_registerName(nome: *const std::os::raw::c_char) -> P;
    fn objc_msgSend();
}
#[link(name = "AppKit", kind = "framework")]
extern "C" {}

fn sel(nome: &'static [u8]) -> P { unsafe { sel_registerName(nome.as_ptr() as *const _) } }
fn classe(nome: &'static [u8]) -> P { unsafe { objc_getClass(nome.as_ptr() as *const _) } }

/// `Some(png)` põe a imagem no Dock; `None` devolve o ícone do pacote.
/// Só pode ser chamada na thread principal (o AppKit não perdoa outra).
pub unsafe fn definir(png: Option<&[u8]>) -> bool {
    let msg = objc_msgSend as *const c_void;
    let (app_cls, img_cls, data_cls) = (classe(b"NSApplication\0"), classe(b"NSImage\0"), classe(b"NSData\0"));
    if app_cls.is_null() || img_cls.is_null() || data_cls.is_null() { return false; }
    let sem_arg: extern "C" fn(P, P) -> P = std::mem::transmute(msg);
    let com_um: extern "C" fn(P, P, P) -> P = std::mem::transmute(msg);
    let app = sem_arg(app_cls, sel(b"sharedApplication\0"));
    if app.is_null() { return false; }
    let Some(bytes) = png else {
        com_um(app, sel(b"setApplicationIconImage:\0"), std::ptr::null_mut()); // nil = ícone do pacote
        return true;
    };
    let dados_de: extern "C" fn(P, P, *const u8, usize) -> P = std::mem::transmute(msg);
    let dados = dados_de(data_cls, sel(b"dataWithBytes:length:\0"), bytes.as_ptr(), bytes.len());
    if dados.is_null() { return false; }
    let imagem = com_um(sem_arg(img_cls, sel(b"alloc\0")), sel(b"initWithData:\0"), dados);
    if imagem.is_null() { return false; } // o AppKit não conseguiu ler a imagem
    com_um(app, sel(b"setApplicationIconImage:\0"), imagem);
    sem_arg(imagem, sel(b"release\0")); // o app segura a sua própria referência
    true
}

/// O que o Dock tem agora: `(largura, altura)` da imagem do app, para provar que trocou.
#[allow(dead_code)] // usada só na prova com um app de verdade
pub unsafe fn tamanho_atual() -> Option<(f64, f64)> {
    let msg = objc_msgSend as *const c_void;
    let sem_arg: extern "C" fn(P, P) -> P = std::mem::transmute(msg);
    let app = sem_arg(classe(b"NSApplication\0"), sel(b"sharedApplication\0"));
    let img = sem_arg(app, sel(b"applicationIconImage\0"));
    if img.is_null() { return None; }
    #[repr(C)] struct Tam { w: f64, h: f64 }
    // NSSize volta em registradores (dois double): a assinatura normal serve em arm64 e x86_64.
    let tamanho: extern "C" fn(P, P) -> Tam = std::mem::transmute(msg);
    let t = tamanho(img, sel(b"size\0"));
    Some((t.w, t.h))
}
