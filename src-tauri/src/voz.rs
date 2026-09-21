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
use crate::voz_dsp::{self, Cortador, Evento, Perfil, Trecho, PERFIL_NOTA, PERFIL_REUNIAO};

/// O que muda entre "fale para anotar" (trechos curtos, uma nota cada) e
/// "reunião" (trechos longos, transcrição corrida): tamanho do corte, evento
/// de saída e se a escuta conta como atividade da execução na máquina.
#[derive(Clone, Copy)]
pub struct Ajustes {
    pub evento_audio: &'static str,
    /// Como o áudio é cortado em trechos (pausa, teto, piso de ruído): ver `voz_dsp`.
    pub perfil: Perfil,
    pub escuta_na_maquina: bool,
}
pub const NOTA: Ajustes = Ajustes { evento_audio: "dnos://gravador/audio", perfil: PERFIL_NOTA, escuta_na_maquina: true };
pub const REUNIAO: Ajustes = Ajustes { evento_audio: "dnos://reuniao/audio", perfil: PERFIL_REUNIAO, escuta_na_maquina: false };

pub struct Voz {
    parar: Option<std::sync::mpsc::Sender<()>>,
    fim: Option<std::sync::mpsc::Receiver<()>>,
}
pub type Compartilhado = Arc<Mutex<Voz>>;

pub fn instalar(app: &AppHandle) {
    use tauri::Manager;
    app.manage::<Compartilhado>(Arc::new(Mutex::new(Voz { parar: None, fim: None })));
}

fn agora_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Abre o microfone numa thread própria (o cpal não é async). Erros só vão para o diário.
pub fn ligar(app: &AppHandle, id: &str) { ligar_com(app, id, NOTA) }

/// Como `ligar`, com o perfil de corte escolhido (nota de voz ou reunião).
pub fn ligar_com(app: &AppHandle, id: &str, ajustes: Ajustes) {
    use tauri::Manager;
    let Some(estado) = app.try_state::<Compartilhado>() else { return };
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let (fim_tx, fim_rx) = std::sync::mpsc::channel();
    if let Ok(mut g) = estado.lock() { g.parar = Some(tx); g.fim = Some(fim_rx); }
    let id = id.to_string();
    let h = app.clone();
    std::thread::spawn(move || {
        if let Err(e) = capturar(&h, rx, &id, ajustes) { meu_chrome::registrar(&h, &format!("voz: {e}")); }
        let _ = h.emit(if ajustes.escuta_na_maquina { "dnos://gravador/audio-fim" } else { "dnos://reuniao/audio-fim" }, json!({ "id": id }));
        let _ = fim_tx.send(());
    });
}

/// Abre o microfone padrão e o SEGURA aberto até `pronto()` dizer que sim (ou
/// até `maximo`). Serve para o macOS mostrar a pergunta de permissão quando ela
/// ainda não foi feita: a pergunta fica presa ao pedido de áudio — soltar o
/// microfone antes da resposta fechava o diálogo sozinho (13/09, 0.7.8).
pub fn segurar_microfone_ate(pronto: &dyn Fn() -> bool, maximo: std::time::Duration) {
    fn ignorar(_: cpal::StreamError) {}
    let host = cpal::default_host();
    let Some(dev) = host.default_input_device() else { return };
    let Ok(conf) = dev.default_input_config() else { return };
    let stream = match conf.sample_format() {
        cpal::SampleFormat::F32 => dev.build_input_stream(&conf.into(), move |_d: &[f32], _| {}, ignorar, None),
        cpal::SampleFormat::I16 => dev.build_input_stream(&conf.into(), move |_d: &[i16], _| {}, ignorar, None),
        cpal::SampleFormat::U16 => dev.build_input_stream(&conf.into(), move |_d: &[u16], _| {}, ignorar, None),
        _ => return,
    };
    let Ok(s) = stream else { return };
    let _ = s.play();
    let fim = std::time::Instant::now() + maximo;
    while std::time::Instant::now() < fim {
        std::thread::sleep(std::time::Duration::from_millis(500));
        if pronto() { break; }
    }
    // Um instante a mais: o macOS grava a decisão antes de o cliente sumir.
    std::thread::sleep(std::time::Duration::from_millis(500));
    drop(s);
}

pub fn desligar(app: &AppHandle) {
    use tauri::Manager;
    if let Some(estado) = app.try_state::<Compartilhado>() {
        if let Ok(mut g) = estado.lock() {
            if let Some(tx) = g.parar.take() { let _ = tx.send(()); }
            if let Some(fim) = g.fim.take() { let _ = fim.recv_timeout(std::time::Duration::from_secs(2)); }
        }
    }
}

/// Dispositivos virtuais (compartilhamento de tela, reunião) entregam só zeros
/// e costumam virar "entrada padrão" sem a pessoa perceber — foi o caso do
/// IdeaShare em 07/09: gravação inteira sem uma nota.
fn dispositivo_virtual(nome: &str) -> bool {
    let n = nome.to_lowercase();
    ["ideashare", "zoom", "teams", "blackhole", "soundflower", "loopback", "virtual", "aggregate", "agregado", "obs "].iter().any(|v| n.contains(v))
}

/// Estado do microfone para a página: qual está em uso e se entrega som.
fn avisar_microfone(app: &AppHandle, nome: &str, ok: bool, motivo: &str) {
    let _ = app.emit("dnos://gravador/microfone", json!({ "nome": nome, "ok": ok, "motivo": motivo }));
}

enum Fim { Parou, SemSinal }

/// Tenta o microfone padrão e, se ele só entregar silêncio absoluto, o próximo
/// dispositivo real da lista. A página recebe qual ficou e se há sinal.
fn capturar(app: &AppHandle, parar: std::sync::mpsc::Receiver<()>, id: &str, ajustes: Ajustes) -> Result<(), String> {
    let host = cpal::default_host();
    let mut cands: Vec<cpal::Device> = Vec::new();
    if let Some(d) = host.default_input_device() { cands.push(d); }
    if let Ok(it) = host.input_devices() {
        for d in it {
            let n = d.name().unwrap_or_default();
            if dispositivo_virtual(&n) { continue; }
            if cands.iter().any(|c| c.name().ok().as_deref() == Some(n.as_str())) { continue; }
            cands.push(d);
        }
    }
    if cands.is_empty() { avisar_microfone(app, "", false, "sem microfone"); return Err("sem microfone".into()); }
    let total = cands.len();
    for (i, dev) in cands.into_iter().enumerate() {
        let nome = dev.name().unwrap_or_else(|_| "?".into());
        let pode_trocar = i + 1 < total;
        match capturar_com(app, &parar, dev, &nome, pode_trocar, id, ajustes) {
            Ok(Fim::Parou) => return Ok(()),
            Ok(Fim::SemSinal) => { meu_chrome::registrar(app, &format!("voz: {nome} só entregou silêncio absoluto; tentando o próximo microfone")); }
            Err(e) => { meu_chrome::registrar(app, &format!("voz: {nome}: {e}; tentando o próximo")); }
        }
        if parar.try_recv().is_ok() { return Ok(()); }
    }
    avisar_microfone(app, "", false, "nenhum microfone entregou áudio");
    Err("nenhum microfone entregou áudio".into())
}

fn capturar_com(app: &AppHandle, parar: &std::sync::mpsc::Receiver<()>, dev: cpal::Device, nome_dev: &str, pode_trocar: bool, id: &str, ajustes: Ajustes) -> Result<Fim, String> {
    let conf = dev.default_input_config().map_err(|e| format!("config do microfone: {e}"))?;
    let taxa = conf.sample_rate().0 as usize;
    let canais = conf.channels() as usize;
    meu_chrome::registrar(app, &format!("voz: microfone aberto: {nome_dev} ({} Hz, {} canais, {:?})", taxa, canais, conf.sample_format()));

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

    // O corte em trechos (piso de ruído, histerese, pré-roll) mora em `voz_dsp`, onde é testado
    // com sinais sintéticos. Aqui só se alimenta o cortador e se reage ao que ele decide.
    let mut cortador = Cortador::novo(taxa, ajustes.perfil);
    let mut inicio_fala = 0u64;
    let mut max_5s = 0f32;
    let mut janelas_5s = 0u32;
    let mut sem_audio_ms = 0u64;
    let mut ultimo_resumo = std::time::Instant::now();
    // Sinal de verdade: qualquer amostra diferente de zero. Só zeros por 3 s é
    // dispositivo mudo, virtual ou sem permissão — troca (ou avisa, se for o último).
    let inicio = std::time::Instant::now();
    let mut com_sinal = false;
    let mut avisou_mudo = false;
    let avisar = |falando: bool| {
        let _ = app.emit("dnos://gravador/ouvindo", json!({ "falando": falando }));
        crate::barra::mesclar(app, json!({ "ouvindo": falando }));
        // Acende o microfone da barra flutuante no modo que estiver no ar
        // (demonstração ou reunião); fora deles, não faz nada.
        crate::maquina::atualizar_escuta(app, falando);
    };
    loop {
        if parar.try_recv().is_ok() {
            drop(stream);
            // O que o microfone ainda tinha na mão entra antes de fechar a fala em curso.
            let restante = acumulado.lock().map(|mut a| std::mem::take(&mut *a)).unwrap_or_default();
            for e in cortador.empurrar(&restante) { tratar(app, e, taxa, &mut inicio_fala, id, ajustes, &avisar, &mut max_5s, &mut janelas_5s); }
            if let Some(t) = cortador.encerrar() { enviar(app, &t, taxa, inicio_fala, id, ajustes); }
            resumo(app, &cortador);
            avisar(false);
            meu_chrome::registrar(app, "voz: microfone fechado"); return Ok(Fim::Parou);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        if !com_sinal && !avisou_mudo && inicio.elapsed() >= std::time::Duration::from_secs(3) {
            if pode_trocar { drop(stream); return Ok(Fim::SemSinal); }
            avisou_mudo = true;
            meu_chrome::registrar(app, &format!("voz: {nome_dev} sem sinal há 3 s (permissão negada ou dispositivo mudo?)"));
            avisar_microfone(app, nome_dev, false, "sem sinal: em Ajustes › Privacidade e Segurança › Microfone o dn.os precisa estar ligado. Se já aparece ligado, desligue e ligue de novo e reabra o dn.os (cada versão é um app novo para o macOS). Depois confira a entrada em Som › Entrada.");
        }
        let pedaco: Vec<f32> = { let mut a = acumulado.lock().map_err(|_| "trava")?; std::mem::take(&mut *a) };
        if pedaco.is_empty() {
            sem_audio_ms += 100;
            if sem_audio_ms == 3000 { meu_chrome::registrar(app, "voz: 3 s sem NENHUMA amostra do microfone (permissão negada ou dispositivo mudo?)"); }
            continue;
        }
        sem_audio_ms = 0;
        if !com_sinal && pedaco.iter().any(|x| *x != 0.0) {
            com_sinal = true;
            meu_chrome::registrar(app, &format!("voz: {nome_dev} entregando áudio"));
            avisar_microfone(app, nome_dev, true, "");
        }
        for e in cortador.empurrar(&pedaco) { tratar(app, e, taxa, &mut inicio_fala, id, ajustes, &avisar, &mut max_5s, &mut janelas_5s); }
        if ultimo_resumo.elapsed() >= std::time::Duration::from_secs(300) { resumo(app, &cortador); ultimo_resumo = std::time::Instant::now(); }
    }
}

/// Reage a um evento do cortador: acende/apaga a barra, envia o trecho, escreve o nível no diário.
#[allow(clippy::too_many_arguments)]
fn tratar(app: &AppHandle, e: Evento, taxa: usize, inicio_fala: &mut u64, id: &str, ajustes: Ajustes, avisar: &dyn Fn(bool), max_5s: &mut f32, janelas_5s: &mut u32) {
    match e {
        Evento::Fala(true) => { *inicio_fala = agora_ms(); avisar(true); }
        Evento::Fala(false) => avisar(false),
        Evento::Trecho(t) => {
            // A hora é a de quando a pessoa COMEÇOU a falar, não a do pré-roll.
            enviar(app, &t, taxa, inicio_fala.saturating_sub(t.pre_ms), id, ajustes);
        }
        Evento::Janela { rms, piso, limiar } => {
            if rms > *max_5s { *max_5s = rms; }
            *janelas_5s += 1;
            if *janelas_5s >= 50 {
                meu_chrome::registrar(app, &format!("voz: nível máx {:.4} / piso {:.4} / limiar {:.4} nos últimos 5 s", max_5s, piso, limiar));
                *max_5s = 0.0; *janelas_5s = 0;
            }
        }
    }
}

/// Para o diário: quanto do que foi captado virou trecho e o tamanho médio deles — é o que
/// diz, sem guardar áudio nenhum, se o corte está bom (muito trecho curto = fala picotada).
fn resumo(app: &AppHandle, c: &Cortador) {
    let s = c.estatisticas();
    if s.trechos == 0 { return; }
    meu_chrome::registrar(app, &format!(
        "voz: resumo — {} trechos, média {:.1} s, fala em {:.0}% do tempo, piso {:.4}",
        s.trechos, s.trechos_ms as f32 / s.trechos as f32 / 1000.0,
        s.fala_ms as f32 * 100.0 / s.tempo_ms.max(1) as f32, c.piso(),
    ));
}

/// Passa-alta, reamostra para 16 kHz com filtro, empacota em WAV e manda para a página transcrever.
fn enviar(app: &AppHandle, trecho: &Trecho, taxa: usize, hora: u64, id: &str, ajustes: Ajustes) {
    let amostras = voz_dsp::preparar(&trecho.amostras, taxa);
    let alvo = voz_dsp::ALVO_HZ as u32;
    let mut cur = std::io::Cursor::new(Vec::<u8>::new());
    {
        let spec = hound::WavSpec { channels: 1, sample_rate: alvo, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = match hound::WavWriter::new(&mut cur, spec) { Ok(w) => w, Err(_) => return };
        for a in &amostras { let _ = w.write_sample(*a); }
        let _ = w.finalize();
    }
    let segundos = amostras.len() as f32 / alvo as f32;
    let b64 = base64_simples(&cur.into_inner());
    meu_chrome::registrar(app, &format!("voz: trecho de {:.1} s ({:.1} s de fala) enviado para transcrever ({} KB)", segundos, trecho.fala_ms as f32 / 1000.0, b64.len() / 1024));
    let _ = app.emit(ajustes.evento_audio, json!({ "id": id, "wav_base64": b64, "hora": hora, "segundos": segundos }));
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
