//! A small Markdown subset for free text boxes, notes and todos. Every source line is one block (no wrapping, no
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
    m.chars()
        .enumerate()
        .all(|(i, mc)| c.get(k + i) == Some(&mc))
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

/// How one source char is shown when Markdown is rendered in place (notes, todos, details):
/// markers are hidden, the rest keeps its position in the text so wrapping, the caret and
/// selections are unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cs {
    /// A marker: not drawn.
    pub hidden: bool,
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    /// Heading level (0 = none).
    pub heading: u8,
    pub quote: bool,
    /// Text of a done task.
    pub done: bool,
    /// A list number: drawn dimmed.
    pub dim: bool,
    /// The first char of a block prefix: draw the block's symbol here (bullet, box, bar, rule).
    pub prefix: Option<Block>,
}

impl Cs {
    /// Same look as `o` (so both can be drawn in one run).
    pub fn same_run(&self, o: &Cs) -> bool {
        (
            self.bold,
            self.italic,
            self.strike,
            self.code,
            self.heading,
            self.quote,
            self.done,
            self.dim,
        ) == (
            o.bold, o.italic, o.strike, o.code, o.heading, o.quote, o.done, o.dim,
        )
    }
}

/// Inline styles of each char of one line.
fn inline_cs(c: &[char]) -> Vec<Cs> {
    let n = c.len();
    let mut out = vec![Cs::default(); n];
    let mut cur = Cs::default();
    let mut k = 0;
    'outer: while k < n {
        let ch = c[k];
        if ch == '\\' && k + 1 < n && c[k + 1].is_ascii_punctuation() {
            out[k] = Cs {
                hidden: true,
                ..cur
            };
            out[k + 1] = cur;
            k += 2;
            continue;
        }
        if ch == '`' {
            if let Some(e) = c[k + 1..].iter().position(|x| *x == '`').map(|e| e + k + 1) {
                let code = Cs { code: true, ..cur };
                out[k] = Cs {
                    hidden: true,
                    ..code
                };
                for o in &mut out[k + 1..e] {
                    *o = code;
                }
                out[e] = out[k];
                k = e + 1;
                continue;
            }
        }
        let hide = |out: &mut Vec<Cs>, k: usize, len: usize, cur: Cs| {
            for o in &mut out[k..k + len] {
                *o = Cs {
                    hidden: true,
                    ..cur
                };
            }
        };
        if starts(c, k, "***") {
            // Bold and italic together.
            let both = cur.bold && cur.italic;
            if both || (!cur.bold && !cur.italic && find(c, k + 4, "***").is_some()) {
                hide(&mut out, k, 3, cur);
                (cur.bold, cur.italic) = (!both, !both);
                k += 3;
                continue;
            }
        }
        for m in ["**", "__", "~~", "*", "_"] {
            if !starts(c, k, m) {
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
                && find(c, k + len + 1, m).is_some();
            if on || can_open {
                hide(&mut out, k, len, cur);
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
        out[k] = cur;
        k += 1;
    }
    out
}

/// Split a line into styled spans.
pub fn inline(s: &str) -> Vec<Span> {
    let c: Vec<char> = s.chars().collect();
    let cs = inline_cs(&c);
    let mut out: Vec<Span> = Vec::new();
    let mut last: Option<Cs> = None;
    for (ch, st) in c.iter().zip(&cs) {
        if st.hidden {
            continue;
        }
        match (out.last_mut(), last) {
            (Some(sp), Some(l)) if l.same_run(st) => sp.text.push(*ch),
            _ => out.push(Span {
                text: ch.to_string(),
                bold: st.bold,
                italic: st.italic,
                strike: st.strike,
                code: st.code,
            }),
        }
        last = Some(*st);
    }
    out
}

/// The block a line starts with: (block, chars of indent, chars of its prefix).
pub fn block_prefix(line: &[char]) -> (Block, usize, usize) {
    let lead = line.iter().take_while(|c| **c == ' ').count();
    let rest = &line[lead..];
    let compact: Vec<char> = rest.iter().copied().filter(|c| *c != ' ').collect();
    if compact.len() >= 3
        && ['-', '*', '_']
            .iter()
            .any(|m| compact.iter().all(|c| c == m))
    {
        return (Block::Rule, 0, line.len());
    }
    let hashes = rest.iter().take_while(|c| **c == '#').count();
    if (1..=6).contains(&hashes) && rest.get(hashes) == Some(&' ') {
        return (Block::Heading(hashes.min(3) as u8), lead, hashes + 1);
    }
    if rest.len() >= 2 && matches!(rest[0], '-' | '*' | '+') && rest[1] == ' ' {
        let r = &rest[2..];
        if r.len() >= 3 && r[0] == '[' && r[2] == ']' && matches!(r[1], ' ' | 'x' | 'X') {
            let done = r[1] != ' ';
            if r.len() == 3 {
                return (Block::Task(done), lead, 5);
            }
            if r[3] == ' ' {
                return (Block::Task(done), lead, 6);
            }
        }
        return (Block::Bullet, lead, 2);
    }
    let digits = rest.iter().take_while(|c| c.is_ascii_digit()).count();
    if (1..=4).contains(&digits)
        && matches!(rest.get(digits), Some('.') | Some(')'))
        && rest.get(digits + 1) == Some(&' ')
    {
        return (Block::Number, lead, digits + 2);
    }
    if rest.first() == Some(&'>') {
        let l = if rest.get(1) == Some(&' ') { 2 } else { 1 };
        return (Block::Quote, lead, l);
    }
    (Block::Para, 0, 0)
}

/// Per-char styles of a whole text (one entry per char, newlines included). `blocks` also reads
/// line prefixes (headings, lists, tasks, quotes, rules); single-line fields pass false.
pub fn char_styles(text: &str, blocks: bool) -> Vec<Cs> {
    let c: Vec<char> = text.chars().collect();
    let n = c.len();
    let mut out = Vec::with_capacity(n);
    let mut start = 0;
    loop {
        let end = c[start..]
            .iter()
            .position(|x| *x == '\n')
            .map_or(n, |p| start + p);
        let line = &c[start..end];
        let (block, ps, pl) = if blocks {
            block_prefix(line)
        } else {
            (Block::Para, 0, 0)
        };
        let mut ls = vec![Cs::default(); line.len()];
        match block {
            Block::Rule => {
                for x in &mut ls {
                    x.hidden = true;
                }
                if let Some(x) = ls.first_mut() {
                    x.prefix = Some(Block::Rule);
                }
            }
            Block::Para => {}
            Block::Number => {
                for x in &mut ls[ps..ps + pl] {
                    x.dim = true;
                }
            }
            _ => {
                for x in &mut ls[ps..ps + pl] {
                    x.hidden = true;
                }
                ls[ps].prefix = Some(block);
            }
        }
        if block != Block::Rule {
            for (i, mut x) in inline_cs(&line[ps + pl..]).into_iter().enumerate() {
                x.heading = if let Block::Heading(h) = block { h } else { 0 };
                x.quote = block == Block::Quote;
                x.done = block == Block::Task(true);
                ls[ps + pl + i] = x;
            }
        }
        out.extend(ls);
        if end >= n {
            break;
        }
        out.push(Cs::default());
        start = end + 1;
    }
    out
}

/// True if rendering `text` as Markdown would look different from the raw text.
pub fn has_markup(text: &str, blocks: bool) -> bool {
    char_styles(text, blocks)
        .iter()
        .any(|c| *c != Cs::default())
}

/// How many times `ch` repeats going left from `at` (exclusive) / right from `at` (inclusive).
fn run_left(c: &[char], at: usize, ch: char) -> usize {
    c[..at].iter().rev().take_while(|x| **x == ch).count()
}
fn run_right(c: &[char], at: usize, ch: char) -> usize {
    c[at..].iter().take_while(|x| **x == ch).count()
}

/// Does a run of `k` marker chars contain marker `m` (`*` needs an odd run: `*` or `***`)?
fn run_has(k: usize, m: &str) -> bool {
    match m {
        "*" => k % 2 == 1,
        "**" => k >= 2,
        _ => k >= m.chars().count(),
    }
}

/// Toggle an inline marker (`**`, `*`, `~~`, `` ` ``) around chars `[a, b)`. Returns the new
/// text and the new range of the same inner text. With `a == b`, inserts an empty pair and puts
/// the range (the caret) between them.
pub fn toggle_wrap(text: &str, a: usize, b: usize, m: &str) -> (String, usize, usize) {
    let mut c: Vec<char> = text.chars().collect();
    let (a, b) = (a.min(b).min(c.len()), a.max(b).min(c.len()));
    let ml = m.chars().count();
    let mc = m.chars().next().unwrap_or('*');
    // Already wrapped just outside the range: unwrap.
    if a >= ml && run_has(run_left(&c, a, mc), m) && run_has(run_right(&c, b, mc), m) && b > a {
        c.drain(b..b + ml);
        c.drain(a - ml..a);
        return (c.into_iter().collect(), a - ml, b - ml);
    }
    // The range includes the markers: unwrap.
    if b - a > 2 * ml && run_has(run_right(&c, a, mc), m) && run_has(run_left(&c, b, mc), m) {
        c.drain(b - ml..b);
        c.drain(a..a + ml);
        return (c.into_iter().collect(), a, b - 2 * ml);
    }
    // Wrap, keeping spaces at the edges of the selection outside the markers.
    let (mut a2, mut b2) = (a, b);
    while a2 < b2 && c[a2] == ' ' {
        a2 += 1;
    }
    while b2 > a2 && c[b2 - 1] == ' ' {
        b2 -= 1;
    }
    if a < b && a2 == b2 {
        return (text.to_string(), a, b);
    }
    let mut out: Vec<char> = c[..a2].to_vec();
    out.extend(m.chars());
    out.extend_from_slice(&c[a2..b2]);
    out.extend(m.chars());
    out.extend_from_slice(&c[b2..]);
    (out.into_iter().collect(), a2 + ml, b2 + ml)
}

/// The word around `caret` (`caret` strictly inside it), as a char range.
pub fn word_at(text: &str, caret: usize) -> Option<(usize, usize)> {
    let c: Vec<char> = text.chars().collect();
    let w = |i: usize| c.get(i).is_some_and(|x| x.is_alphanumeric());
    if caret == 0 || !w(caret - 1) || !w(caret) {
        return None;
    }
    let mut a = caret;
    while a > 0 && w(a - 1) {
        a -= 1;
    }
    let mut b = caret;
    while w(b) {
        b += 1;
    }
    Some((a, b))
}

/// A Markdown edit from the format bar or a shortcut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MdOp {
    /// Toggle an inline marker around the selection.
    Wrap(&'static str),
    /// Toggle a line prefix on the lines of the selection.
    Line(LineKind),
}

/// Result of `apply`: new text, new selection, new caret.
pub type Applied = (String, Option<(usize, usize)>, usize);

/// Apply `op` to `text` with selection `sel` (None: just a caret at `caret`). Returns the new
/// text, the new selection (if there was one) and the new caret. `multiline` allows line ops.
pub fn apply(
    text: &str,
    sel: Option<(usize, usize)>,
    caret: usize,
    op: MdOp,
    multiline: bool,
) -> Option<Applied> {
    let (t, na, nb, new_caret) = match op {
        MdOp::Wrap(m) => {
            let (a, b) = sel
                .or_else(|| word_at(text, caret))
                .unwrap_or((caret, caret));
            let (t, na, nb) = toggle_wrap(text, a, b, m);
            let nc = if sel.is_some() {
                nb
            } else {
                (caret + na).saturating_sub(a)
            };
            (t, na, nb, nc)
        }
        MdOp::Line(k) => {
            if !multiline {
                return None;
            }
            let (a, b) = sel.unwrap_or((caret, caret));
            let (t, na, nb) = toggle_prefix(text, a, b, k);
            (t, na, nb, nb)
        }
    };
    if t == text {
        return None;
    }
    Some((t, sel.map(|_| (na, nb)), new_caret))
}

/// A line-level format: heading (cycles `#`, `##`, `###`, none), bullet, task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Heading,
    Bullet,
    Task,
}

/// Toggle a line prefix on every line touched by `[a, b]`. Returns the new text and range.
pub fn toggle_prefix(text: &str, a: usize, b: usize, kind: LineKind) -> (String, usize, usize) {
    let c: Vec<char> = text.chars().collect();
    let (a, b) = (a.min(b).min(c.len()), a.max(b).min(c.len()));
    // Line starts.
    let mut starts_at = vec![0];
    for (i, ch) in c.iter().enumerate() {
        if *ch == '\n' {
            starts_at.push(i + 1);
        }
    }
    let line_end = |s: usize| {
        c[s..]
            .iter()
            .position(|x| *x == '\n')
            .map_or(c.len(), |p| s + p)
    };
    let touched: Vec<usize> = starts_at
        .iter()
        .copied()
        .filter(|&s| line_end(s) >= a && s <= b)
        .collect();
    let Some(&first) = touched.first() else {
        return (text.to_string(), a, b);
    };
    // The new prefix is decided by the first line.
    let (fb, _, _) = block_prefix(&c[first..line_end(first)]);
    let new: &str = match (kind, fb) {
        (LineKind::Heading, Block::Heading(1)) => "## ",
        (LineKind::Heading, Block::Heading(2)) => "### ",
        (LineKind::Heading, Block::Heading(_)) => "",
        (LineKind::Heading, _) => "# ",
        (LineKind::Bullet, Block::Bullet) => "",
        (LineKind::Bullet, _) => "- ",
        (LineKind::Task, Block::Task(_)) => "",
        (LineKind::Task, _) => "- [ ] ",
    };
    let nl = new.chars().count();
    // Each edit replaces source chars [p0, p1) with `new`.
    let mut edits: Vec<(usize, usize)> = Vec::new();
    for s in touched {
        let e = line_end(s);
        let (blk, ps, pl) = block_prefix(&c[s..e]);
        let (ps, pl) = if matches!(blk, Block::Rule | Block::Para) {
            (0, 0)
        } else {
            (ps, pl)
        };
        edits.push((s + ps, s + ps + pl));
    }
    let mut out: Vec<char> = Vec::with_capacity(c.len() + 8);
    let mut prev = 0;
    for &(p0, p1) in &edits {
        out.extend_from_slice(&c[prev..p0]);
        out.extend(new.chars());
        prev = p1;
    }
    out.extend_from_slice(&c[prev..]);
    let map = |x: usize| -> usize {
        let mut delta = 0isize;
        for &(p0, p1) in &edits {
            if x < p0 {
                break;
            }
            if x < p1 {
                return (p0 as isize + delta) as usize + nl;
            }
            delta += nl as isize - (p1 - p0) as isize;
        }
        (x as isize + delta) as usize
    };
    (out.into_iter().collect(), map(a), map(b))
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
pub fn layout(
    text: &str,
    size: TextSize,
    bold: bool,
    adv: &dyn Fn(Style, char) -> f32,
) -> MdLayout {
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
        assert_eq!(s, vec![sp("a "), b, sp(" "), c, sp(" "), d, sp(" "), e]);
        // Unclosed or spaced markers stay literal; snake_case is not italic; escapes work.
        assert_eq!(inline("2 * 3 = 6"), vec![sp("2 * 3 = 6")]);
        assert_eq!(inline("**open"), vec![sp("**open")]);
        assert_eq!(inline("snake_case_name"), vec![sp("snake_case_name")]);
        assert_eq!(inline(r"\*not\*"), vec![sp("*not*")]);
        // Nested: bold italic.
        let bi = inline("***x***");
        assert!(
            bi.iter().any(|s| s.text == "x" && s.bold && s.italic),
            "{bi:?}"
        );
    }

    #[test]
    fn char_styles_hide_markers_and_keep_positions() {
        let t = "# Hi\n- **b** x\n1. n";
        let cs = char_styles(t, true);
        assert_eq!(cs.len(), t.chars().count());
        let shown: String = t
            .chars()
            .zip(&cs)
            .filter(|(_, c)| !c.hidden)
            .map(|(ch, _)| ch)
            .collect();
        assert_eq!(shown, "Hi\nb x\n1. n");
        assert_eq!(cs[0].prefix, Some(Block::Heading(1)));
        assert_eq!(cs[2].heading, 1);
        assert_eq!(cs[5].prefix, Some(Block::Bullet));
        assert!(cs[9].bold);
        assert!(cs[15].dim);
        // Single-line fields ignore block prefixes.
        assert!(!char_styles("# not a heading", false)[0].hidden);
        assert!(!has_markup("plain words", true) && has_markup("a *b*", false));
    }

    #[test]
    fn wrapping_a_selection_toggles_markers() {
        assert_eq!(
            toggle_wrap("make bold", 5, 9, "**"),
            ("make **bold**".into(), 7, 11)
        );
        assert_eq!(
            toggle_wrap("make **bold**", 7, 11, "**"),
            ("make bold".into(), 5, 9)
        );
        assert_eq!(
            toggle_wrap("make **bold**", 5, 13, "**"),
            ("make bold".into(), 5, 9)
        );
        // Italic inside bold is added, not mistaken for the bold markers.
        assert_eq!(toggle_wrap("**b**", 2, 3, "*"), ("***b***".into(), 3, 4));
        assert_eq!(toggle_wrap("***b***", 3, 4, "*"), ("**b**".into(), 2, 3));
        // Edge spaces stay outside; an empty range inserts a pair around the caret.
        assert_eq!(toggle_wrap("a b c", 1, 4, "~~"), ("a ~~b~~ c".into(), 4, 5));
        assert_eq!(toggle_wrap("ab", 1, 1, "`"), ("a``b".into(), 2, 2));
    }

    #[test]
    fn line_prefixes_toggle_and_cycle() {
        assert_eq!(
            toggle_prefix("one\ntwo", 5, 5, LineKind::Bullet),
            ("one\n- two".into(), 7, 7)
        );
        assert_eq!(
            toggle_prefix("one\n- two", 7, 7, LineKind::Bullet),
            ("one\ntwo".into(), 5, 5)
        );
        assert_eq!(
            toggle_prefix("a\nb", 0, 3, LineKind::Task),
            ("- [ ] a\n- [ ] b".into(), 6, 15)
        );
        let (t, _, _) = toggle_prefix("x", 0, 0, LineKind::Heading);
        assert_eq!(t, "# x");
        let (t, _, _) = toggle_prefix(&t, 0, 0, LineKind::Heading);
        assert_eq!(t, "## x");
        let (t, _, _) = toggle_prefix("### x", 0, 0, LineKind::Heading);
        assert_eq!(t, "x");
        // A bullet becomes a task in place.
        assert_eq!(
            toggle_prefix("- x", 3, 3, LineKind::Task),
            ("- [ ] x".into(), 7, 7)
        );
    }

    #[test]
    fn apply_wraps_the_word_under_the_caret_without_a_selection() {
        let (t, sel, c) = apply("say hello now", None, 6, MdOp::Wrap("**"), false).unwrap();
        assert_eq!((t.as_str(), sel, c), ("say **hello** now", None, 8));
        let (t, _, c) = apply("ab ", None, 3, MdOp::Wrap("*"), false).unwrap();
        assert_eq!((t.as_str(), c), ("ab **", 4));
        let (t, sel, _) = apply("x y", Some((2, 3)), 3, MdOp::Wrap("~~"), false).unwrap();
        assert_eq!((t.as_str(), sel), ("x ~~y~~", Some((4, 5))));
        assert!(apply("x", None, 0, MdOp::Line(LineKind::Bullet), false).is_none());
        let (t, _, c) = apply("x", None, 1, MdOp::Line(LineKind::Bullet), true).unwrap();
        assert_eq!((t.as_str(), c), ("- x", 3));
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
