//! A small Markdown subset for free text boxes. Every source line is one block (no wrapping, no
//! multi-line constructs), so the rendered text maps line for line onto what you typed.
//!
//! Blocks: `#`, `##`, `###` headings, `-`/`*`/`+` bullets, `1.` numbered items, `- [ ]` and
//! `- [x]` tasks, `>` quotes, `---` rules; two leading spaces per nesting level.
//! Inline: `**bold**`, `__bold__`, `*italic*`, `_italic_`, `~~strike~~`, `` `code` ``, `\*`.

use crate::board::TextSize;
use crate::text::Style;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Block {
    Para,
    /// Level 1 to 3.
    Heading(u8),
    Bullet,
    /// A numbered item; its marker (`1.`) is in `MdLine::marker`.
    Number,
    /// A task, done or not.
    Task(bool),
    Quote,
    Rule,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MdLine {
    pub block: Block,
    /// Nesting level (two leading spaces each), for lists and quotes.
    pub indent: usize,
    pub marker: String,
    pub spans: Vec<Span>,
}

/// Parse one source line.
pub fn parse_line(line: &str) -> MdLine {
    let lead = line.chars().take_while(|c| *c == ' ').count();
    let rest = &line[lead..];
    let indent = (lead / 2).min(4);
    let mk = |block, rest: &str| MdLine {
        block,
        indent,
        marker: String::new(),
        spans: inline(rest),
    };
    let compact: String = rest.chars().filter(|c| *c != ' ').collect();
    if compact.len() >= 3
        && ["-", "*", "_"]
            .iter()
            .any(|m| compact.chars().all(|c| c.to_string() == *m))
    {
        return mk(Block::Rule, "");
    }
    let hashes = rest.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) && rest[hashes..].starts_with(' ') {
        let mut l = mk(Block::Heading(hashes.min(3) as u8), rest[hashes..].trim());
        l.indent = 0;
        return l;
    }
    for b in ["- ", "* ", "+ "] {
        if let Some(r) = rest.strip_prefix(b) {
            for (t, done) in [("[ ] ", false), ("[x] ", true), ("[X] ", true)] {
                if let Some(r) = r.strip_prefix(t) {
                    return mk(Block::Task(done), r);
                }
            }
            if r == "[ ]" || r == "[x]" || r == "[X]" {
                return mk(Block::Task(r != "[ ]"), "");
            }
            return mk(Block::Bullet, r);
        }
    }
    let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
    if (1..=4).contains(&digits) {
        let after = &rest[digits..];
        if after.starts_with(". ") || after.starts_with(") ") {
            let mut l = mk(Block::Number, &after[2..]);
            l.marker = rest[..digits + 1].to_string();
            return l;
        }
    }
    if let Some(r) = rest.strip_prefix('>') {
        return mk(Block::Quote, r.strip_prefix(' ').unwrap_or(r));
    }
    MdLine {
        block: Block::Para,
        indent: 0,
        marker: String::new(),
        spans: inline(line),
    }
}

fn starts(c: &[char], k: usize, m: &str) -> bool {
    m.chars().enumerate().all(|(i, mc)| c.get(k + i) == Some(&mc))
}

/// Index of the next `m` at or after `from` (a single-char marker must not be part of a pair).
fn find(c: &[char], from: usize, m: &str) -> Option<usize> {
    let single = m.chars().count() == 1;
    let mut k = from;
    while k < c.len() {
        if c[k] == '\\' {
            k += 2;
            continue;
        }
        if starts(c, k, m) {
            if !single {
                return Some(k);
            }
            let mc = c[k];
            let paired = c.get(k + 1) == Some(&mc) || (k > from && c[k - 1] == mc);
            if !paired {
                return Some(k);
            }
            k += 2;
            continue;
        }
        k += 1;
    }
    None
}

/// Split a line into styled spans.
pub fn inline(s: &str) -> Vec<Span> {
    let c: Vec<char> = s.chars().collect();
    let n = c.len();
    let mut out: Vec<Span> = Vec::new();
    let mut cur = Span::default();
    let push = |out: &mut Vec<Span>, cur: &mut Span| {
        if !cur.text.is_empty() {
            out.push(cur.clone());
            cur.text.clear();
        }
    };
    let mut k = 0;
    'outer: while k < n {
        let ch = c[k];
        if ch == '\\' && k + 1 < n && c[k + 1].is_ascii_punctuation() {
            cur.text.push(c[k + 1]);
            k += 2;
            continue;
        }
        if ch == '`' {
            if let Some(e) = c[k + 1..].iter().position(|x| *x == '`').map(|e| e + k + 1) {
                push(&mut out, &mut cur);
                out.push(Span {
                    text: c[k + 1..e].iter().collect(),
                    code: true,
                    ..cur.clone()
                });
                k = e + 1;
                continue;
            }
        }
        if starts(&c, k, "***") {
            // Bold and italic together.
            let both = cur.bold && cur.italic;
            if both || (!cur.bold && !cur.italic && find(&c, k + 4, "***").is_some()) {
                push(&mut out, &mut cur);
                (cur.bold, cur.italic) = (!both, !both);
                k += 3;
                continue;
            }
        }
        for m in ["**", "__", "~~", "*", "_"] {
            if !starts(&c, k, m) {
                continue;
            }
            let len = m.chars().count();
            let on = match m {
                "**" | "__" => cur.bold,
                "~~" => cur.strike,
                _ => cur.italic,
            };
            let prev_word = k > 0 && c[k - 1].is_alphanumeric();
            let next_space = c.get(k + len).is_none_or(|x| x.is_whitespace());
            let can_open = !next_space
                && !(m.starts_with('_') && prev_word)
                && find(&c, k + len + 1, m).is_some();
            if on || can_open {
                push(&mut out, &mut cur);
                match m {
                    "**" | "__" => cur.bold = !on,
                    "~~" => cur.strike = !on,
                    _ => cur.italic = !on,
                }
                k += len;
                continue 'outer;
            }
            break;
        }
        cur.text.push(ch);
        k += 1;
    }
    push(&mut out, &mut cur);
    out
}

/// A piece of a laid-out line in one style. `x` and `w` are relative to the box, world units.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub x: f32,
    pub w: f32,
    pub text: String,
    pub style: Style,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LaidLine {
    /// Top of the line relative to the box, and its height.
    pub y: f32,
    pub h: f32,
    pub block: Block,
    /// Where the bullet / number / box / quote bar starts.
    pub prefix_x: f32,
    pub marker: String,
    pub size: TextSize,
    pub runs: Vec<Run>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MdLayout {
    pub lines: Vec<LaidLine>,
    pub w: f32,
    pub h: f32,
}

/// The size a block is drawn at, for a box of `base` size: `#` is two steps up, `##` one.
pub fn block_size(block: Block, base: TextSize) -> TextSize {
    match block {
        Block::Heading(1) => base.step(true).step(true),
        Block::Heading(2) => base.step(true),
        _ => base,
    }
}

/// Lay out `text` as Markdown for a box of `size` (`bold` makes everything bold).
pub fn layout(text: &str, size: TextSize, bold: bool, adv: &dyn Fn(Style, char) -> f32) -> MdLayout {
    let mut lines = Vec::new();
    let (mut y, mut w) = (0.0_f32, 0.0_f32);
    for src in text.split('\n') {
        let md = parse_line(src);
        let ls = block_size(md.block, size);
        let px = ls.px();
        let h = if md.block == Block::Rule {
            px * 0.9
        } else {
            px * 1.4
        };
        let prefix_x = md.indent as f32 * px * 1.2;
        let heading = matches!(md.block, Block::Heading(_));
        let pre = match md.block {
            Block::Bullet | Block::Task(_) => px * 1.3,
            Block::Number => {
                let mw: f32 = md
                    .marker
                    .chars()
                    .map(|c| adv(Style::Free(ls, bold), c))
                    .sum();
                (mw + px * 0.4).max(px * 1.3)
            }
            Block::Quote => px * 0.8,
            _ => 0.0,
        };
        let mut x = prefix_x + pre;
        let mut runs = Vec::new();
        for sp in md.spans {
            let style = Style::Free(ls, bold || heading || sp.bold);
            let tw: f32 = sp.text.chars().map(|c| adv(style, c)).sum();
            let pad = if sp.code { px * 0.25 } else { 0.0 };
            runs.push(Run {
                x: x + pad,
                w: tw,
                text: sp.text,
                style,
                italic: sp.italic,
                strike: sp.strike,
                code: sp.code,
            });
            x += tw + 2.0 * pad;
        }
        w = w.max(x);
        lines.push(LaidLine {
            y,
            h,
            block: md.block,
            prefix_x,
            marker: md.marker,
            size: ls,
            runs,
        });
        y += h;
    }
    MdLayout { lines, w, h: y }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sp(text: &str) -> Span {
        Span {
            text: text.into(),
            ..Span::default()
        }
    }

    #[test]
    fn blocks() {
        assert_eq!(parse_line("# Big").block, Block::Heading(1));
        assert_eq!(parse_line("### Small").block, Block::Heading(3));
        assert_eq!(parse_line("#### Smaller").block, Block::Heading(3));
        assert_eq!(parse_line("#nospace").block, Block::Para);
        assert_eq!(parse_line("- item").block, Block::Bullet);
        assert_eq!(parse_line("  * nested").indent, 1);
        assert_eq!(parse_line("- [ ] todo").block, Block::Task(false));
        assert_eq!(parse_line("- [x] done").block, Block::Task(true));
        let n = parse_line("12. twelve");
        assert_eq!((n.block, n.marker.as_str()), (Block::Number, "12."));
        assert_eq!(parse_line("> said").block, Block::Quote);
        assert_eq!(parse_line("---").block, Block::Rule);
        assert_eq!(parse_line("* * *").block, Block::Rule);
        assert_eq!(parse_line("--").block, Block::Para);
        assert_eq!(parse_line("# Big").spans, vec![sp("Big")]);
    }

    #[test]
    fn inline_styles() {
        let s = inline("a **b** *c* ~~d~~ `e*f`");
        let b = Span {
            bold: true,
            ..sp("b")
        };
        let c = Span {
            italic: true,
            ..sp("c")
        };
        let d = Span {
            strike: true,
            ..sp("d")
        };
        let e = Span {
            code: true,
            ..sp("e*f")
        };
        assert_eq!(
            s,
            vec![sp("a "), b, sp(" "), c, sp(" "), d, sp(" "), e]
        );
        // Unclosed or spaced markers stay literal; snake_case is not italic; escapes work.
        assert_eq!(inline("2 * 3 = 6"), vec![sp("2 * 3 = 6")]);
        assert_eq!(inline("**open"), vec![sp("**open")]);
        assert_eq!(inline("snake_case_name"), vec![sp("snake_case_name")]);
        assert_eq!(inline(r"\*not\*"), vec![sp("*not*")]);
        // Nested: bold italic.
        let bi = inline("***x***");
        assert!(bi.iter().any(|s| s.text == "x" && s.bold && s.italic), "{bi:?}");
    }

    #[test]
    fn layout_sizes_and_indents() {
        let adv = |s: Style, _: char| s.size() * 0.5;
        let l = layout("# T\nbody\n- b", TextSize::Medium, false, &adv);
        assert_eq!(l.lines.len(), 3);
        assert_eq!(l.lines[0].size, TextSize::Title);
        assert_eq!(l.lines[0].runs[0].style, Style::Free(TextSize::Title, true));
        assert_eq!(l.lines[1].y, l.lines[0].h);
        assert!(l.lines[2].runs[0].x > 0.0, "bullet text is indented");
        assert!((l.h - l.lines.iter().map(|x| x.h).sum::<f32>()).abs() < 1e-3);
    }
}
