//! F1R3Gaze palette and bundled, offline chrome assets.
use parley::FontContext;
use parley::fontique::{Blob, Collection, CollectionOptions, FontInfoOverride, SourceCache};
use crate::profile::layout::Layout;
use gaze_fs::{Fs, Kind};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
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

/// One of the two built-in colour schemes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scheme {
    Dark,
    Light,
}

/// The theme `[appearance] theme` in `settings.toml` asks for.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum ThemeChoice {
    /// Follow the operating system's light or dark preference; dark when it
    /// has none.
    #[default]
    System,
    /// A built-in scheme: `default-dark` or `default-light` (`dark` and
    /// `light` are accepted too).
    BuiltIn(Scheme),
    /// A theme file, `themes/<name>.css`.
    Named(String),
}

/// The longest theme name, in characters.
pub const THEME_NAME_MAX: usize = 64;

/// The names that mean a built-in choice, in any letter case.
const RESERVED_THEME_NAMES: [&str; 5] = ["system", "default-dark", "default-light", "dark", "light"];

/// File names Windows gives to devices, with or without an extension.
const WINDOWS_DEVICE_NAMES: [&str; 24] = [
    "con", "prn", "aux", "nul", "com0", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9", "lpt0",
    "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

impl ThemeChoice {
    /// Reads a `theme` value: `system`, `default-dark` or `dark`,
    /// `default-light` or `light`, or a theme file's name.
    pub fn parse(text: &str) -> Result<ThemeChoice, String> {
        match text {
            "system" => Ok(ThemeChoice::System),
            "default-dark" | "dark" => Ok(ThemeChoice::BuiltIn(Scheme::Dark)),
            "default-light" | "light" => Ok(ThemeChoice::BuiltIn(Scheme::Light)),
            name => check_theme_name(name).map(|()| ThemeChoice::Named(name.to_string())),
        }
    }

    /// The value `settings.toml` stores for this choice.
    pub fn as_str(&self) -> &str {
        match self {
            ThemeChoice::System => "system",
            ThemeChoice::BuiltIn(Scheme::Dark) => "default-dark",
            ThemeChoice::BuiltIn(Scheme::Light) => "default-light",
            ThemeChoice::Named(name) => name,
        }
    }
}

/// Whether `name` can name a theme file, `themes/<name>.css`, on every
/// platform: letters, digits, `.`, `_` and `-`, starting with a letter or a
/// digit, at most [`THEME_NAME_MAX`] characters, not ending in `.` (Windows
/// drops it) or `.css` (the extension is added), and neither a built-in
/// choice nor a Windows device name.
pub fn check_theme_name(name: &str) -> Result<(), String> {
    let lower = name.to_ascii_lowercase();
    let stem = lower.split('.').next().unwrap_or_default();
    match name.chars().next() {
        None => Err("a theme name cannot be empty".into()),
        Some(first) if !first.is_ascii_alphanumeric() => {
            Err(format!("{name:?} must start with a letter or a digit"))
        }
        Some(_) if name.chars().count() > THEME_NAME_MAX => {
            Err(format!("a theme name is at most {THEME_NAME_MAX} characters"))
        }
        Some(_) if !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) => {
            Err(format!("{name:?} may hold only letters, digits, '.', '_' and '-'"))
        }
        Some(_) if lower.ends_with(".css") => {
            Err(format!("write the theme's name without \".css\": {:?}", &name[..name.len() - 4]))
        }
        Some(_) if name.ends_with('.') => Err(format!("{name:?} cannot end with '.'")),
        Some(_) if RESERVED_THEME_NAMES.contains(&lower.as_str()) => {
            Err(format!("{name:?} is a built-in choice; write it in lower case: {lower:?}"))
        }
        Some(_) if WINDOWS_DEVICE_NAMES.contains(&stem) => {
            Err(format!("{name:?} cannot be a file name on Windows"))
        }
        Some(_) => Ok(()),
    }
}

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

/// The largest theme file read, in bytes.
pub const THEME_FILE_MAX: usize = 64 * 1024;

/// Reads a theme file: a restricted subset of CSS that can only set the
/// chrome's colour tokens. Imports, URLs, other selectors and every other
/// kind of value are refused, so a theme cannot reach anything but the
/// chrome's colours.
///
/// ```text
/// file        = [ BOM ] , gap , ( root | items ) , gap ;
/// root        = ":root" , gap , "{" , items , "}" ;
/// items       = { gap , ( declaration | ";" ) } , gap ;
/// declaration = token , gap , ":" , blank , colour , blank , end ;
/// token       = "--gaze-" , name ;          (* one of the 12 tokens *)
/// colour      = "#" , 6 * hexadecimal digit ;
/// end         = ";" | line break | "}" | end of file ;
/// gap         = { white space | comment } ;
/// blank       = { space | tab | "/*" comment "*/" } ;
/// comment     = "/*" , { any character } , "*/"
///             | "#" , { any character but a line break } ;  (* first on its line *)
/// ```
///
/// A declaration given twice keeps the last one, as in CSS. Text, muted
/// text and accent text must reach 4.5:1 against the surface they are drawn
/// on, and the warning, success and danger colours too when the file sets
/// them. Tokens the file leaves out are taken from the built-in scheme it
/// resembles ([`scheme_of`]) for these checks, as [`palette_with`] does.
/// Errors start with the line they are on: `line 3: …`.
pub fn parse_palette(text: &str) -> Result<BTreeMap<String, String>, String> {
    if text.len() > THEME_FILE_MAX {
        return Err(format!("a theme file is at most {} KiB", THEME_FILE_MAX / 1024));
    }
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut sheet = Sheet { text, at: 0, line: 1 };
    let mut out = BTreeMap::new();
    sheet.gap()?;
    let rooted = sheet.eat(":root");
    if rooted {
        sheet.gap()?;
        if !sheet.eat("{") {
            return Err(sheet.error("expected { after :root"));
        }
    }
    loop {
        sheet.gap()?;
        match sheet.peek() {
            None if rooted => return Err(sheet.error("the :root block is not closed with }")),
            None => break,
            Some('}') if rooted => {
                sheet.bump();
                sheet.gap()?;
                match sheet.peek() {
                    None => break,
                    Some(_) => return Err(sheet.error("nothing may follow the :root block")),
                }
            }
            Some(';') => sheet.bump(),
            Some('-') => {
                let (token, colour) = sheet.declaration()?;
                out.insert(token.to_string(), colour);
            }
            Some('@') => return Err(sheet.error("@-rules such as @import are not allowed")),
            Some('{' | '}') => return Err(sheet.error("blocks other than :root { } are not allowed")),
            Some(_) => {
                return Err(sheet.error("expected a --gaze-* colour; a theme file holds only these, optionally inside :root { }"));
            }
        }
    }
    check_contrast(&out)?;
    Ok(out)
}

/// Rejects a palette whose text would be hard to read.
fn check_contrast(declared: &BTreeMap<String, String>) -> Result<(), String> {
    let base = builtin_palette(scheme_of(declared));
    let get = |k: &str| declared.get(k).unwrap_or(&base[k]).clone();
    let pairs = [
        ("--gaze-text", "--gaze-surface"),
        ("--gaze-muted", "--gaze-surface"),
        ("--gaze-accent-text", "--gaze-accent"),
    ];
    for (text, surface) in pairs {
        let ratio = contrast(&get(text), &get(surface)).unwrap_or(0.0);
        if ratio < 4.5 {
            return Err(format!("{text} must reach 4.5:1 against {surface}; it reaches {ratio:.2}:1"));
        }
    }
    for key in &TOKENS[LEGACY_TOKENS..] {
        if let Some(colour) = declared.get(*key) {
            let ratio = contrast(colour, &get("--gaze-surface")).unwrap_or(0.0);
            if ratio < 4.5 {
                return Err(format!("{key} must reach 4.5:1 against --gaze-surface; it reaches {ratio:.2}:1"));
            }
        }
    }
    Ok(())
}

/// A theme file being read: the text, the byte offset and the line.
struct Sheet<'a> {
    text: &'a str,
    at: usize,
    line: usize,
}

impl<'a> Sheet<'a> {
    /// What is left to read. It borrows the text, not the reader, so the
    /// reader can move on while it is held.
    fn rest(&self) -> &'a str {
        &self.text[self.at..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn bump(&mut self) {
        if let Some(c) = self.peek() {
            self.at += c.len_utf8();
            if c == '\n' {
                self.line += 1;
            }
        }
    }

    fn eat(&mut self, word: &str) -> bool {
        match self.rest().starts_with(word) {
            true => {
                for _ in word.chars() {
                    self.bump();
                }
                true
            }
            false => false,
        }
    }

    fn error(&self, problem: &str) -> String {
        format!("line {}: {problem}", self.line)
    }

    /// Whether only spaces and tabs come before the reader on its line.
    fn at_line_start(&self) -> bool {
        self.text[..self.at]
            .chars()
            .rev()
            .take_while(|&c| c != '\n')
            .all(|c| c == ' ' || c == '\t' || c == '\r')
    }

    /// Skips a `/* */` comment the reader is at.
    fn block_comment(&mut self) -> Result<(), String> {
        let line = self.line;
        self.eat("/*");
        while !self.eat("*/") {
            match self.peek() {
                Some(_) => self.bump(),
                None => return Err(format!("line {line}: a comment is not closed with */")),
            }
        }
        Ok(())
    }

    /// Skips white space and comments: `/* */` anywhere, `#` first on a line.
    fn gap(&mut self) -> Result<(), String> {
        loop {
            match self.peek() {
                Some(c) if c.is_whitespace() => self.bump(),
                Some('/') if self.rest().starts_with("/*") => self.block_comment()?,
                Some('#') if self.at_line_start() => {
                    while let Some(c) = self.peek()
                        && c != '\n'
                    {
                        self.bump();
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    /// Skips spaces, tabs and `/* */` comments, staying on the line.
    fn blank(&mut self) -> Result<(), String> {
        loop {
            match self.peek() {
                Some(' ' | '\t') => self.bump(),
                Some('/') if self.rest().starts_with("/*") => self.block_comment()?,
                _ => return Ok(()),
            }
        }
    }

    /// Reads `--gaze-token: #rrggbb` and its end; the colour in lower case.
    fn declaration(&mut self) -> Result<(&'static str, String), String> {
        let start = self.at;
        while let Some(c) = self.peek()
            && (c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            self.bump();
        }
        let name = &self.text[start..self.at];
        let Some(token) = TOKENS.iter().copied().find(|t| *t == name) else {
            return Err(self.error(&format!("{name} is not a colour token; the tokens are {}", TOKENS.join(", "))));
        };
        self.gap()?;
        if !self.eat(":") {
            return Err(self.error(&format!("expected : after {token}")));
        }
        self.blank()?;
        let rest = self.rest();
        if rest.starts_with("url(") {
            return Err(self.error("url() is not allowed"));
        }
        let digits: String = rest.chars().skip(1).take(7).collect();
        let six = digits.chars().take(6).filter(char::is_ascii_hexdigit).count() == 6;
        let seventh = digits.chars().nth(6).is_some_and(|c| c.is_ascii_alphanumeric());
        if !rest.starts_with('#') || !six || seventh {
            return Err(self.error(&format!("{token} must be a colour written #RRGGBB")));
        }
        let colour = format!("#{}", digits[..6].to_ascii_lowercase());
        self.eat(&rest[..7]);
        self.blank()?;
        if self.rest().starts_with('!') {
            return Err(self.error("!important is not allowed"));
        }
        match self.peek() {
            Some(';') => self.bump(),
            None | Some('\n' | '\r' | '}') => {}
            Some(c) => return Err(self.error(&format!("unexpected {c:?} after the colour of {token}"))),
        }
        Ok((token, colour))
    }
}

/// The colours of a built-in scheme.
pub fn builtin_palette(scheme: Scheme) -> BTreeMap<String, String> {
    let colours = match scheme {
        Scheme::Dark => DARK,
        Scheme::Light => LIGHT,
    };
    TOKENS.iter().zip(colours).map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect()
}

/// The relative luminance above which a background is light: where black
/// and white text reach the same contrast on it. With luminance `L`,
/// black text reaches `(L + 0.05) / 0.05` and white text `1.05 / (L + 0.05)`;
/// they are equal when `(L + 0.05)² = 0.0525`, that is
/// `L = √0.0525 − 0.05 ≈ 0.1791`.
const LIGHT_BACKGROUND_LUMINANCE: f64 = 0.179_128_784_747_792;

/// Which built-in scheme a palette resembles: light when its background
/// (`--gaze-bg`, else `--gaze-surface`) is lighter than
/// `LIGHT_BACKGROUND_LUMINANCE`, else dark.
pub fn scheme_of(colours: &BTreeMap<String, String>) -> Scheme {
    let background = colours.get("--gaze-bg").or_else(|| colours.get("--gaze-surface"));
    match background.and_then(|c| luminance(c)) {
        Some(l) if l > LIGHT_BACKGROUND_LUMINANCE => Scheme::Light,
        _ => Scheme::Dark,
    }
}

/// A theme file's colours with every token it leaves out taken from the
/// built-in scheme it resembles.
pub fn palette_with(declared: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut colours = builtin_palette(scheme_of(declared));
    for (token, colour) in declared {
        colours.insert(token.clone(), colour.clone());
    }
    colours
}

/// The palette as CSS custom properties on `:root`.
pub fn variables_of(colours: &BTreeMap<String, String>) -> String {
    let mut css = String::with_capacity(16 + 28 * colours.len());
    css.push_str(":root{");
    for (token, colour) in colours {
        css.push_str(&format!("{token}:{colour};"));
    }
    css.push('}');
    css
}

/// What each token colours, for the comments in theme files.
const TOKEN_USES: [&str; 12] = [
    "the window's background",
    "the current tab, cards and panels",
    "selected and raised items",
    "text",
    "secondary text and icons",
    "borders and fields",
    "focus rings and main buttons",
    "text on the accent colour",
    "what the pointer is over",
    "warnings",
    "success",
    "errors and destructive buttons",
];

/// A theme file's text: `header` as a comment, then a `:root` block with
/// every token in order, each with a comment saying what it colours.
fn theme_css(header: &str, colours: &BTreeMap<String, String>) -> String {
    let mut css = String::with_capacity(header.len() + 1024);
    css.push_str("/*\n");
    for line in header.lines() {
        match line {
            "" => css.push_str(" *\n"),
            line => css.push_str(&format!(" * {line}\n")),
        }
    }
    css.push_str(" */\n:root {\n");
    for (token, uses) in TOKENS.iter().zip(TOKEN_USES) {
        let colour = &colours[*token];
        css.push_str(&format!("  {token}: {colour}; /* {uses} */\n"));
    }
    css.push_str("}\n");
    css
}

/// The text of `themes/default-dark.css.example` or
/// `themes/default-light.css.example`. It reads back as exactly the
/// built-in scheme.
pub fn example_css(scheme: Scheme) -> String {
    let (name, file) = match scheme {
        Scheme::Dark => ("dark", "default-dark.css.example"),
        Scheme::Light => ("light", "default-light.css.example"),
    };
    let header = format!(
        "F1R3Gaze theme: the built-in {name} scheme, {file}.\n\nTo make a theme of your own, copy this file in this folder to a name\nending in .css, such as nord.css, and change its colours. Then choose\nit in Appearance, or set theme = \"nord\" under [appearance] in\nsettings.toml. F1R3Gaze rewrites this .example file when it starts, so\nchange only your copy.\n\nEach colour is written #RRGGBB. Text, muted text and accent text must\nreach a contrast of 4.5:1 against the colour they are drawn on."
    );
    theme_css(&header, &builtin_palette(scheme))
}

/// The text of a new theme file named `name`, holding `colours`.
pub fn new_theme_css(name: &str, colours: &BTreeMap<String, String>) -> String {
    let header = format!(
        "F1R3Gaze theme {name:?}, made in Appearance.\n\nChoose it in Appearance, or set theme = {name:?} under [appearance] in\nsettings.toml. Each colour is written #RRGGBB. Text, muted text and accent\ntext must reach a contrast of 4.5:1 against the colour they are drawn on."
    );
    theme_css(&header, colours)
}

/// Where theme files are found: the user's folder, then the folders of
/// themes installed for everyone, most important first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeDirs {
    pub user: PathBuf,
    pub packaged: Vec<PathBuf>,
}

impl ThemeDirs {
    pub fn of(layout: &Layout) -> ThemeDirs {
        ThemeDirs {
            user: layout.themes_dir(),
            packaged: layout.system_themes.clone(),
        }
    }
}

/// A theme file, and what it holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeFile {
    /// Its name: the file name without `.css`.
    pub name: String,
    pub path: PathBuf,
    /// Installed for everyone, not in the user's folder.
    pub packaged: bool,
    /// The colours it sets, or why it cannot be used.
    pub colours: Result<BTreeMap<String, String>, String>,
}

/// Reads and checks the theme file at `path`.
fn read_theme(fs: &dyn Fs, name: &str, path: PathBuf, packaged: bool) -> ThemeFile {
    let colours = match check_theme_name(name) {
        Err(why) => Err(format!("{why}; rename the file to use it")),
        Ok(()) => match fs.read(&path) {
            Err(e) => Err(format!("cannot be read: {e}")),
            Ok(bytes) if bytes.len() > THEME_FILE_MAX => {
                Err(format!("a theme file is at most {} KiB", THEME_FILE_MAX / 1024))
            }
            Ok(bytes) => match String::from_utf8(bytes) {
                Err(_) => Err("not UTF-8 text".into()),
                Ok(text) => parse_palette(&text),
            },
        },
    };
    ThemeFile {
        name: name.to_string(),
        path,
        packaged,
        colours,
    }
}

/// The theme files in `dirs`, by name: `*.css` files only, so the
/// `.example` files are not among them. A user's file hides an installed one
/// of exactly the same name, and an installed folder hides the ones after
/// it. A folder that cannot be listed is skipped.
pub fn list_themes(fs: &dyn Fs, dirs: &ThemeDirs) -> Vec<ThemeFile> {
    let mut found: BTreeMap<String, ThemeFile> = BTreeMap::new();
    let folders = std::iter::once((&dirs.user, false)).chain(dirs.packaged.iter().map(|d| (d, true)));
    for (dir, packaged) in folders {
        let Ok(names) = fs.list(dir) else { continue };
        for file_name in names {
            let file_name = file_name.to_string_lossy();
            let Some(name) = file_name.strip_suffix(".css") else { continue };
            let path = dir.join(file_name.as_ref());
            match fs.kind(&path) {
                Ok(Kind::File | Kind::Symlink) if !found.contains_key(name) => {
                    found.insert(name.to_string(), read_theme(fs, name, path, packaged));
                }
                _ => {}
            }
        }
    }
    let mut themes: Vec<ThemeFile> = found.into_values().collect();
    themes.sort_by(|a, b| (a.name.to_lowercase(), &a.name).cmp(&(b.name.to_lowercase(), &b.name)));
    themes
}

/// The theme file named `name`, as [`list_themes`] would find it.
pub fn find_theme(fs: &dyn Fs, dirs: &ThemeDirs, name: &str) -> Option<ThemeFile> {
    let folders = std::iter::once((&dirs.user, false)).chain(dirs.packaged.iter().map(|d| (d, true)));
    for (dir, packaged) in folders {
        let path = dir.join(format!("{name}.css"));
        if let Ok(Kind::File | Kind::Symlink) = fs.kind(&path) {
            return Some(read_theme(fs, name, path, packaged));
        }
    }
    None
}

/// The colours a theme choice gives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub colours: BTreeMap<String, String>,
    pub scheme: Scheme,
    /// The theme file used, if any.
    pub file: Option<PathBuf>,
    /// Why the chosen theme file cannot be used. The colours are then the
    /// built-in scheme the system prefers, or dark.
    pub problem: Option<String>,
}

/// The colours for `choice`, with `system` the operating system's
/// preference if it has one.
pub fn resolve(choice: &ThemeChoice, system: Option<Scheme>, fs: &dyn Fs, dirs: &ThemeDirs) -> Resolved {
    let builtin = |scheme: Scheme, problem: Option<String>| Resolved {
        colours: builtin_palette(scheme),
        scheme,
        file: None,
        problem,
    };
    let fallback = system.unwrap_or(Scheme::Dark);
    match choice {
        ThemeChoice::System => builtin(fallback, None),
        ThemeChoice::BuiltIn(scheme) => builtin(*scheme, None),
        ThemeChoice::Named(name) => match find_theme(fs, dirs, name) {
            None => builtin(
                fallback,
                Some(format!("there is no theme {name:?}: no {name}.css in {}", dirs.user.display())),
            ),
            Some(ThemeFile { path, colours: Err(why), .. }) => {
                builtin(fallback, Some(format!("{}: {why}", path.display())))
            }
            Some(ThemeFile { path, colours: Ok(declared), .. }) => {
                let colours = palette_with(&declared);
                Resolved {
                    scheme: scheme_of(&colours),
                    colours,
                    file: Some(path),
                    problem: None,
                }
            }
        },
    }
}

/// A name for a new theme that no file in `themes` has: `my-theme`, then
/// `my-theme-2`, `my-theme-3`, … Names are compared ignoring case, as macOS
/// and Windows compare file names.
pub fn new_theme_name(themes: &[ThemeFile]) -> String {
    let taken = |candidate: &str| themes.iter().any(|t| t.name.eq_ignore_ascii_case(candidate));
    match taken("my-theme") {
        false => "my-theme".into(),
        true => (2u32..)
            .map(|n| format!("my-theme-{n}"))
            .find(|candidate| !taken(candidate))
            .expect("an unbounded range has a free name"),
    }
}

// The line-based reader that `parse_palette` replaced: it accepted only
// `--gaze-token: #RRGGBB` lines and `#` comment lines, and checked contrast
// against the dark scheme whatever the palette was, so a light palette that
// left a token out was checked against a dark colour. Kept for reference
// (storage plan §3.5, ledger S12).
//
// /// A palette file contains only `--gaze-*` colors. CSS syntax, imports and
// /// URLs are deliberately not accepted, keeping user themes scoped to chrome.
// ///
// /// Text, muted text and accent text must reach 4.5:1 against the surface
// /// they are drawn on. The success and danger colours are checked the same
// /// way when the file defines them.
// pub fn parse_palette(text: &str) -> Result<BTreeMap<String, String>, String> {
//     let mut out = BTreeMap::new();
//     for line in text.lines() {
//         let line = line.trim().trim_end_matches(';');
//         if line.is_empty() || line.starts_with('#') {
//             continue;
//         }
//         let (key, value) = line
//             .split_once(':')
//             .ok_or("palette lines must be --gaze-token: #RRGGBB")?;
//         let (key, value) = (key.trim(), value.trim());
//         if !TOKENS.contains(&key) || rgb(value).is_none() {
//             return Err(format!("invalid palette color: {line}"));
//         }
//         out.insert(key.to_string(), value.to_string());
//     }
//     let base = palette("dark", None);
//     let get = |k: &str| out.get(k).cloned().unwrap_or_else(|| base[k].clone());
//     if contrast(&get("--gaze-text"), &get("--gaze-surface")).unwrap_or(0.0) < 4.5
//         || contrast(&get("--gaze-muted"), &get("--gaze-surface")).unwrap_or(0.0) < 4.5
//         || contrast(&get("--gaze-accent-text"), &get("--gaze-accent")).unwrap_or(0.0) < 4.5
//     {
//         return Err("palette text contrast must be at least 4.5:1".into());
//     }
//     for key in &TOKENS[LEGACY_TOKENS..] {
//         if let Some(color) = out.get(*key)
//             && contrast(color, &get("--gaze-surface")).unwrap_or(0.0) < 4.5
//         {
//             return Err(format!("{key} must reach 4.5:1 against --gaze-surface"));
//         }
//     }
//     Ok(out)
// }

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
/// `LEGACY_TOKENS`) gets that token's default that reads best on the
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
    builtin_css_of(&palette(name, custom))
}

/// [`builtin_css`] for colours already resolved ([`resolve`]): the chrome
/// computes it once per theme change and hands it to every built-in page.
pub fn builtin_css_of(colours: &BTreeMap<String, String>) -> String {
    let mut css = format!(
        "{}body{{background:var(--gaze-bg)!important;color:var(--gaze-text)!important}}h1{{color:var(--gaze-text)!important}}code{{background:var(--gaze-raised)!important;color:var(--gaze-text)!important}}button{{background:var(--gaze-surface)!important;color:var(--gaze-text)!important;border-color:var(--gaze-border)!important}}a{{color:var(--gaze-accent)!important}}a.btn{{background:var(--gaze-surface)!important;color:var(--gaze-text)!important;border-color:var(--gaze-border)!important}}summary,.muted{{color:var(--gaze-muted)!important}}",
        variables_of(colours)
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

    /// The built-in pages' style sheet is the same whether it comes from the
    /// old scheme names or from resolved colours.
    #[test]
    fn legacy_and_resolved_css_agree() {
        for (name, scheme) in [("dark", Scheme::Dark), ("light", Scheme::Light)] {
            assert_eq!(palette(name, None), builtin_palette(scheme), "{name}");
            assert_eq!(variables(name, None), variables_of(&builtin_palette(scheme)), "{name}");
            assert_eq!(builtin_css(name, None), builtin_css_of(&builtin_palette(scheme)), "{name}");
        }
    }
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

    #[test]
    fn theme_choices_read_and_write_back() {
        let cases = [
            ("system", ThemeChoice::System, "system"),
            ("default-dark", ThemeChoice::BuiltIn(Scheme::Dark), "default-dark"),
            ("dark", ThemeChoice::BuiltIn(Scheme::Dark), "default-dark"),
            ("default-light", ThemeChoice::BuiltIn(Scheme::Light), "default-light"),
            ("light", ThemeChoice::BuiltIn(Scheme::Light), "default-light"),
            ("nord", ThemeChoice::Named("nord".into()), "nord"),
            ("Solarized_2.1-dim", ThemeChoice::Named("Solarized_2.1-dim".into()), "Solarized_2.1-dim"),
            ("custom", ThemeChoice::Named("custom".into()), "custom"),
        ];
        for (text, choice, stored) in cases {
            assert_eq!(ThemeChoice::parse(text), Ok(choice.clone()), "{text}");
            assert_eq!(choice.as_str(), stored);
            assert_eq!(ThemeChoice::parse(stored), Ok(choice));
        }
        assert_eq!(ThemeChoice::default(), ThemeChoice::System);
    }

    #[test]
    fn theme_names_are_portable_file_names() {
        let longest = "a".repeat(THEME_NAME_MAX);
        assert_eq!(check_theme_name(&longest), Ok(()));
        let refused = [
            "".to_string(),
            "a".repeat(THEME_NAME_MAX + 1),
            "-dark".into(),
            ".hidden".into(),
            "my theme".into(),
            "../etc".into(),
            "a/b".into(),
            "a\\b".into(),
            "naïve".into(),
            "nord.css".into(),
            "nord.CSS".into(),
            "nord.".into(),
            "System".into(),
            "DARK".into(),
            "Default-Light".into(),
            "con".into(),
            "NUL".into(),
            "com1.dark".into(),
            "Lpt9".into(),
        ];
        for name in refused {
            assert!(check_theme_name(&name).is_err(), "{name:?} was accepted");
            assert!(ThemeChoice::parse(&name).is_err(), "{name:?} was accepted as a choice");
        }
        for accepted in ["console", "nul1", "com10", "a..b", "2tone"] {
            assert_eq!(check_theme_name(accepted), Ok(()), "{accepted}");
        }
    }

    fn colours(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn theme_files_accept_the_css_subset() {
        let bg = "--gaze-bg";
        let accepted: [(&str, BTreeMap<String, String>); 8] = [
            ("", colours(&[])),
            ("--gaze-bg: #161b26\n--gaze-text: #EAF0F7;\n", colours(&[(bg, "#161b26"), ("--gaze-text", "#eaf0f7")])),
            ("# a palette from before\n  # indented too\n--gaze-bg: #161b26;\n", colours(&[(bg, "#161b26")])),
            (
                "\u{feff}/* mine */\r\n:root {\r\n  --gaze-bg: #161b26; /* the back */\r\n}\r\n",
                colours(&[(bg, "#161b26")]),
            ),
            (":root{--gaze-bg:#161b26;--gaze-surface:#202735}", colours(&[(bg, "#161b26"), ("--gaze-surface", "#202735")])),
            (";;--gaze-bg: #161b26;;", colours(&[(bg, "#161b26")])),
            ("--gaze-bg: #000000;\n--gaze-bg: #161b26;\n", colours(&[(bg, "#161b26")])),
            ("--gaze-bg /* x */ : /* y */ #161b26 /* z */ ;", colours(&[(bg, "#161b26")])),
        ];
        for (text, wanted) in accepted {
            assert_eq!(parse_palette(text), Ok(wanted), "{text:?}");
        }
    }

    #[test]
    fn theme_files_refuse_everything_else() {
        let refused = [
            ("@import url(x.css);", "line 1: @-rules"),
            ("body { color: red }", "line 1: expected a --gaze-* colour"),
            (":root { --gaze-bg: url(x.png) }", "line 1: url() is not allowed"),
            ("--gaze-bg: red;", "line 1: --gaze-bg must be a colour written #RRGGBB"),
            ("--gaze-bg: #fff;", "line 1: --gaze-bg must be a colour written #RRGGBB"),
            ("--gaze-bg: #161b26ff;", "line 1: --gaze-bg must be a colour written #RRGGBB"),
            ("--gaze-bg: #161b26 !important;", "line 1: !important is not allowed"),
            (":root { --gaze-bg: #161b26; { } }", "line 1: blocks other than :root"),
            ("--gaze-nope: #161b26;", "line 1: --gaze-nope is not a colour token"),
            (":root {\n--gaze-bg: #161b26;\n", "line 3: the :root block is not closed"),
            (":root { }\n--gaze-bg: #161b26;", "line 2: nothing may follow the :root block"),
            ("--gaze-bg: #161b26;\n/* open\n", "line 2: a comment is not closed"),
            ("--gaze-bg: #161b26 x", "line 1: unexpected 'x'"),
            ("--gaze-bg: #161b26; # not first on its line", "line 1: expected a --gaze-* colour"),
            ("\n\n--gaze-bg: rgb(0, 0, 0)", "line 3: --gaze-bg must be a colour"),
            ("--gaze-bg: #161b26;\n--gaze-text: #1b2130;\n", "--gaze-text must reach 4.5:1 against --gaze-surface"),
        ];
        for (text, wanted) in refused {
            let error = parse_palette(text).expect_err(text);
            assert!(error.starts_with(wanted) || error.contains(wanted), "{text:?}: {error}");
        }
        let large = format!("/*{}*/", " ".repeat(THEME_FILE_MAX));
        assert!(parse_palette(&large).expect_err("too large").contains("at most 64 KiB"));
    }

    #[test]
    fn the_examples_read_back_as_the_builtin_schemes() {
        for scheme in [Scheme::Dark, Scheme::Light] {
            let example = example_css(scheme);
            assert_eq!(parse_palette(&example), Ok(builtin_palette(scheme)), "{example}");
            assert_eq!(scheme_of(&builtin_palette(scheme)), scheme);
            assert!(example.contains("F1R3Gaze rewrites this .example file"), "{example}");
        }
        let nord = palette_with(&colours(&[("--gaze-accent", "#88c0d0")]));
        assert_eq!(parse_palette(&new_theme_css("nord", &nord)), Ok(nord));
    }

    /// The threshold is where black and white text read equally well: a
    /// background is light exactly when black text has more contrast on it.
    #[test]
    fn palettes_are_classified_by_their_background() {
        for level in 0..=255u8 {
            let grey = format!("#{level:02x}{level:02x}{level:02x}");
            let light = contrast(&grey, "#000000").expect("grey") > contrast(&grey, "#ffffff").expect("grey");
            let wanted = if light { Scheme::Light } else { Scheme::Dark };
            assert_eq!(scheme_of(&colours(&[("--gaze-bg", &grey)])), wanted, "{grey}");
        }
        assert_eq!(scheme_of(&colours(&[("--gaze-surface", "#ffffff")])), Scheme::Light, "the surface when there is no bg");
        assert_eq!(scheme_of(&colours(&[("--gaze-bg", "#000000"), ("--gaze-surface", "#ffffff")])), Scheme::Dark, "bg first");
        assert_eq!(scheme_of(&colours(&[])), Scheme::Dark);
    }

    #[test]
    fn a_light_theme_takes_what_it_leaves_out_from_the_light_scheme() {
        let declared = parse_palette("--gaze-bg: #ffffff;\n--gaze-text: #111111;\n").expect("a light theme");
        let colours = palette_with(&declared);
        assert_eq!(colours["--gaze-bg"], "#ffffff");
        assert_eq!(colours["--gaze-text"], "#111111");
        for (i, token) in TOKENS.iter().enumerate() {
            if !matches!(*token, "--gaze-bg" | "--gaze-text") {
                assert_eq!(colours[*token], LIGHT[i], "{token}");
            }
        }
    }

    fn theme_fs() -> (gaze_fs::MemFs, ThemeDirs) {
        let fs = gaze_fs::MemFs::new();
        let dirs = ThemeDirs {
            user: PathBuf::from("/c/themes"),
            packaged: vec![PathBuf::from("/usr/local/share/t"), PathBuf::from("/usr/share/t")],
        };
        fs.seed_file("/c/themes/nord.css", b"--gaze-accent: #88c0d0;\n");
        fs.seed_file("/c/themes/default-dark.css.example", example_css(Scheme::Dark).as_bytes());
        fs.seed_file("/c/themes/My Theme.css", b"");
        fs.seed_dir("/c/themes/folder.css");
        fs.seed_file("/usr/local/share/t/nord.css", b"--gaze-accent: #000000;\n");
        fs.seed_file("/usr/local/share/t/solar.css", b"--gaze-bg: #fdf6e3;\n--gaze-text: #073642;\n");
        fs.seed_file("/usr/share/t/solar.css", b"--gaze-bg: #000000;\n");
        fs.seed_file("/usr/share/t/broken.css", b"--gaze-bg: #000000;\nbody { }\n");
        (fs, dirs)
    }

    #[test]
    fn user_themes_hide_installed_ones_of_the_same_name() {
        let (fs, dirs) = theme_fs();
        let themes = list_themes(&fs, &dirs);
        let names: Vec<(&str, bool, bool)> = themes.iter().map(|t| (t.name.as_str(), t.packaged, t.colours.is_ok())).collect();
        assert_eq!(names, [("broken", true, false), ("My Theme", false, false), ("nord", false, true), ("solar", true, true)]);
        assert_eq!(themes[2].path, Path::new("/c/themes/nord.css"));
        assert_eq!(themes[3].path, Path::new("/usr/local/share/t/solar.css"), "the more important folder wins");
        assert!(themes[0].colours.as_ref().expect_err("broken").starts_with("line 2:"));
        assert!(themes[1].colours.as_ref().expect_err("bad name").contains("rename the file"));
        assert_eq!(find_theme(&fs, &dirs, "solar").map(|t| t.path), Some(PathBuf::from("/usr/local/share/t/solar.css")));
        assert_eq!(find_theme(&fs, &dirs, "missing"), None);
    }

    #[test]
    fn a_theme_that_cannot_be_used_falls_back_and_says_why() {
        let (fs, dirs) = theme_fs();
        let choose = |choice: ThemeChoice, system: Option<Scheme>| resolve(&choice, system, &fs, &dirs);
        assert_eq!(choose(ThemeChoice::System, Some(Scheme::Light)).colours, builtin_palette(Scheme::Light));
        assert_eq!(choose(ThemeChoice::System, None).scheme, Scheme::Dark, "dark when the system has no preference");
        assert_eq!(choose(ThemeChoice::BuiltIn(Scheme::Light), Some(Scheme::Dark)).scheme, Scheme::Light);
        let nord = choose(ThemeChoice::Named("nord".into()), None);
        assert_eq!((nord.colours["--gaze-accent"].as_str(), nord.scheme, nord.problem), ("#88c0d0", Scheme::Dark, None));
        assert_eq!(nord.file, Some(PathBuf::from("/c/themes/nord.css")));
        let solar = choose(ThemeChoice::Named("solar".into()), Some(Scheme::Dark));
        assert_eq!(solar.scheme, Scheme::Light, "classified by its own background");
        let missing = choose(ThemeChoice::Named("missing".into()), Some(Scheme::Light));
        assert_eq!(missing.colours, builtin_palette(Scheme::Light));
        assert!(missing.problem.expect("a reason").contains("no missing.css in /c/themes"));
        let broken = choose(ThemeChoice::Named("broken".into()), None);
        assert_eq!(broken.scheme, Scheme::Dark);
        let why = broken.problem.expect("a reason");
        assert!(why.contains("broken.css: line 2:"), "{why}");
    }

    #[test]
    fn new_theme_names_never_clash() {
        let named = |names: &[&str]| -> Vec<ThemeFile> {
            names
                .iter()
                .map(|n| ThemeFile {
                    name: n.to_string(),
                    path: PathBuf::from(format!("/c/themes/{n}.css")),
                    packaged: false,
                    colours: Ok(BTreeMap::new()),
                })
                .collect()
        };
        assert_eq!(new_theme_name(&named(&[])), "my-theme");
        assert_eq!(new_theme_name(&named(&["my-theme"])), "my-theme-2");
        assert_eq!(new_theme_name(&named(&["My-Theme", "my-theme-2"])), "my-theme-3");
    }
}
