//! Tiny helpers for deriving secondary colours from a palette's base colours.
//!
//! Used by palette files when the upstream source doesn't directly specify a token's value
//! (e.g. `panel.background` derived from `editor.background` by darkening).

fn parse_hex(hex: &str) -> Option<(u8, u8, u8)> {
    let s = hex.strip_prefix('#')?;
    let (r, g, b) = match s.len() {
        3 => {
            let r = u8::from_str_radix(&s[0..1].repeat(2), 16).ok()?;
            let g = u8::from_str_radix(&s[1..2].repeat(2), 16).ok()?;
            let b = u8::from_str_radix(&s[2..3].repeat(2), 16).ok()?;
            (r, g, b)
        }
        6 | 8 => {
            let r = u8::from_str_radix(&s[0..2], 16).ok()?;
            let g = u8::from_str_radix(&s[2..4], 16).ok()?;
            let b = u8::from_str_radix(&s[4..6], 16).ok()?;
            (r, g, b)
        }
        _ => return None,
    };
    Some((r, g, b))
}

fn fmt_rgb(r: u8, g: u8, b: u8) -> String {
    format!("#{:02X}{:02X}{:02X}", r, g, b)
}

/// Multiply each channel toward 0 by `frac` (0.0 = no change, 1.0 = black).
pub fn darken(hex: &str, frac: f64) -> String {
    let (r, g, b) = parse_hex(hex).expect("valid hex input");
    let f = (1.0 - frac).clamp(0.0, 1.0);
    fmt_rgb(
        (r as f64 * f) as u8,
        (g as f64 * f) as u8,
        (b as f64 * f) as u8,
    )
}

/// Multiply each channel toward 255 by `frac` (0.0 = no change, 1.0 = white).
pub fn lighten(hex: &str, frac: f64) -> String {
    let (r, g, b) = parse_hex(hex).expect("valid hex input");
    let f = frac.clamp(0.0, 1.0);
    let lift = |c: u8| -> u8 { (c as f64 + (255.0 - c as f64) * f) as u8 };
    fmt_rgb(lift(r), lift(g), lift(b))
}

/// Append an alpha component as `#RRGGBBAA` (alpha is rounded to 0..255).
pub fn with_alpha_hex(hex: &str, alpha: f64) -> String {
    let (r, g, b) = parse_hex(hex).expect("valid hex input");
    let a = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02X}{:02X}{:02X}{:02X}", r, g, b, a)
}

/// Format an `rgba(R,G,B,α)` string from a hex base + alpha.
pub fn rgba(hex: &str, alpha: f64) -> String {
    let (r, g, b) = parse_hex(hex).expect("valid hex input");
    format!("rgba({},{},{},{})", r, g, b, alpha)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn darken_grey_halfway_toward_black() {
        assert_eq!(darken("#808080", 0.5), "#404040");
    }

    #[test]
    fn lighten_grey_halfway_toward_white() {
        // 0x80 + (0xFF-0x80)/2 = 0xBF
        assert_eq!(lighten("#808080", 0.5), "#BFBFBF");
    }

    #[test]
    fn darken_zero_is_passthrough() {
        assert_eq!(darken("#1F1F1F", 0.0), "#1F1F1F");
    }

    #[test]
    fn with_alpha_emits_rrggbbaa() {
        assert_eq!(with_alpha_hex("#000000", 0.5), "#00000080");
    }

    #[test]
    fn rgba_emits_rgba_form() {
        assert_eq!(rgba("#FFFFFF", 0.36), "rgba(255,255,255,0.36)");
    }

    #[test]
    fn three_digit_hex_input_works() {
        assert_eq!(darken("#fff", 0.5), "#7F7F7F");
    }
}
