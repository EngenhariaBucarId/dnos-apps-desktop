//! Processamento do áudio da voz, sem cpal e sem tauri (21/09/2026).
//!
//! Tudo o que decide a QUALIDADE do que chega ao transcritor mora aqui, separado
//! do microfone de verdade para poder ser provado com sinais sintéticos:
//!
//! - `Cortador`: onde termina uma fala e começa outra;
//! - `preparar`: filtro passa-alta, reamostragem para 16 kHz COM filtro e ganho.
//!
//! Por que existe: nas duas reuniões presenciais de 21/09 (Malu, 41 min; Lia,
//! 36 min) os agentes disseram que a transcrição veio ruidosa, com nomes,
//! números e frases inteiras ilegíveis. O diário mostrou o motivo — 70% dos
//! trechos com menos de 2 s, o "ruído de fundo" estimado em 0,04 (era a fala
//! dos outros na sala), um terço das janelas com som audível abaixo do limiar —
//! e o código mostrou dois defeitos de processamento: a reamostragem pegava
//! amostras soltas, sem filtro (os agudos do "s" e do "f" viravam chiado na
//! faixa da voz), e a fala nunca ganhava o começo da primeira sílaba de volta.

use std::collections::VecDeque;
use std::f32::consts::PI;

pub const ALVO_HZ: usize = 16_000;
/// Uma janela de análise: o mesmo passo de 100 ms que o cortador sempre usou.
const JANELA_MS: u64 = 100;

// ───────────────────────────── perfis de corte ─────────────────────────────

/// Como o piso de ruído é estimado.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Piso {
    /// O de sempre (nota de voz): média móvel dos blocos abaixo do limiar. Funciona com
    /// uma pessoa falando perto do microfone; numa sala cheia ele SOBE junto com o barulho
    /// e passa a tratar a fala mais distante como ruído.
    Exponencial,
    /// Percentil baixo dos últimos 30 s: o nível dos momentos mais quietos da sala.
    /// Fala baixa, mesmo contínua, não o empurra para cima.
    Percentil,
}

#[derive(Clone, Copy)]
pub struct Perfil {
    /// Pausa que fecha um trecho.
    pub silencio_ms: u64,
    /// Fala mínima (só os blocos com voz, sem a cauda de silêncio) para valer a pena transcrever.
    pub minimo_fala_ms: u64,
    /// Teto de um trecho; perto dele, o primeiro respiro já corta.
    pub maximo_ms: u64,
    /// Quanto de áudio ANTES da fala detectada volta para o trecho (o começo da primeira sílaba).
    pub pre_roll_ms: u64,
    pub piso: Piso,
    /// A fala começa acima de `piso * entrada_x`.
    pub entrada_x: f32,
    /// E só termina abaixo de `piso * saida_x` (histerese: o fim de uma palavra é mais baixo que o começo).
    pub saida_x: f32,
}

/// "Fale para anotar" do Aprenda comigo: uma pessoa perto do microfone, notas curtas.
/// Comportamento idêntico ao de antes desta mudança.
pub const PERFIL_NOTA: Perfil = Perfil { silencio_ms: 800, minimo_fala_ms: 200, maximo_ms: 15_000, pre_roll_ms: 0, piso: Piso::Exponencial, entrada_x: 3.0, saida_x: 3.0 };

/// Reunião presencial: várias pessoas, microfone na mesa. Trechos mais longos (o
/// transcritor precisa de contexto), pausa maior para fechar, fala baixa e distante conta.
pub const PERFIL_REUNIAO: Perfil = Perfil { silencio_ms: 1_400, minimo_fala_ms: 500, maximo_ms: 30_000, pre_roll_ms: 300, piso: Piso::Percentil, entrada_x: 2.2, saida_x: 1.5 };

// ───────────────────────────────── cortador ────────────────────────────────

pub struct Trecho {
    pub amostras: Vec<f32>,
    /// Quanto do trecho é fala de verdade (blocos com voz), sem pausas nem cauda.
    pub fala_ms: u64,
    /// Quanto dele é o pré-roll: a fala começou isto antes do começo do trecho.
    pub pre_ms: u64,
}

pub enum Evento {
    /// Alguém começou (true) ou parou (false) de falar — acende e apaga o microfone da barra.
    Fala(bool),
    Trecho(Trecho),
    /// Uma janela de 100 ms analisada: para o diário do nível de áudio.
    Janela { rms: f32, piso: f32, limiar: f32 },
}

#[derive(Default, Clone, Copy, Debug)]
pub struct Estatisticas {
    pub trechos: u32,
    pub fala_ms: u64,
    pub trechos_ms: u64,
    pub tempo_ms: u64,
}

const HISTORICO_JANELAS: usize = 300; // 30 s
const PERCENTIL: f32 = 0.15;
/// Antes disto (0,8 s) não se decide nada: o piso ainda não tem de onde tirar.
const CALIBRACAO_JANELAS: usize = 8;
const PISO_MINIMO: f32 = 0.004;

pub struct Cortador {
    perfil: Perfil,
    taxa: usize,
    janela: usize,
    pendente: Vec<f32>,
    pre: VecDeque<Vec<f32>>,
    fala: Vec<f32>,
    em_fala: bool,
    silencio_ms: u64,
    fala_ms: u64,
    pre_ms_do_trecho: u64,
    ruido_exp: f32,
    historico: VecDeque<f32>,
    piso: f32,
    estat: Estatisticas,
}

impl Cortador {
    pub fn novo(taxa: usize, perfil: Perfil) -> Self {
        Cortador {
            perfil, taxa, janela: (taxa / 10).max(1),
            pendente: Vec::new(), pre: VecDeque::new(), fala: Vec::new(),
            em_fala: false, silencio_ms: 0, fala_ms: 0, pre_ms_do_trecho: 0,
            ruido_exp: 0.003, historico: VecDeque::new(), piso: PISO_MINIMO,
            estat: Estatisticas::default(),
        }
    }

    pub fn piso(&self) -> f32 { self.piso }
    pub fn estatisticas(&self) -> Estatisticas { self.estat }

    /// Alimenta com o que o microfone entregou (qualquer tamanho). Devolve o que aconteceu.
    pub fn empurrar(&mut self, amostras: &[f32]) -> Vec<Evento> {
        let mut eventos = Vec::new();
        self.pendente.extend_from_slice(amostras);
        while self.pendente.len() >= self.janela {
            let w: Vec<f32> = self.pendente.drain(..self.janela).collect();
            self.analisar(&w, &mut eventos);
        }
        eventos
    }

    /// A gravação parou: devolve a fala que estava em curso, se valer a pena.
    pub fn encerrar(&mut self) -> Option<Trecho> {
        if !self.em_fala { return None; }
        self.pendente.clear();
        self.fechar()
    }

    fn analisar(&mut self, w: &[f32], eventos: &mut Vec<Evento>) {
        let rms = (w.iter().map(|x| x * x).sum::<f32>() / w.len() as f32).sqrt();
        self.estat.tempo_ms += JANELA_MS;

        let (entrada, saida) = match self.perfil.piso {
            Piso::Exponencial => {
                let l = (self.ruido_exp * self.perfil.entrada_x).max(PISO_MINIMO);
                self.piso = self.ruido_exp;
                (l, (self.ruido_exp * self.perfil.saida_x).max(PISO_MINIMO))
            }
            Piso::Percentil => {
                self.historico.push_back(rms);
                if self.historico.len() > HISTORICO_JANELAS { self.historico.pop_front(); }
                let mut v: Vec<f32> = self.historico.iter().copied().collect();
                v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                self.piso = v[((v.len() as f32 * PERCENTIL) as usize).min(v.len() - 1)].max(PISO_MINIMO / 2.0);
                ((self.piso * self.perfil.entrada_x).max(0.008), (self.piso * self.perfil.saida_x).max(0.005))
            }
        };
        eventos.push(Evento::Janela { rms, piso: self.piso, limiar: entrada });

        // Sala que já chega barulhenta não pode virar "fala contínua" nos primeiros segundos.
        let calibrando = self.perfil.piso == Piso::Percentil && self.historico.len() < CALIBRACAO_JANELAS;
        let voz = !calibrando && if self.em_fala { rms > saida } else { rms > entrada };
        if self.perfil.piso == Piso::Exponencial && !voz { self.ruido_exp = self.ruido_exp * 0.95 + rms * 0.05; }

        if voz {
            if !self.em_fala {
                self.em_fala = true;
                self.fala.clear();
                self.pre_ms_do_trecho = self.pre.len() as u64 * JANELA_MS;
                for p in self.pre.drain(..) { self.fala.extend(p); }
                self.silencio_ms = 0;
                self.fala_ms = 0;
                eventos.push(Evento::Fala(true));
            }
            self.silencio_ms = 0;
            self.fala_ms += JANELA_MS;
            self.fala.extend_from_slice(w);
        } else if self.em_fala {
            self.silencio_ms += JANELA_MS;
            self.fala.extend_from_slice(w);
        } else if self.perfil.pre_roll_ms > 0 {
            self.pre.push_back(w.to_vec());
            while self.pre.len() as u64 * JANELA_MS > self.perfil.pre_roll_ms { self.pre.pop_front(); }
        }

        if self.em_fala {
            let dur_ms = self.fala.len() as u64 * 1000 / self.taxa as u64;
            let pausa = self.silencio_ms >= self.perfil.silencio_ms;
            // Perto do teto, o primeiro respiro já corta — em vez de cortar no meio de uma palavra.
            let respiro_perto_do_teto = dur_ms >= self.perfil.maximo_ms * 3 / 4 && self.silencio_ms >= 300;
            if pausa || respiro_perto_do_teto || dur_ms >= self.perfil.maximo_ms {
                if let Some(t) = self.fechar() { eventos.push(Evento::Trecho(t)); }
                eventos.push(Evento::Fala(false));
            }
        }
    }

    fn fechar(&mut self) -> Option<Trecho> {
        let amostras = std::mem::take(&mut self.fala);
        let (fala_ms, pre_ms) = (self.fala_ms, self.pre_ms_do_trecho);
        self.em_fala = false;
        self.silencio_ms = 0;
        self.fala_ms = 0;
        self.pre_ms_do_trecho = 0;
        if fala_ms < self.perfil.minimo_fala_ms { return None; }
        self.estat.trechos += 1;
        self.estat.fala_ms += fala_ms;
        self.estat.trechos_ms += amostras.len() as u64 * 1000 / self.taxa as u64;
        Some(Trecho { amostras, fala_ms, pre_ms })
    }
}

// ─────────────────────────── preparo do áudio ───────────────────────────

/// O que vai para o transcritor: passa-alta, reamostra para 16 kHz com filtro, ganho.
pub fn preparar(amostras: &[f32], taxa: usize) -> Vec<i16> {
    let mut x = passa_alta(amostras, taxa, 80.0);
    x = reamostrar(&x, taxa, ALVO_HZ);
    normalizar(&mut x, 0.9, 6.0);
    x.iter().map(|a| (a.clamp(-1.0, 1.0) * 32767.0) as i16).collect()
}

/// Passa-alta de 2ª ordem (Butterworth): tira o estrondo de ar-condicionado, mesa e passos,
/// que não ajudam a entender ninguém e ocupam o "espaço" que o transcritor tem para ouvir.
pub fn passa_alta(x: &[f32], taxa: usize, corte_hz: f32) -> Vec<f32> {
    let w0 = 2.0 * PI * corte_hz / taxa as f32;
    let (cos_w, alfa) = (w0.cos(), w0.sin() / (2.0 * 0.7071));
    let a0 = 1.0 + alfa;
    let (b0, b1, b2) = ((1.0 + cos_w) / 2.0 / a0, -(1.0 + cos_w) / a0, (1.0 + cos_w) / 2.0 / a0);
    let (a1, a2) = (-2.0 * cos_w / a0, (1.0 - alfa) / a0);
    let (mut x1, mut x2, mut y1, mut y2) = (0f32, 0f32, 0f32, 0f32);
    x.iter().map(|&xn| {
        let y = b0 * xn + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
        x2 = x1; x1 = xn; y2 = y1; y1 = y;
        y
    }).collect()
}

/// Reamostra com filtro passa-baixa (sinc em janela de Blackman), corte em 45% da taxa de destino.
///
/// O que havia antes era pegar a amostra mais próxima de cada posição: sem filtro, tudo
/// acima de 8 kHz DOBRAVA para dentro da faixa da voz. O "s" e o "f" (que vivem entre 4
/// e 10 kHz) viravam chiado sobre a própria fala — justo o que separa "seis" de "três".
pub fn reamostrar(x: &[f32], de: usize, para: usize) -> Vec<f32> {
    if de == para || x.is_empty() { return x.to_vec(); }
    let r = de as f64 / para as f64;
    let n_saida = (x.len() as f64 / r).floor() as usize;
    let fc = 0.45 * de.min(para) as f64 / de as f64; // ciclos por amostra de entrada
    let meia = ((8.0 * r).ceil() as isize).max(8);
    let mut saida = Vec::with_capacity(n_saida);
    for i in 0..n_saida {
        let pos = i as f64 * r;
        let c = pos.floor() as isize;
        let (mut acc, mut soma) = (0.0f64, 0.0f64);
        for j in (c - meia + 1)..=(c + meia) {
            if j < 0 || j as usize >= x.len() { continue; }
            let d = j as f64 - pos;
            let t = 2.0 * fc * d;
            let sinc = if t.abs() < 1e-12 { 1.0 } else { (std::f64::consts::PI * t).sin() / (std::f64::consts::PI * t) };
            let u = d / meia as f64; // -1..1
            let janela = 0.42 + 0.5 * (std::f64::consts::PI * u).cos() + 0.08 * (2.0 * std::f64::consts::PI * u).cos();
            let h = 2.0 * fc * sinc * janela;
            acc += x[j as usize] as f64 * h;
            soma += h;
        }
        saida.push(if soma.abs() > 1e-12 { (acc / soma) as f32 } else { 0.0 });
    }
    saida
}

/// Sobe o volume de trecho baixo (fala distante) até o pico alvo, com teto de ganho.
/// Nunca abaixa, e silêncio quase total não vira ruído alto.
pub fn normalizar(x: &mut [f32], pico_alvo: f32, ganho_maximo: f32) {
    let pico = x.iter().fold(0f32, |m, a| m.max(a.abs()));
    if pico < 1e-4 { return; }
    let ganho = (pico_alvo / pico).clamp(1.0, ganho_maximo);
    if ganho > 1.0 { for a in x.iter_mut() { *a *= ganho; } }
}

// ───────────────────────────────── testes ─────────────────────────────────

#[cfg(test)]
mod testes {
    use super::*;

    const T44: usize = 44_100;

    fn seno(hz: f32, taxa: usize, seg: f32, amp: f32) -> Vec<f32> {
        (0..(taxa as f32 * seg) as usize).map(|i| amp * (2.0 * PI * hz * i as f32 / taxa as f32).sin()).collect()
    }
    /// Ruído uniforme determinístico, escalado para o rms pedido.
    fn ruido(taxa: usize, seg: f32, rms: f32, mut semente: u32) -> Vec<f32> {
        (0..(taxa as f32 * seg) as usize).map(|_| {
            semente = semente.wrapping_mul(1664525).wrapping_add(1013904223);
            ((semente >> 8) as f32 / 8_388_608.0 - 1.0) * rms * 3f32.sqrt()
        }).collect()
    }
    fn rms(x: &[f32]) -> f32 { (x.iter().map(|a| a * a).sum::<f32>() / x.len().max(1) as f32).sqrt() }
    /// Amplitude da componente em `hz` (Goertzel).
    fn amplitude(x: &[f32], taxa: usize, hz: f32) -> f32 {
        let k = 2.0 * (2.0 * PI * hz / taxa as f32).cos();
        let (mut s1, mut s2) = (0f32, 0f32);
        for &v in x { let s = v + k * s1 - s2; s2 = s1; s1 = s; }
        let p = s1 * s1 + s2 * s2 - k * s1 * s2;
        2.0 * p.max(0.0).sqrt() / x.len() as f32
    }
    /// Fala sintética: harmônicos de 150 Hz modulados a 4 Hz, com o rms pedido.
    fn fala(taxa: usize, seg: f32, alvo: f32) -> Vec<f32> {
        let n = (taxa as f32 * seg) as usize;
        let mut v: Vec<f32> = (0..n).map(|i| {
            let t = i as f32 / taxa as f32;
            let env = 0.6 + 0.4 * (2.0 * PI * 4.0 * t).sin();
            env * [1.0f32, 0.6, 0.4, 0.25].iter().enumerate().map(|(h, a)| a * (2.0 * PI * 150.0 * (h + 1) as f32 * t).sin()).sum::<f32>()
        }).collect();
        let atual = rms(&v).max(1e-9);
        for a in v.iter_mut() { *a *= alvo / atual; }
        v
    }
    /// Um trecho de tempo: silêncio ou fala, sempre sobre o ruído de fundo da sala.
    enum P { Sil(f32), Fala(f32, f32) }
    fn cenario(taxa: usize, fundo: f32, partes: &[P]) -> Vec<f32> {
        let mut saida = Vec::new();
        for (i, p) in partes.iter().enumerate() {
            let (seg, f) = match p { P::Sil(s) => (*s, None), P::Fala(s, r) => (*s, Some(*r)) };
            let mut bloco = ruido(taxa, seg, fundo, 7 + i as u32);
            if let Some(r) = f { for (a, b) in bloco.iter_mut().zip(fala(taxa, seg, r)) { *a += b; } }
            saida.extend(bloco);
        }
        saida
    }
    fn trechos(c: &mut Cortador, x: &[f32]) -> Vec<Trecho> {
        let mut t = Vec::new();
        for pedaco in x.chunks(4410 + 137) { // tamanho quebrado de propósito: o corte não pode depender do bloco
            for e in c.empurrar(pedaco) { if let Evento::Trecho(tr) = e { t.push(tr); } }
        }
        if let Some(tr) = c.encerrar() { t.push(tr); }
        t
    }

    // ── reamostragem ──
    /// O que o código fazia antes: a amostra mais próxima, sem filtro.
    fn decimar_como_antes(x: &[f32], taxa: usize) -> Vec<f32> {
        let passo = taxa as f32 / ALVO_HZ as f32;
        (0..(x.len() as f32 / passo) as usize).map(|i| x[((i as f32 * passo) as usize).min(x.len() - 1)]).collect()
    }

    #[test]
    fn agudo_acima_de_8k_nao_vira_chiado_na_faixa_da_voz() {
        // 12 kHz é o "s" mais agudo. Sem filtro ele dobra para 16k-12k = 4 kHz, com a amplitude cheia.
        let x = seno(12_000.0, T44, 1.0, 0.5);
        let antes = decimar_como_antes(&x, T44);
        let agora = reamostrar(&x, T44, ALVO_HZ);
        assert!(amplitude(&antes, ALVO_HZ, 4_000.0) > 0.3, "o defeito antigo precisa aparecer no teste: {}", amplitude(&antes, ALVO_HZ, 4_000.0));
        assert!(amplitude(&agora, ALVO_HZ, 4_000.0) < 0.03, "ainda dobra: {}", amplitude(&agora, ALVO_HZ, 4_000.0));
    }

    #[test]
    fn a_voz_passa_com_o_mesmo_volume() {
        for (taxa, hz) in [(T44, 1_000.0), (48_000, 1_000.0), (T44, 3_000.0), (48_000, 5_000.0)] {
            let y = reamostrar(&seno(hz, taxa, 1.0, 0.5), taxa, ALVO_HZ);
            let a = amplitude(&y[400..y.len() - 400], ALVO_HZ, hz);
            assert!((a - 0.5).abs() < 0.03, "{taxa} Hz, tom de {hz}: amplitude {a}");
        }
    }

    #[test]
    fn o_tamanho_sai_certo_e_16k_passa_direto() {
        let y = reamostrar(&seno(1_000.0, T44, 1.0, 0.5), T44, ALVO_HZ);
        assert!((y.len() as i64 - 16_000).abs() <= 1, "{}", y.len());
        let x = seno(1_000.0, 16_000, 0.5, 0.5);
        assert_eq!(reamostrar(&x, 16_000, ALVO_HZ), x);
        assert!(reamostrar(&[], T44, ALVO_HZ).is_empty());
    }

    // ── passa-alta e ganho ──
    #[test]
    fn passa_alta_tira_o_estrondo_e_preserva_a_voz() {
        let ar = passa_alta(&seno(30.0, T44, 2.0, 0.3), T44, 80.0);
        let voz = passa_alta(&seno(1_000.0, T44, 2.0, 0.3), T44, 80.0);
        assert!(amplitude(&ar[T44..], T44, 30.0) < 0.06, "estrondo de 30 Hz: {}", amplitude(&ar[T44..], T44, 30.0));
        assert!(amplitude(&voz[T44..], T44, 1_000.0) > 0.29);
    }

    #[test]
    fn ganho_sobe_o_baixinho_com_teto_e_nao_mexe_no_que_ja_esta_alto() {
        let mut baixo = seno(500.0, ALVO_HZ, 0.5, 0.2);
        normalizar(&mut baixo, 0.9, 6.0);
        assert!((baixo.iter().fold(0f32, |m, a| m.max(a.abs())) - 0.9).abs() < 0.02);

        let mut baixissimo = seno(500.0, ALVO_HZ, 0.5, 0.02);
        normalizar(&mut baixissimo, 0.9, 6.0);
        assert!((baixissimo.iter().fold(0f32, |m, a| m.max(a.abs())) - 0.12).abs() < 0.01, "teto de 6x");

        let mut alto = seno(500.0, ALVO_HZ, 0.5, 0.95);
        normalizar(&mut alto, 0.9, 6.0);
        assert!((alto.iter().fold(0f32, |m, a| m.max(a.abs())) - 0.95).abs() < 1e-4, "não abaixa");

        let mut mudo = vec![0.00001f32; 100];
        normalizar(&mut mudo, 0.9, 6.0);
        assert!(mudo.iter().all(|a| *a == 0.00001), "silêncio não vira ruído alto");
    }

    #[test]
    fn preparar_entrega_16k_em_16_bits() {
        let y = preparar(&seno(1_000.0, T44, 1.0, 0.1), T44);
        assert!((y.len() as i64 - 16_000).abs() <= 1);
        assert!(y.iter().map(|a| a.abs()).max().unwrap() > 20_000, "ganho aplicado");
    }

    // ── cortador ──
    #[test]
    fn pausa_curta_dentro_da_frase_nao_corta_na_reuniao_mas_corta_na_nota() {
        // Fundo quieto (0,002): o perfil da nota, que estima o ruído como sempre, só o acompanha numa sala assim.
        let x = cenario(T44, 0.002, &[P::Sil(3.0), P::Fala(2.0, 0.15), P::Sil(1.0), P::Fala(2.0, 0.15), P::Sil(3.0)]);
        let reuniao = trechos(&mut Cortador::novo(T44, PERFIL_REUNIAO), &x);
        let nota = trechos(&mut Cortador::novo(T44, PERFIL_NOTA), &x);
        assert_eq!(reuniao.len(), 1, "a reunião junta as duas partes: o transcritor precisa da frase inteira");
        assert_eq!(nota.len(), 2, "a nota continua cortando em 0,8 s, como sempre");
        assert!(reuniao[0].amostras.len() as f32 / T44 as f32 > 4.8);
    }

    #[test]
    fn o_trecho_ganha_de_volta_o_comeco_da_primeira_silaba() {
        let x = cenario(T44, 0.01, &[P::Sil(4.0), P::Fala(2.0, 0.15), P::Sil(3.0)]);
        let t = trechos(&mut Cortador::novo(T44, PERFIL_REUNIAO), &x);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].pre_ms, 300);
        // fala (2 s) + pré-roll (0,3 s) + pausa que fechou (1,4 s)
        let seg = t[0].amostras.len() as f32 / T44 as f32;
        assert!((seg - 3.7).abs() < 0.25, "{seg}");
        let nota = trechos(&mut Cortador::novo(T44, PERFIL_NOTA), &x);
        assert_eq!(nota[0].pre_ms, 0, "a nota não muda");
    }

    #[test]
    fn interjeicao_isolada_curta_demais_e_descartada_na_reuniao() {
        let curta = cenario(T44, 0.01, &[P::Sil(3.0), P::Fala(0.3, 0.2), P::Sil(3.0)]);
        assert!(trechos(&mut Cortador::novo(T44, PERFIL_REUNIAO), &curta).is_empty(), "0,3 s de fala sozinha vira alucinação no transcritor");
        let longa = cenario(T44, 0.01, &[P::Sil(3.0), P::Fala(1.2, 0.2), P::Sil(3.0)]);
        assert_eq!(trechos(&mut Cortador::novo(T44, PERFIL_REUNIAO), &longa).len(), 1);
    }

    #[test]
    fn sala_barulhenta_desde_o_primeiro_segundo_nao_vira_fala_continua() {
        let x = ruido(T44, 30.0, 0.03, 1);
        assert!(trechos(&mut Cortador::novo(T44, PERFIL_REUNIAO), &x).is_empty(), "só chiado, nenhum trecho");
    }

    #[test]
    fn fala_baixa_e_distante_nao_empurra_o_piso_para_cima() {
        // O caso do diário: sala em 0,03 e alguém mais longe falando em 0,07, 40% do tempo.
        let mut partes = Vec::new();
        for _ in 0..20 { partes.push(P::Sil(1.8)); partes.push(P::Fala(1.2, 0.07)); }
        let x = cenario(T44, 0.03, &partes);
        let mut c = Cortador::novo(T44, PERFIL_REUNIAO);
        let t = trechos(&mut c, &x);
        assert!(c.piso() < 0.04, "o piso subiu para {} — o barulho da própria fala virou 'ruído'", c.piso());
        assert!(t.len() >= 8, "só {} trechos de 20 falas: a fala distante foi tratada como ruído", t.len());
    }

    #[test]
    fn perto_do_teto_o_respiro_corta_em_vez_de_cortar_no_meio_da_palavra() {
        // 25 s falando, um respiro de 0,4 s, mais 15 s. Teto de 30 s: corta no respiro, não aos 30.
        let x = cenario(T44, 0.01, &[P::Sil(3.0), P::Fala(25.0, 0.15), P::Sil(0.4), P::Fala(15.0, 0.15), P::Sil(3.0)]);
        let t = trechos(&mut Cortador::novo(T44, PERFIL_REUNIAO), &x);
        assert_eq!(t.len(), 2);
        let primeiro = t[0].amostras.len() as f32 / T44 as f32;
        assert!(primeiro > 25.0 && primeiro < 26.5, "cortou aos {primeiro} s, e não no respiro (~25,7 s)");
    }

    #[test]
    fn fala_sem_pausa_nunca_passa_do_teto() {
        let x = cenario(T44, 0.01, &[P::Sil(3.0), P::Fala(70.0, 0.15), P::Sil(3.0)]);
        let t = trechos(&mut Cortador::novo(T44, PERFIL_REUNIAO), &x);
        assert!(t.len() >= 3);
        assert!(t.iter().all(|t| t.amostras.len() as f32 / T44 as f32 <= 30.6));
    }

    #[test]
    fn estatisticas_contam_o_que_de_fato_foi_fala() {
        let x = cenario(T44, 0.01, &[P::Sil(3.0), P::Fala(2.0, 0.15), P::Sil(3.0), P::Fala(2.0, 0.15), P::Sil(3.0)]);
        let mut c = Cortador::novo(T44, PERFIL_REUNIAO);
        let t = trechos(&mut c, &x);
        let e = c.estatisticas();
        assert_eq!(e.trechos as usize, t.len());
        assert_eq!(t.len(), 2);
        assert!((e.fala_ms as f32 / 1000.0 - 4.0).abs() < 0.5, "{}", e.fala_ms);
        assert!(e.trechos_ms > e.fala_ms, "o trecho inclui pré-roll e a pausa que o fechou");
        assert!(e.tempo_ms >= 13_000);
    }
}

#[cfg(test)]
mod custo {
    use super::*;
    /// Um trecho de 30 s a 48 kHz (o teto da reunião) precisa ser preparado bem antes de o próximo
    /// terminar, e sem travar o laço de captura por mais que uns décimos de segundo.
    #[test]
    fn preparar_um_trecho_de_30_s_e_rapido() {
        let x: Vec<f32> = (0..48_000 * 30).map(|i| 0.1 * (i as f32 * 0.05).sin()).collect();
        let t = std::time::Instant::now();
        let y = preparar(&x, 48_000);
        let gasto = t.elapsed();
        assert_eq!(y.len(), 16_000 * 30);
        assert!(gasto.as_millis() < 3_000, "preparar levou {gasto:?}");
        eprintln!("preparar 30 s @48k: {gasto:?}");
    }
}
