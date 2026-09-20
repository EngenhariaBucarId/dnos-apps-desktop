//! Captura de tela pelo GDI (`BitBlt` do desktop), como uma pessoa vê: a tela
//! inteira do monitor, com janelas, menus e listas suspensas. Devolve JPEG em
//! base64, reduzido a 1600 px no maior lado, e uma impressão (SHA-256) dos
//! pixels da janela lida.
use std::ffi::c_void;

use base64::Engine;
use image::{codecs::jpeg::JpegEncoder, imageops::FilterType, ExtendedColorType, RgbImage};
use sha2::{Digest, Sha256};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, CAPTUREBLT, DIB_RGB_COLORS, HGDIOBJ, RGBQUAD, SRCCOPY,
};

use crate::logica::{self, Ret};

pub struct Imagem {
    pub w: u32,
    pub h: u32,
    /// RGB, 3 bytes por pixel.
    pub rgb: Vec<u8>,
}

/// Copia uma área da tela (pixels físicos, coordenadas do desktop virtual).
pub fn capturar(r: Ret) -> Option<Imagem> {
    if r.w <= 0 || r.h <= 0 || (r.w as i64 * r.h as i64) > 64_000_000 {
        return None;
    }
    unsafe {
        let tela = GetDC(None);
        if tela.is_invalid() {
            return None;
        }
        let mem = CreateCompatibleDC(tela);
        let bmp = CreateCompatibleBitmap(tela, r.w, r.h);
        let antigo = SelectObject(mem, bmp);
        let copiou = BitBlt(mem, 0, 0, r.w, r.h, tela, r.x, r.y, SRCCOPY | CAPTUREBLT).is_ok();
        // O bitmap não pode estar selecionado num DC para o GetDIBits.
        SelectObject(mem, antigo);
        let mut bgra = vec![0u8; (r.w as usize) * (r.h as usize) * 4];
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: r.w,
                biHeight: -r.h, // de cima para baixo
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            bmiColors: [RGBQUAD::default(); 1],
        };
        let linhas = if copiou { GetDIBits(mem, bmp, 0, r.h as u32, Some(bgra.as_mut_ptr() as *mut c_void), &mut info, DIB_RGB_COLORS) } else { 0 };
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(mem);
        ReleaseDC(None, tela);
        if !copiou || linhas == 0 {
            return None;
        }
        let mut rgb = Vec::with_capacity((r.w as usize) * (r.h as usize) * 3);
        for px in bgra.chunks_exact(4) {
            rgb.extend_from_slice(&[px[2], px[1], px[0]]);
        }
        Some(Imagem { w: r.w as u32, h: r.h as u32, rgb })
    }
}

impl Imagem {
    /// SHA-256 de pixels: "a janela mudou?" sem guardar a imagem.
    pub fn impressao_de(bytes: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(bytes);
        h.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }

    /// JPEG em base64 reduzido a `maximo` px no maior lado. Devolve (base64, largura, altura).
    pub fn jpeg_base64(&self, maximo: u32, qualidade: u8) -> Option<(String, u32, u32)> {
        let (w, h) = logica::escala_para(self.w, self.h, maximo);
        let original = RgbImage::from_raw(self.w, self.h, self.rgb.clone())?;
        let reduzida = if (w, h) == (self.w, self.h) { original } else { image::imageops::resize(&original, w, h, FilterType::Triangle) };
        let mut saida = Vec::new();
        JpegEncoder::new_with_quality(&mut saida, qualidade).encode(reduzida.as_raw(), w, h, ExtendedColorType::Rgb8).ok()?;
        Some((base64::engine::general_purpose::STANDARD.encode(saida), w, h))
    }
}
