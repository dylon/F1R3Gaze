//! Exact text fitting for the chrome.
//!
//! Blitz (674d7d2) does not implement `text-overflow: ellipsis`: an
//! overflowing label is simply cut, often mid-glyph. The chrome therefore
//! shortens labels before it renders them.
//!
//! Widths are measured with parley, the text engine Blitz itself lays text
//! out with. The measurement uses the same bundled fonts ([`theme::font_context`])
//! and the same style parameters Blitz derives from the chrome stylesheet:
//! family list, size, weight, collapsed white space, and pixel-quantized
//! advances at the window's scale. A shortened label is therefore the
//! longest one that fits its box, not an estimate.
//!
//! Each fitting is a binary search over character boundaries. Results are
//! memoised, so re-rendering an unchanged panel costs a hash lookup.
//!
//! [`theme::font_context`]: crate::theme::font_context

use crate::theme;
use parley::{
    FontContext, FontFamily, FontFamilyName, FontWeight, GenericFamily, LayoutContext, TextStyle,
    WhiteSpaceCollapse,
};
use std::borrow::Cow;
use std::collections::HashMap;
use std::ops::Range;

/// The horizontal ellipsis that replaces removed text.
pub const ELLIPSIS: &str = "…";

/// A typeface of the chrome stylesheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Face {
    /// `'Noto Sans', system-ui, sans-serif`, weight 400.
    Sans,
    /// The same family at weight 600.
    SansSemibold,
    /// `'Fira Code', monospace`, weight 400.
    Mono,
}

/// A face at a CSS pixel size, in tenths of a pixel (`130` is 13 px), so
/// fonts can be hashed and compared exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Font {
    pub face: Face,
    pub tenths_px: u16,
}

impl Font {
    pub const fn new(face: Face, tenths_px: u16) -> Font {
        Font { face, tenths_px }
    }

    fn size_px(self) -> f32 {
        f32::from(self.tenths_px) / 10.0
    }

    fn style(self) -> TextStyle<'static, 'static, ()> {
        const SANS: &[FontFamilyName<'static>] = &[
            FontFamilyName::Named(Cow::Borrowed("Noto Sans")),
            FontFamilyName::Generic(GenericFamily::SystemUi),
            FontFamilyName::Generic(GenericFamily::SansSerif),
        ];
        const MONO: &[FontFamilyName<'static>] = &[
            FontFamilyName::Named(Cow::Borrowed("Fira Code")),
            FontFamilyName::Generic(GenericFamily::Monospace),
        ];
        let (families, weight) = match self.face {
            Face::Sans => (SANS, FontWeight::NORMAL),
            Face::SansSemibold => (SANS, FontWeight::SEMI_BOLD),
            Face::Mono => (MONO, FontWeight::NORMAL),
        };
        TextStyle {
            font_family: FontFamily::List(Cow::Borrowed(families)),
            font_size: self.size_px(),
            font_weight: weight,
            white_space_collapse: WhiteSpaceCollapse::Collapse,
            ..TextStyle::default()
        }
    }
}

/// How a label is shortened.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Cut {
    End,
    Middle,
    Url,
    Host,
    /// A window around a byte range that must stay visible.
    Around { start: usize, end: usize },
}

/// What a fitting keeps of a text. Every cut is one of these shapes, so a
/// byte range of the original text can be located in what is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kept {
    /// The whole text: it fits.
    All,
    /// `text[..head]`, `…`, `text[tail..]`: a prefix and a suffix (either
    /// may be empty).
    Ends { head: usize, tail: usize },
    /// `…` unless `start` is 0, `text[start..end]`, `…` unless `end` is the
    /// end of the text: a window inside it.
    Window { start: usize, end: usize },
    /// Not even the ellipsis fits.
    Nothing,
}

impl Kept {
    fn render(self, text: &str) -> Cow<'_, str> {
        match self {
            Kept::All => Cow::Borrowed(text),
            Kept::Ends { head, tail } => {
                let mut out = String::with_capacity(head + ELLIPSIS.len() + text.len() - tail);
                out.push_str(&text[..head]);
                out.push_str(ELLIPSIS);
                out.push_str(&text[tail..]);
                Cow::Owned(out)
            }
            Kept::Window { start, end } => {
                let mut out = String::with_capacity(end - start + 2 * ELLIPSIS.len());
                if start > 0 {
                    out.push_str(ELLIPSIS);
                }
                out.push_str(&text[start..end]);
                if end < text.len() {
                    out.push_str(ELLIPSIS);
                }
                Cow::Owned(out)
            }
            Kept::Nothing => Cow::Borrowed(""),
        }
    }

    /// Where the byte range `mark` of `text` appears in [`render`](Self::render)'s
    /// output, when all of it is shown.
    fn locate(self, text: &str, mark: &Range<usize>) -> Option<Range<usize>> {
        match self {
            Kept::All => Some(mark.clone()),
            Kept::Ends { head, .. } if mark.end <= head => Some(mark.clone()),
            Kept::Ends { head, tail } if mark.start >= tail => {
                let shift = head + ELLIPSIS.len();
                Some(mark.start - tail + shift..mark.end - tail + shift)
            }
            Kept::Window { start, end } if start <= mark.start && mark.end <= end => {
                let lead = if start > 0 { ELLIPSIS.len() } else { 0 };
                Some(mark.start - start + lead..mark.end - start + lead)
            }
            Kept::Window { start, end } if start == mark.start && end < mark.end && end < text.len() => {
                // Only the start of a mark too wide to show whole.
                let lead = if start > 0 { ELLIPSIS.len() } else { 0 };
                Some(lead..end - start + lead)
            }
            _ => None,
        }
    }
}

/// A fitted label and the part of it to highlight.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Marked {
    pub text: String,
    /// A byte range of `text`; `None` when there is nothing to highlight.
    pub mark: Option<Range<usize>>,
}

/// Memoised fittings are dropped once there are this many.
const MEMO_LIMIT: usize = 4_096;

/// Measures and shortens labels exactly as Blitz will lay them out.
pub struct TextFitter {
    fonts: FontContext,
    layouts: LayoutContext<()>,
    scale: f32,
    memo: HashMap<(Font, Cut, u32, String), Kept>,
}

impl Default for TextFitter {
    fn default() -> Self {
        TextFitter::new()
    }
}

impl TextFitter {
    pub fn new() -> TextFitter {
        TextFitter {
            fonts: theme::font_context(),
            layouts: LayoutContext::new(),
            scale: 1.0,
            memo: HashMap::with_capacity(256),
        }
    }

    /// The window's device-pixel scale. Advances are quantized to device
    /// pixels, so widths depend on it; changing it forgets memoised cuts.
    pub fn set_scale(&mut self, scale: f32) {
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        if scale != self.scale {
            self.scale = scale;
            self.memo.clear();
        }
    }

    /// The laid-out width of `text`, in CSS px, without trailing white space.
    pub fn width(&mut self, font: Font, text: &str) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        let style = font.style();
        let mut builder = self
            .layouts
            .tree_builder(&mut self.fonts, self.scale, true, &style);
        builder.push_text(text);
        let (mut layout, _) = builder.build();
        layout.break_all_lines(None);
        layout.width() / self.scale
    }

    /// `text` if it fits in `max_px`; otherwise its longest prefix that fits
    /// together with a trailing `…`.
    pub fn end<'a>(&mut self, font: Font, text: &'a str, max_px: f32) -> Cow<'a, str> {
        self.fit(font, Cut::End, text, max_px).render(text)
    }

    /// `text` if it fits; otherwise `head…tail` with the most characters
    /// that fit, the head taking the extra one. For identifiers such as
    /// wallet addresses, whose two ends are what people compare.
    pub fn middle<'a>(&mut self, font: Font, text: &'a str, max_px: f32) -> Cow<'a, str> {
        self.fit(font, Cut::Middle, text, max_px).render(text)
    }

    /// A URL that fits: `scheme://authority` is kept whole and the rest of
    /// the address is cut from the left, starting at a `/` where that loses
    /// little (`file://…/site/notes.html`). When the authority alone would
    /// take most of the room, the URL is cut in the middle instead.
    pub fn url<'a>(&mut self, font: Font, url: &'a str, max_px: f32) -> Cow<'a, str> {
        self.fit(font, Cut::Url, url, max_px).render(url)
    }

    /// A host name that fits, cut from the left so the registrable domain
    /// survives (`…login.example.com`).
    pub fn host<'a>(&mut self, font: Font, host: &'a str, max_px: f32) -> Cow<'a, str> {
        self.fit(font, Cut::Host, host, max_px).render(host)
    }

    /// [`end`](Self::end), keeping `mark` (a byte range of `text`) in view.
    /// When the end cut would hide any of it, a window around it is shown
    /// instead: `…context mark context…`.
    pub fn end_marked(
        &mut self,
        font: Font,
        text: &str,
        mark: Option<Range<usize>>,
        max_px: f32,
    ) -> Marked {
        self.fit_marked(font, Cut::End, text, mark, max_px)
    }

    /// [`url`](Self::url) unchanged, with `mark` highlighted only where
    /// that cut already shows all of it.
    pub fn url_marked_in_place(
        &mut self,
        font: Font,
        url: &str,
        mark: Option<Range<usize>>,
        max_px: f32,
    ) -> Marked {
        let kept = self.fit(font, Cut::Url, url, max_px);
        let shown = mark
            .filter(|m| m.start < m.end && m.end <= url.len())
            .and_then(|m| kept.locate(url, &m));
        Marked {
            text: kept.render(url).into_owned(),
            mark: shown,
        }
    }

    /// [`url`](Self::url), keeping `mark` in view the same way.
    pub fn url_marked(
        &mut self,
        font: Font,
        url: &str,
        mark: Option<Range<usize>>,
        max_px: f32,
    ) -> Marked {
        self.fit_marked(font, Cut::Url, url, mark, max_px)
    }

    fn fit_marked(
        &mut self,
        font: Font,
        cut: Cut,
        text: &str,
        mark: Option<Range<usize>>,
        max_px: f32,
    ) -> Marked {
        let kept = self.fit(font, cut, text, max_px);
        let mark = mark.filter(|m| {
            m.start < m.end
                && m.end <= text.len()
                && text.is_char_boundary(m.start)
                && text.is_char_boundary(m.end)
        });
        let Some(mark) = mark else {
            return Marked {
                text: kept.render(text).into_owned(),
                mark: None,
            };
        };
        let (kept, shown) = match kept.locate(text, &mark) {
            Some(shown) => (kept, Some(shown)),
            None => {
                let around = Cut::Around {
                    start: mark.start,
                    end: mark.end,
                };
                let window = self.fit(font, around, text, max_px);
                (window, window.locate(text, &mark))
            }
        };
        Marked {
            text: kept.render(text).into_owned(),
            mark: shown,
        }
    }

    fn fit(&mut self, font: Font, cut: Cut, text: &str, max_px: f32) -> Kept {
        if text.is_empty() {
            return Kept::All;
        }
        if max_px <= 0.0 {
            return Kept::Nothing;
        }
        // Tenths of a pixel: finer than any visible difference.
        let key = (font, cut, (max_px * 10.0).round() as u32, text.to_string());
        if let Some(&hit) = self.memo.get(&key) {
            return hit;
        }
        let kept = match self.width(font, text) <= max_px {
            true => Kept::All,
            false => match cut {
                Cut::End => self.cut_end(font, text, max_px),
                Cut::Middle => self.cut_middle(font, text, max_px),
                Cut::Url => self.cut_url(font, text, max_px),
                Cut::Host => self.cut_start(font, text, max_px),
                Cut::Around { start, end } => self.cut_around(font, text, start..end, max_px),
            },
        };
        if self.memo.len() >= MEMO_LIMIT {
            self.memo.clear();
        }
        self.memo.insert(key, kept);
        kept
    }

    /// The largest `k` in `0..=limit` for which `fits(k)` holds, assuming
    /// `fits` is monotone (true up to some point, false after). `None` when
    /// even `fits(0)` fails. The result always fits, even where `fits` is
    /// not quite monotone.
    fn largest_fitting(limit: usize, mut fits: impl FnMut(usize) -> bool) -> Option<usize> {
        if !fits(0) {
            return None;
        }
        let (mut low, mut high) = (0, limit);
        while low < high {
            let mid = low + (high - low).div_ceil(2);
            match fits(mid) {
                true => low = mid,
                false => high = mid - 1,
            }
        }
        Some(low)
    }

    /// The largest `k` for which `candidate(k)` fits, as [`Kept`].
    fn longest(
        &mut self,
        font: Font,
        text: &str,
        max_px: f32,
        limit: usize,
        candidate: impl Fn(usize) -> Kept,
    ) -> Option<Kept> {
        Self::largest_fitting(limit, |k| {
            self.width(font, &candidate(k).render(text)) <= max_px
        })
        .map(candidate)
    }

    fn cut_end(&mut self, font: Font, text: &str, max_px: f32) -> Kept {
        let bounds = char_bounds(text);
        let candidate = |k: usize| Kept::Ends {
            head: text[..bounds[k]].trim_end().len(),
            tail: text.len(),
        };
        self.longest(font, text, max_px, bounds.len() - 1, candidate)
            .unwrap_or(Kept::Nothing)
    }

    fn cut_start(&mut self, font: Font, text: &str, max_px: f32) -> Kept {
        let bounds = char_bounds(text);
        let count = bounds.len() - 1;
        let candidate = |k: usize| {
            let kept = &text[bounds[count - k]..];
            Kept::Ends {
                head: 0,
                tail: text.len() - kept.trim_start().len(),
            }
        };
        self.longest(font, text, max_px, count, candidate)
            .unwrap_or(Kept::Nothing)
    }

    fn cut_middle(&mut self, font: Font, text: &str, max_px: f32) -> Kept {
        let bounds = char_bounds(text);
        let count = bounds.len() - 1;
        let candidate = |k: usize| Kept::Ends {
            head: bounds[k.div_ceil(2)],
            tail: bounds[count - k / 2],
        };
        self.longest(font, text, max_px, count, candidate)
            .unwrap_or(Kept::Nothing)
    }

    fn cut_url(&mut self, font: Font, url: &str, max_px: f32) -> Kept {
        // `scheme://authority` (the authority of `file:///p` is empty).
        let head = url.find("://").map_or(0, |scheme_end| {
            let after = scheme_end + 3;
            url[after..]
                .find(['/', '?', '#'])
                .map_or(url.len(), |rest| after + rest)
        });
        if head == 0 || head == url.len() {
            return self.cut_middle(font, url, max_px);
        }
        let head_px = self.width(font, &format!("{}{ELLIPSIS}", &url[..head]));
        if head_px > max_px * 0.6 {
            return self.cut_middle(font, url, max_px);
        }
        let bounds = char_bounds(&url[head..]);
        let count = bounds.len() - 1;
        let candidate = |k: usize| Kept::Ends {
            head,
            tail: head + bounds[count - k],
        };
        let Some(Kept::Ends { tail, .. }) = self.longest(font, url, max_px, count, candidate)
        else {
            return self.cut_middle(font, url, max_px);
        };
        // Start the kept tail at a path boundary when that drops at most 8
        // characters: `…/site/notes.html` reads better than `…te/notes.html`.
        let kept = &url[tail..];
        match kept.char_indices().find(|(_, c)| *c == '/') {
            Some((slash, _)) if kept[..slash].chars().count() <= 8 => Kept::Ends {
                head,
                tail: tail + slash,
            },
            _ => Kept::Ends { head, tail },
        }
    }

    /// A window of `text` that shows all of `mark` with as much context as
    /// fits, shared evenly between the two sides (the left one taking the
    /// odd character); a side that reaches the end of the text gives its
    /// share to the other. When the mark alone is too wide, its start and
    /// as much of it as fits.
    fn cut_around(&mut self, font: Font, text: &str, mark: Range<usize>, max_px: f32) -> Kept {
        let bounds = char_bounds(text);
        let count = bounds.len() - 1;
        let first = bounds.partition_point(|&at| at < mark.start);
        let last = bounds.partition_point(|&at| at < mark.end);
        let (left_room, right_room) = (first, count - last);
        let candidate = |k: usize| {
            let right = (k / 2).min(right_room);
            let left = (k - right).min(left_room);
            let right = (k - left).min(right_room);
            Kept::Window {
                start: bounds[first - left],
                end: bounds[last + right],
            }
        };
        if let Some(window) = self.longest(font, text, max_px, left_room + right_room, candidate) {
            return window;
        }
        let start = bounds[first];
        let partial = |j: usize| Kept::Window {
            start,
            end: bounds[first + j],
        };
        self.longest(font, text, max_px, last - first, partial)
            .unwrap_or(Kept::Nothing)
    }
}

/// Byte offsets of every character boundary of `text`, including the end.
fn char_bounds(text: &str) -> Vec<usize> {
    let mut bounds = Vec::with_capacity(text.len() + 1);
    bounds.extend(text.char_indices().map(|(at, _)| at));
    bounds.push(text.len());
    bounds
}

#[cfg(test)]
mod tests {
    use super::*;

    const LABEL: Font = Font::new(Face::Sans, 130);
    const DETAIL: Font = Font::new(Face::Mono, 110);

    #[test]
    fn short_text_is_unchanged() {
        let mut fitter = TextFitter::new();
        assert!(matches!(fitter.end(LABEL, "Tabs", 200.0), Cow::Borrowed("Tabs")));
        assert_eq!(fitter.end(LABEL, "", 10.0), "");
        assert_eq!(fitter.end(LABEL, "anything", 0.0), "");
    }

    #[test]
    fn end_cut_is_the_longest_that_fits() {
        let mut fitter = TextFitter::new();
        let title = "A page with a very long title that should be truncated in the sidebar";
        let max = 180.0;
        let short = fitter.end(LABEL, title, max).into_owned();
        assert!(short.ends_with(ELLIPSIS), "{short}");
        assert!(fitter.width(LABEL, &short) <= max);
        // One more character would not fit.
        // One more visible character would not fit (white space before it is
        // trimmed, so it costs nothing and is skipped).
        let kept = short.trim_end_matches(ELLIPSIS).len();
        let (offset, next) = title[kept..]
            .char_indices()
            .find(|(_, c)| !c.is_whitespace())
            .expect("text was cut");
        let one_more = format!("{}{ELLIPSIS}", &title[..kept + offset + next.len_utf8()]);
        assert!(fitter.width(LABEL, &one_more) > max, "{one_more}");
    }

    #[test]
    fn multibyte_text_is_cut_on_character_boundaries() {
        let mut fitter = TextFitter::new();
        let text = "ΑΒΓΔΕΖΗΘΙΚΛΜΝΞΟΠΡΣΤΥΦΧΨΩ ελληνικά γράμματα";
        let short = fitter.end(LABEL, text, 90.0);
        assert!(short.ends_with(ELLIPSIS));
        assert!(fitter.width(LABEL, &short) <= 90.0);
    }

    #[test]
    fn middle_cut_keeps_both_ends() {
        let mut fitter = TextFitter::new();
        let address = "11112eMWq1fS7F8iUcujR3zktD1woWR6ZALP4mW5acQq9xyz";
        let short = fitter.middle(DETAIL, address, 150.0).into_owned();
        let (head, tail) = short.split_once(ELLIPSIS).expect("cut in the middle");
        assert!(address.starts_with(head) && address.ends_with(tail), "{short}");
        assert!(head.chars().count() >= tail.chars().count());
        assert!(fitter.width(DETAIL, &short) <= 150.0);
    }

    #[test]
    fn urls_keep_their_origin_and_a_path_tail() {
        let mut fitter = TextFitter::new();
        let url = "file:///tmp/claude-1000/-home-dylon-Workspace/scratchpad/site/notes.html";
        let short = fitter.url(DETAIL, url, 200.0).into_owned();
        assert!(short.starts_with("file://…/"), "{short}");
        assert!(short.ends_with("notes.html"), "{short}");
        assert!(fitter.width(DETAIL, &short) <= 200.0);

        let url = "https://docs.example.invalid/very/long/path/to/a/page?query=capability";
        let short = fitter.url(DETAIL, url, 330.0).into_owned();
        assert!(short.starts_with("https://docs.example.invalid…"), "{short}");
        assert!(short.ends_with("capability"), "{short}");
        assert!(fitter.width(DETAIL, &short) <= 330.0);
        // When the origin alone would take most of the room, the URL is cut
        // in the middle instead.
        let short = fitter.url(DETAIL, url, 150.0).into_owned();
        assert!(short.starts_with("https://") && short.ends_with("capability"), "{short}");
        assert!(fitter.width(DETAIL, &short) <= 150.0);
    }

    #[test]
    fn hosts_keep_their_registrable_end() {
        let mut fitter = TextFitter::new();
        let host = "a.very.deeply.nested.subdomain.login.example.com";
        let short = fitter.host(LABEL, host, 120.0).into_owned();
        assert!(short.starts_with(ELLIPSIS) && short.ends_with("example.com"), "{short}");
        assert!(fitter.width(LABEL, &short) <= 120.0);
    }

    #[test]
    fn memoised_fits_are_stable() {
        let mut fitter = TextFitter::new();
        let text = "Field notes on gaze and capability, a rather long title";
        let first = fitter.end(LABEL, text, 140.0).into_owned();
        let second = fitter.end(LABEL, text, 140.0).into_owned();
        assert_eq!(first, second);
        fitter.set_scale(2.0);
        let scaled = fitter.end(LABEL, text, 140.0).into_owned();
        assert!(fitter.width(LABEL, &scaled) <= 140.0);
    }

    fn range_of(text: &str, part: &str) -> Range<usize> {
        let at = text.find(part).expect("the part is in the text");
        at..at + part.len()
    }

    fn shown(marked: &Marked) -> &str {
        &marked.text[marked.mark.clone().expect("a visible mark")]
    }

    #[test]
    fn a_mark_the_cut_keeps_is_located_in_place() {
        let mut fitter = TextFitter::new();
        let title = "Field notes on gaze and capability, a rather long title";
        let marked = fitter.end_marked(LABEL, title, Some(range_of(title, "Field")), 140.0);
        assert_eq!(marked.text, fitter.end(LABEL, title, 140.0));
        assert_eq!(marked.mark, Some(0..5));

        // In the kept tail of a URL, the mark moves left past the ellipsis.
        let url = "file:///tmp/claude-1000/-home-dylon-Workspace/scratchpad/site/notes.html";
        let marked = fitter.url_marked(DETAIL, url, Some(range_of(url, "notes")), 200.0);
        assert_eq!(marked.text, fitter.url(DETAIL, url, 200.0));
        assert_eq!(shown(&marked), "notes");
    }

    #[test]
    fn a_mark_the_cut_would_hide_gets_a_window_around_it() {
        let mut fitter = TextFitter::new();
        let title = "Capability-based security - Wikipedia, the free encyclopedia";
        let max = 160.0;
        assert!(!fitter.end(LABEL, title, max).contains("Wiki"));
        let marked = fitter.end_marked(LABEL, title, Some(range_of(title, "Wiki")), max);
        assert_eq!(shown(&marked), "Wiki");
        assert!(marked.text.starts_with(ELLIPSIS) && marked.text.ends_with(ELLIPSIS), "{}", marked.text);
        assert!(fitter.width(LABEL, &marked.text) <= max);

        let url = "file:///tmp/claude-1000/-home-dylon-Workspace-f1r3fly-io-F1R3Gaze/28795e59/scratchpad/f1r3gaze-ui-snapshots/site/prompt.html";
        let gaze = url.rfind("gaze").expect("gaze") ;
        let max = 230.0;
        assert!(!fitter.url(DETAIL, url, max).contains("gaze"));
        let marked = fitter.url_marked(DETAIL, url, Some(gaze..gaze + 4), max);
        assert_eq!(shown(&marked), "gaze");
        assert!(fitter.width(DETAIL, &marked.text) <= max);
        // Context on both sides, the even split shifting only at the ends.
        let mark = marked.mark.clone().expect("mark");
        let before = marked.text[..mark.start].chars().count();
        let after = marked.text[mark.end..].chars().count();
        assert!(before.abs_diff(after) <= 1, "{}", marked.text);

        // At the start of the text the window needs no leading ellipsis.
        let marked = fitter.url_marked(DETAIL, url, Some(0..4), 100.0);
        assert_eq!(shown(&marked), "file");
    }

    #[test]
    fn a_mark_wider_than_the_room_shows_its_start() {
        let mut fitter = TextFitter::new();
        let text = "prefix words then an extraordinarily-long-matched-segment and more";
        let mark = range_of(text, "extraordinarily-long-matched-segment");
        let max = 120.0;
        let marked = fitter.end_marked(LABEL, text, Some(mark.clone()), max);
        assert!(fitter.width(LABEL, &marked.text) <= max);
        let visible = shown(&marked);
        assert!(text[mark].starts_with(visible) && !visible.is_empty(), "{}", marked.text);
    }

    #[test]
    fn unusable_marks_are_ignored() {
        let mut fitter = TextFitter::new();
        let text = "ελληνικά γράμματα and more text to cut";
        for mark in [None, Some(1..2), Some(3..3), Some(0..999)] {
            let marked = fitter.end_marked(LABEL, text, mark, 90.0);
            assert_eq!(marked.text, fitter.end(LABEL, text, 90.0));
            assert_eq!(marked.mark, None);
        }
    }

    #[test]
    fn marked_fits_always_fit_and_point_at_the_mark() {
        let mut fitter = TextFitter::new();
        let samples = [
            ("A page with a very long title that should be truncated in the sidebar", "truncated"),
            ("https://docs.example.invalid/very/long/path/to/a/page?query=capability", "page"),
            ("https://docs.example.invalid/very/long/path/to/a/page?query=capability", "docs"),
            ("ΑΒΓΔΕΖΗΘΙΚΛΜΝΞΟΠΡΣΤΥΦΧΨΩ ελληνικά γράμματα", "ελληνικά"),
        ];
        for (text, part) in samples {
            let mark = range_of(text, part);
            for tenth in (300..=4_000).step_by(170) {
                let max = tenth as f32 / 10.0;
                let end = fitter.end_marked(LABEL, text, Some(mark.clone()), max);
                let url = fitter.url_marked(DETAIL, text, Some(mark.clone()), max);
                for (font, marked) in [(LABEL, end), (DETAIL, url)] {
                    assert!(fitter.width(font, &marked.text) <= max, "{max}: {}", marked.text);
                    if let Some(r) = marked.mark.clone() {
                        assert!(part.starts_with(&marked.text[r]), "{max}: {}", marked.text);
                    }
                }
            }
        }
    }

    /// The measurement must agree with the width Blitz gives the same text
    /// in the same style; otherwise "exact" cuts would not be exact. Blitz
    /// rounds box sizes to whole pixels, hence the 1 px tolerance (budgets
    /// in the chrome keep a 1 px margin for the same reason).
    #[test]
    fn measured_width_matches_blitz_layout() {
        let samples = [
            (LABEL, "font:13px 'Noto Sans',system-ui,sans-serif", "Field notes on gaze and capability"),
            (Font::new(Face::SansSemibold, 140), "font:600 14px 'Noto Sans',system-ui,sans-serif", "Recently closed"),
            (DETAIL, "font:11px 'Fira Code',monospace", "file:///tmp/site/notes.html"),
        ];
        let mut fitter = TextFitter::new();
        for (font, css, text) in samples {
            let html = format!(
                "<html><body style='margin:0'><span id='probe' style=\"display:inline-block;white-space:nowrap;{css}\">{text}</span></body></html>"
            );
            let mut doc = gaze_dom_blitz::parse_html(
                &html,
                blitz_dom::DocumentConfig {
                    font_ctx: Some(theme::font_context()),
                    ..Default::default()
                },
            );
            doc.viewport_mut().window_size = (1200, 200);
            doc.resolve(0.0);
            let probe = doc.get_element_by_id("probe").expect("probe element");
            let blitz_width = f64::from(doc.get_node(probe).expect("probe node").final_layout().size.width);
            let ours = f64::from(fitter.width(font, text));
            assert!(
                (blitz_width - ours).abs() <= 1.0,
                "{text}: Blitz lays it out {blitz_width} px wide, the fitter measured {ours} px"
            );
        }
    }
}
