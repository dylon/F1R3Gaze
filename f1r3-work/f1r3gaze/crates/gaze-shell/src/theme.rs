//! F1R3Gaze palette and bundled, offline chrome assets.
use parley::FontContext;
use parley::fontique::{Blob, Collection, CollectionOptions, FontInfoOverride, SourceCache};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

/// The palette's colour tokens, in palette-file order.
const TOKENS: &[&str] = &[
    "--gaze-bg",
    "--gaze-surface",
    "--gaze-raised",
    "--gaze-text",
    "--gaze-muted",
    "--gaze-border",
    "--gaze-accent",
    "--gaze-accent-text",
    "--gaze-hover",
    "--gaze-warning",
    "--gaze-success",
    "--gaze-danger",
];

/// Tokens before this index existed when custom palettes were introduced.
/// A palette file may omit the newer ones; see [`palette`].
const LEGACY_TOKENS: usize = 10;

const DARK: &[&str] = &[
    "#161b26", "#202735", "#293244", "#eaf0f7", "#a8b7c9", "#3a4659", "#71d4cf", "#10272c",
    "#344156", "#efbf75", "#7ddc9a", "#ff8a80",
];
const LIGHT: &[&str] = &[
    "#f5f7fa", "#ffffff", "#eaf0f4", "#182734", "#526879", "#c9d7df", "#087f86", "#ffffff",
    "#dde9ed", "#955300", "#146c43", "#b42318",
];

fn rgb(s: &str) -> Option<[f64; 3]> {
    if s.len() != 7 || !s.starts_with('#') {
        return None;
    }
    Some([
        u8::from_str_radix(&s[1..3], 16).ok()? as f64 / 255.0,
        u8::from_str_radix(&s[3..5], 16).ok()? as f64 / 255.0,
        u8::from_str_radix(&s[5..7], 16).ok()? as f64 / 255.0,
    ])
}

fn luminance(s: &str) -> Option<f64> {
    let c = rgb(s)?;
    let f = |v: f64| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    Some(0.2126 * f(c[0]) + 0.7152 * f(c[1]) + 0.0722 * f(c[2]))
}

fn contrast(a: &str, b: &str) -> Option<f64> {
    let (a, b) = (luminance(a)?, luminance(b)?);
    Some((a.max(b) + 0.05) / (a.min(b) + 0.05))
}

/// The palette file in the profile directory, parsed by [`parse_palette`].
pub fn custom_palette(dir: &Path) -> Result<BTreeMap<String, String>, String> {
    let text = std::fs::read_to_string(dir.join("palette.css")).map_err(|e| e.to_string())?;
    parse_palette(&text)
}

/// A palette file contains only `--gaze-*` colors. CSS syntax, imports and
/// URLs are deliberately not accepted, keeping user themes scoped to chrome.
///
/// Text, muted text and accent text must reach 4.5:1 against the surface
/// they are drawn on. The success and danger colours are checked the same
/// way when the file defines them.
pub fn parse_palette(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim().trim_end_matches(';');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once(':')
            .ok_or("palette lines must be --gaze-token: #RRGGBB")?;
        let (key, value) = (key.trim(), value.trim());
        if !TOKENS.contains(&key) || rgb(value).is_none() {
            return Err(format!("invalid palette color: {line}"));
        }
        out.insert(key.to_string(), value.to_string());
    }
    let base = palette("dark", None);
    let get = |k: &str| out.get(k).cloned().unwrap_or_else(|| base[k].clone());
    if contrast(&get("--gaze-text"), &get("--gaze-surface")).unwrap_or(0.0) < 4.5
        || contrast(&get("--gaze-muted"), &get("--gaze-surface")).unwrap_or(0.0) < 4.5
        || contrast(&get("--gaze-accent-text"), &get("--gaze-accent")).unwrap_or(0.0) < 4.5
    {
        return Err("palette text contrast must be at least 4.5:1".into());
    }
    for key in &TOKENS[LEGACY_TOKENS..] {
        if let Some(color) = out.get(*key)
            && contrast(color, &get("--gaze-surface")).unwrap_or(0.0) < 4.5
        {
            return Err(format!("{key} must reach 4.5:1 against --gaze-surface"));
        }
    }
    Ok(out)
}

/// The default for token `index` that reads best on `surface`: the dark or
/// the light scheme's value, whichever has more contrast with it.
fn legible_default(index: usize, surface: &str) -> &'static str {
    match (contrast(DARK[index], surface), contrast(LIGHT[index], surface)) {
        (Some(dark), Some(light)) if light > dark => LIGHT[index],
        _ => DARK[index],
    }
}

/// Every token name, in palette-file order (for swatches).
pub fn token_names() -> &'static [&'static str] {
    TOKENS
}

/// The colours of scheme `name`, with a custom palette's values on top.
/// A custom palette written before a token existed (index at or beyond
/// [`LEGACY_TOKENS`]) gets that token's default that reads best on the
/// palette's own surface.
pub fn palette(name: &str, custom: Option<&BTreeMap<String, String>>) -> BTreeMap<String, String> {
    let colors = if name == "light" { LIGHT } else { DARK };
    let custom_surface = custom.and_then(|c| c.get("--gaze-surface"));
    TOKENS
        .iter()
        .zip(colors)
        .enumerate()
        .map(|(index, (k, v))| {
            let value = match (custom.and_then(|c| c.get(*k)), custom_surface) {
                (Some(own), _) => own.clone(),
                (None, Some(surface)) if index >= LEGACY_TOKENS => {
                    legible_default(index, surface).to_string()
                }
                (None, _) => (*v).to_string(),
            };
            ((*k).to_string(), value)
        })
        .collect()
}

pub fn variables(name: &str, custom: Option<&BTreeMap<String, String>>) -> String {
    let mut css = String::from(":root{");
    for (key, value) in palette(name, custom) {
        css.push_str(&format!("{key}:{value};"));
    }
    css.push('}');
    css
}

pub fn font_css() -> String {
    let mut css = include_str!("../assets/fontawesome.min.css").to_string();
    let solid = include_str!("../assets/solid.min.css");
    css.push_str(&solid.replace("@font-face{font-family:\"Font Awesome 7 Free\";font-style:normal;font-weight:900;font-display:block;src:url(../webfonts/fa-solid-900.woff2)}", ""));
    css
}

/// The vendored faces, each with the family name the stylesheet asks for.
/// They are registered under that name rather than the one in their own name
/// table (ledger L10). The vendored Fira Code is a variable font whose name
/// table calls it "Fira Code Light", after its default instance. A lookup of
/// 'Fira Code' therefore never found it, and each platform drew monospace
/// text in a face of its own: Fira Code where it happened to be installed,
/// DejaVu Sans Mono, Courier, or the narrower Consolas. Its weight comes from
/// its `wght` axis, which fontique sets to the weight asked for.
const BUNDLED_FONTS: [(&[u8], &str); 4] = [
    (include_bytes!("../assets/NotoSans-Regular.otf"), "Noto Sans"),
    (include_bytes!("../assets/NotoSans-SemiBold.otf"), "Noto Sans"),
    (include_bytes!("../assets/fira-code-latin-wght-normal.woff2"), "Fira Code"),
    (include_bytes!("../assets/fa-solid-900.woff2"), "Font Awesome 7 Free"),
];

/// Register the vendored faces directly with Parley. The chrome can use them
/// before any CSS resource request completes, including in offline builds.
pub fn font_context() -> FontContext {
    let mut ctx = FontContext {
        source_cache: SourceCache::new_shared(),
        collection: Collection::new(CollectionOptions { shared: false, system_fonts: true }),
    };
    register_bundled_fonts(&mut ctx.collection);
    ctx
}

/// Register the vendored faces with `collection` under the stylesheet's family
/// names. Returns the family names they were registered under, in order.
fn register_bundled_fonts(collection: &mut Collection) -> Vec<String> {
    let mut names = Vec::with_capacity(BUNDLED_FONTS.len());
    for (bytes, family) in BUNDLED_FONTS {
        let decoded = blitz_dom::decode_font_bytes(bytes).into_owned();
        let named = FontInfoOverride {
            family_name: Some(family),
            ..FontInfoOverride::default()
        };
        for (id, _) in collection.register_fonts(Blob::new(Arc::new(decoded) as _), Some(named)) {
            if let Some(name) = collection.family_name(id) {
                names.push(name.to_owned());
            }
        }
    }
    names
}

/// The new-tab page's lamp while it is lit (`button.lit` in `pages.rs`). The
/// colours are the same in both schemes: the page's own yellow, a dark label,
/// and an edge in a darker shade of the page's amber. The yellow is close to
/// the light scheme's background, so on the light page the edge outlines the
/// lit lamp. It reaches 3:1 against both schemes' backgrounds (ledger L11).
pub(crate) const LAMP_LIT_FILL: &str = "#ffcf3f";
pub(crate) const LAMP_LIT_LABEL: &str = "#1d2330";
pub(crate) const LAMP_LIT_EDGE: &str = "#a37f00";

/// The host theme of the built-in pages (`gaze://…`) and the error page. The
/// chrome installs it as a user-agent style sheet
/// (`RhoDocument::set_host_theme`), and its declarations are `!important`.
/// An important user-agent declaration beats every author declaration,
/// whatever the selectors' specificity (CSS Cascade 4, §6.1). So these rules
/// override the pages' own rules. A page state that changes a property they
/// set, such as the lamp's `button.lit`, shows only if this sheet styles that
/// state too (ledger L11).
pub fn builtin_css(name: &str, custom: Option<&BTreeMap<String, String>>) -> String {
    let mut css = format!(
        "{}body{{background:var(--gaze-bg)!important;color:var(--gaze-text)!important}}h1{{color:var(--gaze-text)!important}}code{{background:var(--gaze-raised)!important;color:var(--gaze-text)!important}}button{{background:var(--gaze-surface)!important;color:var(--gaze-text)!important;border-color:var(--gaze-border)!important}}a{{color:var(--gaze-accent)!important}}a.btn{{background:var(--gaze-surface)!important;color:var(--gaze-text)!important;border-color:var(--gaze-border)!important}}summary,.muted{{color:var(--gaze-muted)!important}}",
        variables(name, custom)
    );
    // The lit lamp. Both rules are important user-agent rules, so the more
    // specific `button.lit` wins over `button` above.
    css.push_str(&format!(
        "button.lit{{background:{LAMP_LIT_FILL}!important;color:{LAMP_LIT_LABEL}!important;border-color:{LAMP_LIT_EDGE}!important}}"
    ));
    css
}

#[cfg(test)]
mod tests {
    use super::*;
    const BG: usize = 0;
    const SURFACE: usize = 1;
    const RAISED: usize = 2;
    const TEXT: usize = 3;
    const MUTED: usize = 4;
    const ACCENT: usize = 6;
    const ACCENT_TEXT: usize = 7;
    const HOVER: usize = 8;
    const SUCCESS: usize = 10;
    const DANGER: usize = 11;

    #[test]
    fn every_token_has_a_dark_and_a_light_value() {
        assert_eq!(TOKENS.len(), DARK.len());
        assert_eq!(TOKENS.len(), LIGHT.len());
        assert_eq!(token_names(), TOKENS);
        assert_eq!(TOKENS[SUCCESS], "--gaze-success");
        assert_eq!(TOKENS[DANGER], "--gaze-danger");
    }

    #[test]
    fn palettes_are_legible() {
        let ratio = |fg: &str, bg: &str| contrast(fg, bg).expect("built-in palette colours are #RRGGBB");
        for p in [DARK, LIGHT] {
            assert!(ratio(p[TEXT], p[SURFACE]) >= 4.5);
            assert!(ratio(p[MUTED], p[SURFACE]) >= 4.5);
            assert!(ratio(p[ACCENT_TEXT], p[ACCENT]) >= 4.5);
            // Success and danger colour text on every chrome background.
            for background in [BG, SURFACE, RAISED, HOVER] {
                for tone in [SUCCESS, DANGER] {
                    let r = ratio(p[tone], p[background]);
                    assert!(r >= 4.5, "{} on {}: {r:.2}", TOKENS[tone], TOKENS[background]);
                }
                // Accent marks controls (bars, rings, icons): 3:1 for
                // non-text contrast (WCAG 2.1, 1.4.11). Accent *text* is
                // only drawn on the surface, at 4.5:1.
                let r = ratio(p[ACCENT], p[background]);
                assert!(r >= 3.0, "accent on {}: {r:.2}", TOKENS[background]);
            }
            assert!(ratio(p[ACCENT], p[SURFACE]) >= 4.5);
            // A solid danger button draws the bg colour on the danger colour.
            assert!(ratio(p[BG], p[DANGER]) >= 4.5);
        }
    }

    #[test]
    fn palette_files_written_before_new_tokens_still_parse() {
        let legacy: String = TOKENS[..LEGACY_TOKENS]
            .iter()
            .zip(DARK)
            .map(|(k, v)| format!("{k}: {v};\n"))
            .collect();
        let parsed = parse_palette(&legacy).expect("a ten-token palette is valid");
        assert!(!parsed.contains_key("--gaze-danger"));
        let colors = palette("custom", Some(&parsed));
        assert_eq!(colors["--gaze-danger"], DARK[DANGER]);
        assert_eq!(colors["--gaze-success"], DARK[SUCCESS]);
    }

    #[test]
    fn missing_new_tokens_follow_the_custom_surface() {
        let parsed = parse_palette(
            "--gaze-surface: #ffffff;\n--gaze-text: #182734;\n--gaze-muted: #526879;\n",
        )
        .expect("a light palette is valid");
        let colors = palette("custom", Some(&parsed));
        assert_eq!(colors["--gaze-danger"], LIGHT[DANGER]);
        assert_eq!(colors["--gaze-success"], LIGHT[SUCCESS]);
    }

    /// Ledger L11: the lit lamp's label reads at 4.5:1 on its yellow (WCAG
    /// 2.1, 1.4.3). The lit state's edge reaches 3:1 against both schemes'
    /// backgrounds (1.4.11): the yellow itself barely differs from the light
    /// one.
    #[test]
    fn the_lit_lamp_is_legible_in_both_schemes() {
        let ratio = |fg: &str, bg: &str| contrast(fg, bg).expect("the lamp's colours are #RRGGBB");
        let label = ratio(LAMP_LIT_LABEL, LAMP_LIT_FILL);
        assert!(label >= 4.5, "label on the lit fill: {label:.2}");
        for p in [DARK, LIGHT] {
            let edge = ratio(LAMP_LIT_EDGE, p[BG]);
            assert!(edge >= 3.0, "lit edge on {}: {edge:.2}", p[BG]);
        }
    }

    #[test]
    fn low_contrast_new_tokens_are_rejected() {
        let error = parse_palette("--gaze-danger: #2a3040;\n").expect_err("illegible danger");
        assert!(error.contains("--gaze-danger"), "{error}");
        assert!(parse_palette("--gaze-success: #7ddc9a;\n").is_ok());
    }

    /// Ledger L10: every vendored face is registered under the family name
    /// the stylesheet asks for, so no platform falls back to a face of its
    /// own. System fonts are left out, so a face installed on the machine
    /// cannot stand in for a missing one.
    #[test]
    fn bundled_faces_register_under_the_stylesheet_names() {
        let mut collection = Collection::new(CollectionOptions {
            shared: false,
            system_fonts: false,
        });
        let names = register_bundled_fonts(&mut collection);
        let wanted: Vec<&str> = BUNDLED_FONTS.iter().map(|&(_, family)| family).collect();
        assert_eq!(names, wanted);
        for family in ["Noto Sans", "Fira Code", "Font Awesome 7 Free"] {
            assert!(
                collection.family_id(family).is_some(),
                "{family} is not registered"
            );
        }
    }
}
