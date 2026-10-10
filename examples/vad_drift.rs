//! Does Silero's recurrent state drift over a long session? Feeds the same
//! quiet, reverberant word every 20 s into noise for N minutes and prints the
//! word's peak speech probability with the state carried over vs reset
//! before each word. Usage: vad_drift WORD.wav [minutes] [word_rms] [gain]
use ears::vad::{SileroVad, VadConfig};

fn read(path: &str) -> Vec<f32> {
    let b = std::fs::read(path).unwrap();
    let mut i = 12;
    while &b[i..i + 4] != b"data" {
        let n = u32::from_le_bytes(b[i + 4..i + 8].try_into().unwrap()) as usize;
        i += 8 + n;
    }
    b[i + 8..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| i16::from_le_bytes(*c) as f32 / 32768.0)
        .collect()
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let word = read(&args[1]);
    let minutes: usize = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(10);
    let word_rms: f32 = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(0.01);
    let gain: f32 = args.get(4).and_then(|v| v.parse().ok()).unwrap_or(4.0);
    // Distance: a 250 ms exponential reverb tail, then scale.
    let mut seed = 12345u64;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed as f64 / u64::MAX as f64) as f32 * 2.0 - 1.0
    };
    let tail: Vec<f32> = (0..4000)
        .map(|i| rnd() * (-(i as f32) / 1000.0).exp() * 0.08)
        .collect();
    let mut wet = vec![0.0f32; word.len() + tail.len()];
    for (i, &w) in word.iter().enumerate() {
        if w == 0.0 {
            continue;
        }
        wet[i] += w;
        for (j, &t) in tail.iter().enumerate() {
            wet[i + j] += w * t;
        }
    }
    let k = word_rms / rms(&wet[..word.len()]);
    wet.iter_mut().for_each(|v| *v *= k);

    let period = 16000 * 20;
    let total = 16000 * 60 * minutes;
    let mut audio: Vec<f32> = (0..total).map(|_| rnd() * 0.0012).collect();
    let mut starts = vec![];
    let mut at = period / 2;
    while at + wet.len() < total {
        for (i, &v) in wet.iter().enumerate() {
            audio[at + i] += v;
        }
        starts.push(at);
        at += period;
    }
    audio
        .iter_mut()
        .for_each(|v| *v = (*v * gain).clamp(-1.0, 1.0));

    let cfg = VadConfig::default();
    let mut carried = SileroVad::new(cfg.clone()).unwrap();
    println!(
        "word_rms={word_rms} gain={gain} noise_rms={:.4}",
        0.0012 / 3f32.sqrt()
    );
    let mut next = 0;
    let (mut hit_c, mut hit_r) = (0, 0);
    let mut peak_c = 0.0f32;
    for (f, frame) in audio.as_chunks::<512>().0.iter().enumerate() {
        let pos = f * 512;
        carried.process_frame(frame).unwrap();
        if next < starts.len() && pos >= starts[next] && pos < starts[next] + wet.len() {
            peak_c = peak_c.max(carried.last_probability());
        }
        if next < starts.len() && pos >= starts[next] + wet.len() {
            // Same word with a fresh state primed by 1 s of the noise before it.
            let mut fresh = SileroVad::new(cfg.clone()).unwrap();
            let from = starts[next] - 16000;
            let mut peak_r = 0.0f32;
            for (g, fr) in audio[from..starts[next] + wet.len()]
                .as_chunks::<512>()
                .0
                .iter()
                .enumerate()
            {
                fresh.process_frame(fr).unwrap();
                if from + g * 512 >= starts[next] {
                    peak_r = peak_r.max(fresh.last_probability());
                }
            }
            hit_c += (peak_c >= 0.5) as usize;
            hit_r += (peak_r >= 0.5) as usize;
            println!(
                "t={:>5.1}min carried={:.2} reset={:.2}",
                starts[next] as f32 / 16000.0 / 60.0,
                peak_c,
                peak_r
            );
            peak_c = 0.0;
            next += 1;
        }
    }
    println!(
        "over 0.5: carried {hit_c}/{n}  reset {hit_r}/{n}",
        n = starts.len()
    );
}
