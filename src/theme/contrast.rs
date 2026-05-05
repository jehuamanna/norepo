//! WCAG relative-luminance and contrast-ratio helpers.
//!
//! Used by the `HighContrastDark` palette test and as an informational survey across all
//! palettes. The contrast formula and sRGB linearisation follow
//! <https://www.w3.org/TR/WCAG21/#dfn-relative-luminance>.

/// Parse a hex string (`#RGB`, `#RRGGBB`, or `#RRGGBBAA`) into 8-bit (R, G, B) components,
/// dropping any alpha. Returns `None` for unrecognised input — including the `rgb()` /
/// `rgba()` forms which we don't need here.
fn parse_rgb(hex: &str) -> Option<(u8, u8, u8)> {
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

fn channel(c: u8) -> f64 {
    let v = c as f64 / 255.0;
    if v <= 0.03928 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// Relative luminance per WCAG. Returns `None` for unparseable hex input.
pub fn relative_luminance(hex: &str) -> Option<f64> {
    let (r, g, b) = parse_rgb(hex)?;
    Some(0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b))
}

/// Contrast ratio per WCAG: `(L_brighter + 0.05) / (L_darker + 0.05)`. `None` if either
/// input is unparseable.
pub fn contrast_ratio(a: &str, b: &str) -> Option<f64> {
    let la = relative_luminance(a)?;
    let lb = relative_luminance(b)?;
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    Some((hi + 0.05) / (lo + 0.05))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() < tol
    }

    #[test]
    fn black_white_max_ratio() {
        let r = contrast_ratio("#000000", "#FFFFFF").unwrap();
        assert!(approx(r, 21.0, 0.01), "{r}");
    }

    #[test]
    fn mid_grey_white_known_ratio() {
        // mid-grey #777777 vs white ≈ 4.48
        let r = contrast_ratio("#777777", "#FFFFFF").unwrap();
        assert!(approx(r, 4.48, 0.05), "{r}");
    }

    #[test]
    fn three_digit_hex_parses() {
        let r = contrast_ratio("#000", "#fff").unwrap();
        assert!(approx(r, 21.0, 0.01), "{r}");
    }

    #[test]
    fn order_independent() {
        let a = contrast_ratio("#000000", "#FFFFFF").unwrap();
        let b = contrast_ratio("#FFFFFF", "#000000").unwrap();
        assert!(approx(a, b, 0.0001));
    }

    #[test]
    fn unparseable_returns_none() {
        assert!(contrast_ratio("not a colour", "#000").is_none());
        assert!(contrast_ratio("#000", "rgb(0,0,0)").is_none());
    }

    #[test]
    fn hex_with_alpha_drops_alpha() {
        // RRGGBBAA — alpha is ignored.
        let r = contrast_ratio("#000000FF", "#FFFFFFFF").unwrap();
        assert!(approx(r, 21.0, 0.01));
    }
}
