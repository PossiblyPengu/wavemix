//! Parse AutoEQ parametric-EQ profile text files.
//!
//! AutoEQ exports ParametricEQ.txt files with lines like:
//! ```text
//! Preamp: -6.8 dB
//! Filter 1: ON PK Fc 64 Hz Gain 5.1 dB Q 1.09
//! Filter 2: ON LS Fc 105 Hz Gain 3.2 dB Q 0.71
//! ```
//!
//! This module converts them to `ParamEqParams` (up to `BUS_EQ_BANDS` bands).

use super::params::{BandKind, BandParams, ParamEqParams, BUS_EQ_BANDS};

/// Parse an AutoEQ ParametricEQ text block. Returns `(preamp_db, eq_params)` on success.
pub fn parse(text: &str) -> Result<(f32, ParamEqParams), String> {
    let mut preamp = 0.0f32;
    let mut bands: Vec<BandParams> = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("Preamp:") {
            preamp = rest
                .split_whitespace()
                .next()
                .and_then(|s| s.parse::<f32>().ok())
                .ok_or_else(|| format!("Could not read preamp from: {line}"))?;
        } else if line.starts_with("Filter") {
            if let Some(band) = parse_filter(line) {
                bands.push(band);
            }
        }
    }

    if bands.is_empty() {
        return Err(
            "No filters found. Paste the contents of an AutoEQ ParametricEQ.txt file.".into(),
        );
    }

    let mut eq = ParamEqParams {
        enabled: true,
        ..Default::default()
    };
    for (i, b) in bands.into_iter().take(BUS_EQ_BANDS).enumerate() {
        eq.bands[i] = b;
    }
    Ok((preamp, eq))
}

/// Parse one filter line: `Filter N: ON PK Fc 64 Hz Gain 5.1 dB Q 1.09`.
fn parse_filter(line: &str) -> Option<BandParams> {
    // Everything after the colon.
    let rest = line.splitn(2, ':').nth(1)?.trim();
    let tokens: Vec<&str> = rest.split_whitespace().collect();
    let mut t = tokens.iter();

    // ON / OFF
    let on = t.next()?.to_uppercase() == "ON";

    // Filter type.
    let kind = match t.next()?.to_uppercase().as_str() {
        "PK" | "PEAK" => BandKind::Peak,
        "LS" | "LSC" | "LSQ" | "LOW_SHELF" => BandKind::LowShelf,
        "HS" | "HSC" | "HSQ" | "HIGH_SHELF" => BandKind::HighShelf,
        "HP" | "HPQ" | "HIGH_PASS" => BandKind::HighPass,
        "LP" | "LPQ" | "LOW_PASS" => BandKind::LowPass,
        _ => return None,
    };

    let mut freq = 1000.0f32;
    let mut gain_db = 0.0f32;
    let mut q = 1.0f32;

    let remaining: Vec<&str> = t.collect();
    let mut i = 0;
    while i < remaining.len() {
        match remaining[i].to_uppercase().as_str() {
            "FC" => {
                if let Some(v) = remaining.get(i + 1).and_then(|s| s.parse().ok()) {
                    freq = v;
                }
                i += 3; // skip value + "Hz"
            }
            "GAIN" => {
                if let Some(v) = remaining.get(i + 1).and_then(|s| s.parse().ok()) {
                    gain_db = v;
                }
                i += 3; // skip value + "dB"
            }
            "Q" => {
                if let Some(v) = remaining.get(i + 1).and_then(|s| s.parse().ok()) {
                    q = v;
                }
                i += 2;
            }
            _ => {
                i += 1;
            }
        }
    }

    Some(BandParams {
        on,
        kind,
        freq,
        gain_db,
        q,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "
Preamp: -6.8 dB
Filter 1: ON PK Fc 64 Hz Gain 5.1 dB Q 1.09
Filter 2: ON LS Fc 105 Hz Gain 3.2 dB Q 0.71
Filter 3: ON HS Fc 10000 Hz Gain -2.0 dB Q 0.71
Filter 4: ON HP Fc 20 Hz Gain 0.0 dB Q 0.71
Filter 5: ON LP Fc 20000 Hz Gain 0.0 dB Q 0.71
";

    #[test]
    fn parses_preamp_and_filters() {
        let (preamp, eq) = parse(SAMPLE).unwrap();
        assert!((preamp + 6.8).abs() < 0.01, "preamp {preamp}");
        assert!(eq.enabled);
        assert_eq!(eq.bands[0].kind, BandKind::Peak);
        assert!((eq.bands[0].freq - 64.0).abs() < 0.1);
        assert!((eq.bands[0].gain_db - 5.1).abs() < 0.01);
        assert!((eq.bands[0].q - 1.09).abs() < 0.001);
        assert_eq!(eq.bands[1].kind, BandKind::LowShelf);
        assert_eq!(eq.bands[2].kind, BandKind::HighShelf);
        assert_eq!(eq.bands[3].kind, BandKind::HighPass);
        assert_eq!(eq.bands[4].kind, BandKind::LowPass);
    }

    #[test]
    fn empty_text_returns_error() {
        assert!(parse("Preamp: 0 dB").is_err());
        assert!(parse("").is_err());
    }
}
