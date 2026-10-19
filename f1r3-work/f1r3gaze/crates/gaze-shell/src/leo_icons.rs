//! Inline Leo glyphs in the browser chrome. The source SVG files carry MPL 2.0.

fn glyph(name: &str) -> Option<&'static str> {
    Some(match name {
        "fa-arrow-left" => include_str!("../assets/leo/browser-back.svg"),
        "fa-arrow-right" => include_str!("../assets/leo/browser-forward.svg"),
        "fa-rotate-right" => include_str!("../assets/leo/browser-refresh.svg"),
        "fa-table-columns" => include_str!("../assets/leo/browser-sidebar.svg"),
        "fa-layer-group" => include_str!("../assets/leo/window-tab.svg"),
        "fa-clock-rotate-left" | "fa-clock" => include_str!("../assets/leo/history.svg"),
        "fa-database" => include_str!("../assets/leo/database.svg"),
        "fa-shield-halved" => include_str!("../assets/leo/shield.svg"),
        "fa-wallet" => include_str!("../assets/leo/crypto-wallets.svg"),
        "fa-terminal" => include_str!("../assets/leo/window-console.svg"),
        "fa-gear" => include_str!("../assets/leo/settings.svg"),
        "fa-magnifying-glass" => include_str!("../assets/leo/search.svg"),
        "fa-lock" => include_str!("../assets/leo/lock-plain.svg"),
        "fa-xmark" => include_str!("../assets/leo/close.svg"),
        "fa-plus" => include_str!("../assets/leo/plus-add.svg"),
        "fa-copy" | "fa-clone" => include_str!("../assets/leo/copy.svg"),
        "fa-check" | "fa-circle-check" => include_str!("../assets/leo/check-normal.svg"),
        "fa-triangle-exclamation" | "fa-circle-exclamation" => {
            include_str!("../assets/leo/warning-triangle-outline.svg")
        }
        "fa-file" | "fa-file-lines" => include_str!("../assets/leo/file.svg"),
        "fa-file-import" => include_str!("../assets/leo/import-arrow.svg"),
        "fa-file-export" | "fa-arrow-down" => include_str!("../assets/leo/download.svg"),
        "fa-eye" => include_str!("../assets/leo/eye-on.svg"),
        "fa-globe" => include_str!("../assets/leo/globe.svg"),
        "fa-circle-info" => include_str!("../assets/leo/info-outline.svg"),
        "fa-ellipsis" => include_str!("../assets/leo/more-horizontal.svg"),
        "fa-trash-can" => include_str!("../assets/leo/trash.svg"),
        "fa-circle-half-stroke" | "fa-palette" | "fa-swatchbook" => {
            include_str!("../assets/leo/browser-appearance.svg")
        }
        "fa-bars" => include_str!("../assets/leo/browser-menu.svg"),
        "fa-hourglass-half" => include_str!("../assets/leo/loading-spinner.svg"),
        "fa-folder-open" => include_str!("../assets/leo/folder-import.svg"),
        _ => return None,
    })
}

/// Replace a supported Font Awesome marker with the matching Leo SVG.
/// Other markers keep their original glyph until the archive supplies one.
pub fn inline(html: &str) -> String {
    const OPEN: &str = "<i class=\"fa-solid ";
    const CLOSE: &str = "</i>";
    let mut output = String::with_capacity(html.len());
    let mut remaining = html;
    while let Some(start) = remaining.find(OPEN) {
        output.push_str(&remaining[..start]);
        remaining = &remaining[start..];
        let Some(class_end) = remaining[OPEN.len()..].find('"') else {
            break;
        };
        let class_end = OPEN.len() + class_end;
        let classes = &remaining[OPEN.len()..class_end];
        let after_class = &remaining[class_end + 1..];
        if !after_class.starts_with('>') || !after_class[1..].starts_with(CLOSE) {
            output.push_str(OPEN);
            remaining = &remaining[OPEN.len()..];
            continue;
        }
        let end = class_end + 2 + CLOSE.len();
        let icon = classes.split_ascii_whitespace().find_map(glyph);
        if let Some(svg) = icon {
            output.push_str("<i class=\"leo-icon");
            for class in classes
                .split_ascii_whitespace()
                .filter(|class| !class.starts_with("fa-"))
            {
                output.push(' ');
                output.push_str(class);
            }
            output.push_str("\" aria-hidden=\"true\">");
            output.push_str(svg);
            output.push_str(CLOSE);
        } else {
            output.push_str(&remaining[..end]);
        }
        remaining = &remaining[end..];
    }
    output.push_str(remaining);
    output
}

#[cfg(test)]
mod tests {
    use super::inline;

    #[test]
    fn replaces_chrome_icon_and_preserves_layout_class() {
        let html = "<button><i class=\"fa-solid fa-arrow-left row-icon\"></i></button>";
        let rendered = inline(html);
        assert!(rendered.contains("class=\"leo-icon row-icon\""));
        assert!(rendered.contains("fill=\"currentColor\""));
        assert!(rendered.contains("viewBox=\"0 0 24 24\""));
        assert!(!rendered.contains("fa-arrow-left"));
    }

    #[test]
    fn leaves_unmapped_icon_and_other_markup_intact() {
        let html = "<span><i class=\"fa-solid fa-dice\"></i></span>";
        assert_eq!(inline(html), html);
    }
}
