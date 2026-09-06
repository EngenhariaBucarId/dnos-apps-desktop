//! Voz durante a gravação ("fale para anotar").
//!
//! Enquanto o Aprenda comigo grava, o microfone fica aberto. O áudio é
//! cortado em trechos pelo silêncio (fala de pelo menos 1 s, seguida de 0,8 s
//! de silêncio; teto de 15 s) e cada trecho vai para a página do dn.os como
//! WAV mono 16 kHz em base64 (`dnos://gravador/audio {wav_base64, hora}`).
//! A página transcreve pelo mesmo serviço do microfone do chat e devolve a
//! nota (`dnos://gravador/nota {texto, hora}`), que a gravação alinha pelo tempo.
//! Nada é guardado em disco; quando a gravação para, o microfone fecha.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde_json::json;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

use crate::meu_chrome;

pub struct Voz {
    parar: Option<std::sync::mpsc::Sender<()>>,
}
pub type Compartilhado = Arc<Mutex<Voz>>;

pub fn instalar(app: &AppHandle) {
    use tauri::Manager;
    app.manage::<Compartilhado>(Arc::new(Mutex::new(Voz { parar: None })));
}

fn agora_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Abre o microfone numa thread própria (o cpal não é async). Erros só vão para o diário.
pub fn ligar(app: &AppHandle) {
    use tauri::Manager;
    let Some(estado) = app.try_state::<Compartilhado>() else { return };
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    if let Ok(mut g) = estado.lock() { g.parar = Some(tx); }
    let h = app.clone();
    std::thread::spawn(move || {
        if let Err(e) = capturar(&h, rx) { meu_chrome::registrar(&h, &format!("voz: {e}")); }
    });
}

pub fn desligar(app: &AppHandle) {
    use tauri::Manager;
    if let Some(estado) = app.try_state::<Compartilhado>() {
        if let Ok(mut g) = estado.lock() {
            if let Some(tx) = g.parar.take() { let _ = tx.send(()); }
        }
    }
}

fn capturar(app: &AppHandle, parar: std::sync::mpsc::Receiver<()>) -> Result<(), String> {
    let host = cpal::default_host();
    let dev = host.default_input_device().ok_or("sem microfone padrão")?;
    let conf = dev.default_input_config().map_err(|e| format!("config do microfone: {e}"))?;
    let taxa = conf.sample_rate().0 as usize;
    let canais = conf.channels() as usize;
    meu_chrome::registrar(app, &format!("voz: microfone aberto ({} Hz, {} canais)", taxa, canais));

    // Acumulador de fala compartilhado entre o callback e o cortador.
    let acumulado: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let ac = acumulado.clone();
    let erro = |e| eprintln!("[voz] {e}");
    let stream = match conf.sample_format() {
        cpal::SampleFormat::F32 => dev.build_input_stream(&conf.into(), move |d: &[f32], _| { if let Ok(mut a) = ac.lock() { for f in d.chunks(canais) { a.push(f.iter().sum::<f32>() / canais as f32); } } }, erro, None),
        cpal::SampleFormat::I16 => dev.build_input_stream(&conf.into(), move |d: &[i16], _| { if let Ok(mut a) = ac.lock() { for f in d.chunks(canais) { a.push(f.iter().map(|x| *x as f32 / 32768.0).sum::<f32>() / canais as f32); } } }, erro, None),
        cpal::SampleFormat::U16 => dev.build_input_stream(&conf.into(), move |d: &[u16], _| { if let Ok(mut a) = ac.lock() { for f in d.chunks(canais) { a.push(f.iter().map(|x| (*x as f32 - 32768.0) / 32768.0).sum::<f32>() / canais as f32); } } }, erro, None),
        _ => return Err("formato de áudio não suportado".into()),
    }.map_err(|e| format!("abrindo o microfone: {e}"))?;
    stream.play().map_err(|e| format!("iniciando o microfone: {e}"))?;

    // Cortador: a cada 100 ms olha o que chegou e decide onde termina uma fala.
    let limiar = 0.009f32;                 // RMS acima disso = voz
    let janela = taxa / 10;                // 100 ms
    let mut fala: Vec<f32> = Vec::new();
    let mut em_fala = false;
    let mut silencio_ms = 0u64;
    let mut inicio_fala = 0u64;
    loop {
        if parar.try_recv().is_ok() { break; }
        std::thread::sleep(std::time::Duration::from_millis(100));
        let pedaco: Vec<f32> = { let mut a = acumulado.lock().map_err(|_| "trava")?; std::mem::take(&mut *a) };
        if pedaco.is_empty() { continue; }
        for bloco in pedaco.chunks(janela.max(1)) {
            let rms = (bloco.iter().map(|x| x * x).sum::<f32>() / bloco.len() as f32).sqrt();
            let voz = rms > limiar;
            if voz { if !em_fala { em_fala = true; inicio_fala = agora_ms(); } silencio_ms = 0; }
            else if em_fala { silencio_ms += 100; }
            if em_fala { fala.extend_from_slice(bloco); }
            let dur_ms = (fala.len() as u64 * 1000) / taxa as u64;
            if em_fala && ((silencio_ms >= 800 && dur_ms >= 1000) || dur_ms >= 15000) {
                if dur_ms >= 1000 { enviar(app, &fala, taxa, inicio_fala); }
                fala.clear(); em_fala = false; silencio_ms = 0;
            }
            if em_fala && silencio_ms >= 800 && dur_ms < 1000 { fala.clear(); em_fala = false; silencio_ms = 0; }
        }
    }
    drop(stream);
    meu_chrome::registrar(app, "voz: microfone fechado");
    Ok(())
}

/// Reamostra para 16 kHz mono, empacota em WAV e manda para a página transcrever.
fn enviar(app: &AppHandle, amostras: &[f32], taxa: usize, hora: u64) {
    let alvo = 16000usize;
    let passo = taxa as f32 / alvo as f32;
    let n = (amostras.len() as f32 / passo) as usize;
    let mut cur = std::io::Cursor::new(Vec::<u8>::new());
    {
        let spec = hound::WavSpec { channels: 1, sample_rate: alvo as u32, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = match hound::WavWriter::new(&mut cur, spec) { Ok(w) => w, Err(_) => return };
        for i in 0..n {
            let pos = i as f32 * passo;
            let a = amostras[(pos as usize).min(amostras.len() - 1)];
            let _ = w.write_sample((a.clamp(-1.0, 1.0) * 32767.0) as i16);
        }
        let _ = w.finalize();
    }
    let b64 = base64_simples(&cur.into_inner());
    meu_chrome::registrar(app, &format!("voz: trecho de {:.1} s enviado para transcrever ({} KB)", n as f32 / alvo as f32, b64.len() / 1024));
    let _ = app.emit("dnos://gravador/audio", json!({ "wav_base64": b64, "hora": hora, "segundos": n as f32 / alvo as f32 }));
}

fn base64_simples(d: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity((d.len() + 2) / 3 * 4);
    for c in d.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let v = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        s.push(T[(v >> 18) as usize & 63] as char);
        s.push(T[(v >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { T[(v >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { T[v as usize & 63] as char } else { '=' });
    }
    s
}
