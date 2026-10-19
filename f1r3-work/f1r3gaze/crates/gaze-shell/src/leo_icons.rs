//! Map chrome markers to the bundled Leo icon font. The source SVGs carry MPL 2.0.

fn glyph(name: &str) -> Option<&'static str> {
    Some(match name {
        "fa-arrow-left" => "browser-back",
        "fa-arrow-right" => "browser-forward",
        "fa-rotate-right" => "browser-refresh",
        "fa-table-columns" => "browser-sidebar",
        "fa-layer-group" => "window-tab",
        "fa-clock-rotate-left" | "fa-clock" => "history",
        "fa-database" => "database",
        "fa-shield-halved" => "shield",
        "fa-wallet" => "crypto-wallets",
        "fa-terminal" => "window-console",
        "fa-gear" => "settings",
        "fa-magnifying-glass" => "search",
        "fa-lock" => "lock-plain",
        "fa-xmark" => "close",
        "fa-plus" => "plus-add",
        "fa-copy" | "fa-clone" => "copy",
        "fa-check" | "fa-circle-check" => "check-normal",
        "fa-triangle-exclamation" | "fa-circle-exclamation" => "warning-triangle-outline",
        "fa-file" | "fa-file-lines" => "file",
        "fa-file-import" => "import-arrow",
        "fa-file-export" | "fa-arrow-down" => "download",
        "fa-eye" => "eye-on",
        "fa-globe" => "globe",
        "fa-circle-info" => "info-outline",
        "fa-ellipsis" => "more-horizontal",
        "fa-trash-can" => "trash",
        "fa-circle-half-stroke" | "fa-palette" | "fa-swatchbook" => "browser-appearance",
        "fa-bars" => "browser-menu",
        "fa-hourglass-half" => "loading-spinner",
        "fa-folder-open" => "folder-import",
        _ => return None,
    })
}

/// Replace a supported Font Awesome marker with the matching Leo font glyph.
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
        if let Some(name) = icon {
            output.push_str("<i class=\"leo-icon");
            output.push_str(" leo-");
            output.push_str(name);
            for class in classes
                .split_ascii_whitespace()
                .filter(|class| !class.starts_with("fa-"))
            {
                output.push(' ');
                output.push_str(class);
            }
            output.push_str("\" aria-hidden=\"true\">");
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
        assert!(rendered.contains("class=\"leo-icon leo-browser-back row-icon\""));
        assert!(!rendered.contains("fa-arrow-left"));
    }

    #[test]
    fn leaves_unmapped_icon_and_other_markup_intact() {
        let html = "<span><i class=\"fa-solid fa-dice\"></i></span>";
        assert_eq!(inline(html), html);
    }
}
