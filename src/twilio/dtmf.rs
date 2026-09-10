use std::f64::consts::PI;

/// DTMF tone frequency pairs (low_freq, high_freq) for each key.
fn dtmf_frequencies(key: char) -> Option<(f64, f64)> {
    match key {
        '1' => Some((697.0, 1209.0)),
        '2' => Some((697.0, 1336.0)),
        '3' => Some((697.0, 1477.0)),
        '4' => Some((770.0, 1209.0)),
        '5' => Some((770.0, 1336.0)),
        '6' => Some((770.0, 1477.0)),
        '7' => Some((852.0, 1209.0)),
        '8' => Some((852.0, 1336.0)),
        '9' => Some((852.0, 1477.0)),
        '0' => Some((941.0, 1336.0)),
        '*' => Some((941.0, 1209.0)),
        '#' => Some((941.0, 1477.0)),
        _ => None,
    }
}

/// Convert a linear 16-bit sample to μ-law encoding.
fn linear_to_ulaw(sample: i16) -> u8 {
    const BIAS: i16 = 0x84;
    const MAX: i16 = 0x7FFF;
    const CLIP: i16 = 32635;

    let sign = if sample < 0 { 0x80u8 } else { 0x00u8 };
    let mut s = if sample < 0 {
        (-sample).min(CLIP)
    } else {
        sample.min(CLIP)
    };

    s += BIAS;

    let exponent = match s {
        0..=0xFF => 0,
        0x100..=0x1FF => 1,
        0x200..=0x3FF => 2,
        0x400..=0x7FF => 3,
        0x800..=0xFFF => 4,
        0x1000..=0x1FFF => 5,
        0x2000..=0x3FFF => 6,
        _ => 7,
    };

    let mantissa = ((s >> (exponent + 3)) & 0x0F) as u8;
    let result = !(sign | ((exponent as u8) << 4) | mantissa);

    result
}

/// Generate a DTMF tone for a single key as μ-law audio at 8000 Hz.
/// Duration: 200ms tone + 100ms silence.
fn generate_tone(key: char) -> Option<Vec<u8>> {
    let (low, high) = dtmf_frequencies(key)?;
    let sample_rate = 8000.0;
    let tone_duration = 0.2; // 200ms
    let silence_duration = 0.1; // 100ms
    let tone_samples = (sample_rate * tone_duration) as usize;
    let silence_samples = (sample_rate * silence_duration) as usize;
    let amplitude = 8000.0; // ~25% of max to avoid clipping

    let mut audio = Vec::with_capacity(tone_samples + silence_samples);

    // Generate dual-tone
    for i in 0..tone_samples {
        let t = i as f64 / sample_rate;
        let sample =
            amplitude * (2.0 * PI * low * t).sin() + amplitude * (2.0 * PI * high * t).sin();
        audio.push(linear_to_ulaw(sample as i16));
    }

    // Silence gap
    let silence_byte = linear_to_ulaw(0);
    for _ in 0..silence_samples {
        audio.push(silence_byte);
    }

    Some(audio)
}

/// Generate DTMF audio for a sequence of keys (e.g., "123#").
pub fn generate_dtmf_sequence(keys: &str) -> Vec<u8> {
    let mut audio = Vec::new();
    for key in keys.chars() {
        if let Some(tone) = generate_tone(key) {
            audio.extend(tone);
        }
    }
    audio
}

/// Extract DTMF key presses from Claude's response.
/// Looks for patterns like [PRESS 1], [PRESS 123], [PRESS #].
/// Returns (cleaned_text, Vec<keys_to_press>).
pub fn extract_dtmf_commands(text: &str) -> (String, Vec<String>) {
    let mut cleaned = text.to_string();
    let mut commands = Vec::new();

    while let Some(start) = cleaned.find("[PRESS ") {
        if let Some(end) = cleaned[start..].find(']') {
            let full_match = &cleaned[start..start + end + 1];
            let keys = cleaned[start + 7..start + end].trim().to_string();
            commands.push(keys);
            cleaned = cleaned.replace(full_match, "");
        } else {
            break;
        }
    }

    let cleaned = cleaned.trim().to_string();
    (cleaned, commands)
}
