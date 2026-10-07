//! Slim template compiler.
//!
//! Slim is indentation-nested like HAML, so this reuses [`crate::haml`]'s
//! `Compiler` (the `_buf`-append emitter, indentation stack and segment
//! map) and only adds Slim's line grammar. The result flows through the
//! shared `ingest_template` view pipeline unchanged.
//!
//! Subset: `tag.class#id` (and implicit `div`), attributes (`a="s"`,
//! `a=ruby`, wrapped `(…)`/`[…]`/`{…}` with bare boolean names) routed
//! through the runtime `render_attrs` helper, inline text, `tag: child`
//! nesting, `=`/`==` output, `-` code (blocks close by indentation),
//! `|`/`'` text, `/` comments, `/!` HTML comments, `doctype html`, `#{}`
//! interpolation and the `ruby:`, `javascript:` and `css:` embedded engines. Other embedded engines,
//! attribute splats and non-HTML5 doctypes are recorded as survey gaps
//! rather than mis-compiled. Escaping (`=` vs `==`) is not distinguished,
//! as in `haml`.

use crate::erb::{ErbSegment, ruby_string_literal};
use crate::haml::{
    Capture, Close, Compiler, Frame, VOID_ELEMENTS, is_middle_marker, is_name_char, split_lines,
};

/// Compile Slim source to the `_buf`-append Ruby program.
pub fn compile_slim(source: &str) -> String {
    compile_slim_mapped(source).0
}

/// As [`compile_slim`], plus the compiled-Ruby ↔ template segment map.
pub fn compile_slim_mapped(source: &str) -> (String, Vec<ErbSegment>) {
    let mut c = Compiler { out: String::new(), map: Vec::new(), stack: Vec::new() };
    c.out.push_str("_buf = \"\"\n");

    // Indent of an open `|` / `'` text block: deeper lines are its text.
    let mut text_block: Option<usize> = None;
    let lines = split_lines(source);
    let mut i = 0;
    while i < lines.len() {
        let (line_start, line) = lines[i];
        i += 1;
        let indent = line.len() - line.trim_start().len();
        let content = line.trim_start();
        if content.is_empty() {
            continue;
        }

        if let Some(block) = text_block {
            if indent > block {
                let e = line_start + indent;
                c.text_mapped(&format!("{content}\n"), e, e + content.len());
                continue;
            }
            text_block = None;
        }

        if let Some(top) = c.stack.last() {
            if top.capture != Capture::None && indent > top.indent {
                if top.capture == Capture::RawRuby {
                    c.code(content, line_start + indent);
                }
                continue;
            }
        }

        // Continuations: trailing `\`, trailing comma on `=` / `-` lines,
        // and a `(`/`[`/`{` attribute wrapper left open across lines. The
        // joined line maps to the first line's range (spans clamp).
        let mut content = content.to_string();
        while i < lines.len() {
            let t = content.trim_end();
            if let Some(head) = t.strip_suffix('\\') {
                content = head.trim_end().to_string();
            } else if !((t.ends_with(',') && matches!(t.as_bytes()[0], b'=' | b'-'))
                || unclosed_wrapper(t))
            {
                break;
            }
            content.push(' ');
            content.push_str(lines[i].1.trim_start());
            i += 1;
        }

        let middle = content.starts_with('-')
            && is_middle_marker(content.trim_start_matches('-').trim_start());
        c.close_to(indent, middle);
        line_to_ruby(&mut c, &content, indent, line_start + indent, &mut text_block);
    }

    while let Some(f) = c.stack.pop() {
        c.emit_close(&f);
    }
    c.out.push_str("_buf\n");
    (c.out, c.map)
}

fn gap(message: String) {
    crate::ingest::survey::record(&crate::ingest::IngestError::Unsupported {
        file: String::new(),
        message,
    });
}

fn skip_frame(c: &mut Compiler, indent: usize, capture: Capture) {
    c.stack.push(Frame { indent, close: Close::Nothing, capture, ruby_block: false });
}

/// Compile one (continuation-joined) Slim line.
fn line_to_ruby(
    c: &mut Compiler,
    content: &str,
    indent: usize,
    e_base: usize,
    text_block: &mut Option<usize>,
) {
    let first = content.as_bytes()[0];
    match first {
        b'/' if content.starts_with("/!") => c.html_comment(content[2..].trim(), indent),
        b'/' => skip_frame(c, indent, Capture::Skip),
        b'|' | b'\'' => {
            let rest = content[1..].trim_start();
            let mut text = rest.to_string();
            if first == b'\'' {
                text.push(' ');
            }
            if !text.is_empty() {
                let off = e_base + content.len() - rest.len();
                c.text_mapped(&text, off, off + rest.len());
            }
            *text_block = Some(indent);
        }
        b'=' => {
            let n = if content.starts_with("==") { 2 } else { 1 };
            let expr = content[n..].trim_start_matches(['<', '>']).trim();
            c.output(expr, n == 2, indent, e_base + content.len() - expr.len());
        }
        b'-' => {
            let rest = content[1..].trim_start();
            c.silent(rest, indent, e_base + content.len() - rest.len());
        }
        b'<' => c.text_mapped(content, e_base, e_base + content.len()),
        b'.' | b'#' if content.as_bytes().get(1).is_some_and(|&b| is_name_char(b)) => {
            element(c, content, indent, e_base, text_block)
        }
        b if b.is_ascii_alphabetic() => {
            if let Some(rest) = content.strip_prefix("doctype") {
                let xhtml = |kind: &str, dtd: &str| {
                    format!("<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 {kind}//EN\" \"http://www.w3.org/TR/xhtml1/DTD/{dtd}.dtd\">\n")
                };
                match rest.trim() {
                    "html" | "5" => c.text("<!DOCTYPE html>\n"),
                    "transitional" => c.text(&xhtml("Transitional", "xhtml1-transitional")),
                    "strict" => c.text(&xhtml("Strict", "xhtml1-strict")),
                    other => {
                        gap(format!("slim doctype not supported: {other}"));
                        c.text("<!DOCTYPE html>\n");
                    }
                }
            } else if let Some(name) = embedded_engine(content) {
                if name == "ruby" {
                    skip_frame(c, indent, Capture::RawRuby);
                } else if let Some(tag) = match name {
                    "javascript" => Some("script"),
                    "css" => Some("style"),
                    _ => None,
                } {
                    // Body lines are text (with `#{}` interpolation).
                    c.text(&format!("<{tag}>"));
                    c.stack.push(Frame {
                        indent,
                        close: Close::Tag(tag.to_string()),
                        capture: Capture::None,
                        ruby_block: false,
                    });
                    *text_block = Some(indent);
                } else {
                    gap(format!("slim embedded engine not supported: {name}:"));
                    skip_frame(c, indent, Capture::Skip);
                }
            } else {
                element(c, content, indent, e_base, text_block)
            }
        }
        _ => {
            gap(format!("slim line not supported: {}", content.chars().take(20).collect::<String>()));
            c.text_mapped(content, e_base, e_base + content.len());
        }
    }
}

/// Does this element line open a `(`/`[`/`{` attribute wrapper right after
/// its `tag.class#id` head without closing it on the same line?
fn unclosed_wrapper(mut content: &str) -> bool {
    let (b, pos) = loop {
        let b = content.as_bytes();
        if !(b[0].is_ascii_alphabetic() || b[0] == b'.' || b[0] == b'#') {
            return false;
        }
        let mut pos = 0;
        while pos < b.len() && (is_name_char(b[pos]) || b[pos] == b'.' || b[pos] == b'#') {
            pos += 1;
        }
        // `li: a(` — the wrapper belongs to the innermost inline child.
        match content[pos..].strip_prefix(": ") {
            Some(rest) if !rest.trim_start().is_empty() => content = rest.trim_start(),
            _ => break (b, pos),
        }
    };
    // Slim allows blanks between the head and the wrapper (`div [`).
    let pos = pos + b[pos..].iter().take_while(|&&x| x == b' ' || x == b'\t').count();
    if !matches!(b.get(pos), Some(b'(' | b'[' | b'{')) {
        return false;
    }
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    for (k, &ch) in b.iter().enumerate().skip(pos) {
        if let Some(q) = quote {
            if ch == q && b[k - 1] != b'\\' {
                quote = None;
            }
        } else {
            match ch {
                b'"' | b'\'' => quote = Some(ch),
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return false;
                    }
                }
                _ => {}
            }
        }
    }
    true
}

/// `javascript:` / `ruby:` / … on a line by itself.
fn embedded_engine(content: &str) -> Option<&str> {
    let name = content.strip_suffix(':')?;
    ["ruby", "javascript", "css", "coffee", "markdown", "scss", "sass", "less", "erb", "haml"]
        .contains(&name)
        .then_some(name)
}

enum AttrValue {
    /// Bare boolean attribute (wrapped form only).
    Bool,
    /// Ruby expression (a quoted string is one), as a byte range in the line.
    Code(usize, usize),
}

/// End of an unquoted Ruby value starting at `pos`: up to whitespace / the
/// wrapper's `close` at bracket depth 0 (quotes inside are skipped over).
fn value_end(b: &[u8], mut pos: usize, close: Option<u8>) -> usize {
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    while pos < b.len() {
        let ch = b[pos];
        if let Some(q) = quote {
            if ch == b'\\' {
                pos += 1;
            } else if ch == q {
                quote = None;
            }
        } else {
            match ch {
                b'"' | b'\'' => quote = Some(ch),
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' if depth == 0 && close == Some(ch) => return pos,
                b')' | b']' | b'}' => depth -= 1,
                b' ' | b'\t' if depth == 0 => return pos,
                _ => {}
            }
        }
        pos += 1;
    }
    pos
}

/// Parse `name=value` pairs from `pos`. `close` is the wrapper's closing
/// byte (wrapped form, where bare names are booleans), `None` for the
/// unwrapped form (stops at the first token that is not `name=`).
fn parse_attrs(content: &str, mut pos: usize, close: Option<u8>) -> (Vec<(String, AttrValue)>, usize) {
    let b = content.as_bytes();
    let mut attrs = Vec::new();
    loop {
        let before_ws = pos;
        while pos < b.len() && (b[pos] == b' ' || b[pos] == b'\t') {
            pos += 1;
        }
        if close.is_some() && b.get(pos) == close.as_ref() {
            return (attrs, pos + 1);
        }
        let start = pos;
        while pos < b.len() && (is_name_char(b[pos]) || b[pos] == b':' || b[pos] == b'@') {
            pos += 1;
        }
        if pos == start {
            return (attrs, if close.is_some() { pos } else { before_ws });
        }
        let name = content[start..pos].to_string();
        if b.get(pos) != Some(&b'=') {
            if close.is_none() {
                return (attrs, before_ws); // text, not an attribute
            }
            attrs.push((name, AttrValue::Bool));
            continue;
        }
        pos += 1;
        if b.get(pos) == Some(&b'=') {
            pos += 1; // `a==raw` — unescaped; escaping isn't modeled
        }
        let v_start = pos;
        let v_end = match b.get(pos) {
            Some(&q @ (b'"' | b'\'')) => {
                let mut p = pos + 1;
                while p < b.len() && b[p] != q {
                    p += if b[p] == b'\\' { 2 } else { 1 };
                }
                (p + 1).min(b.len())
            }
            _ => value_end(b, pos, close),
        };
        attrs.push((name, AttrValue::Code(v_start, v_end)));
        pos = v_end;
    }
}

fn element(
    c: &mut Compiler,
    content: &str,
    indent: usize,
    e_base: usize,
    text_block: &mut Option<usize>,
) {
    let b = content.as_bytes();
    let mut pos = 0;
    while pos < b.len() && (b[pos].is_ascii_alphanumeric() || b[pos] == b'-' || b[pos] == b'_') {
        pos += 1;
    }
    let tag = if pos == 0 { "div".to_string() } else { content[..pos].to_string() };

    let mut classes: Vec<&str> = Vec::new();
    let mut id = None;
    while pos < b.len() && (b[pos] == b'.' || b[pos] == b'#') {
        let kind = b[pos];
        let start = pos + 1;
        pos = start;
        while pos < b.len() && is_name_char(b[pos]) {
            pos += 1;
        }
        if kind == b'.' {
            classes.push(&content[start..pos]);
        } else {
            id = Some(&content[start..pos]);
        }
    }

    let pos = pos + b[pos..].iter().take_while(|&&x| x == b' ' || x == b'\t').count();
    let (attrs, pos) = match b.get(pos) {
        Some(&open @ (b'(' | b'[' | b'{')) => {
            let close = match open {
                b'(' => b')',
                b'[' => b']',
                _ => b'}',
            };
            parse_attrs(content, pos + 1, Some(close))
        }
        _ => parse_attrs(content, pos, None),
    };

    if attrs.is_empty() && classes.is_empty() && id.is_none() {
        c.text(&format!("<{tag}>"));
    } else {
        c.text(&format!("<{tag}"));
        let class = classes.join(" ");
        let has_class_attr = attrs.iter().any(|(n, _)| n == "class");
        c.out.push_str("_buf = _buf + (render_attrs({ ");
        if !class.is_empty() && !has_class_attr {
            c.out.push_str(&format!("class: {class:?}, "));
        }
        if let Some(id) = id {
            c.out.push_str(&format!("id: {id:?}, "));
        }
        for (name, value) in &attrs {
            c.out.push_str(&format!("{name:?}: "));
            match value {
                AttrValue::Bool => c.out.push_str("true"),
                AttrValue::Code(s, e) => {
                    // `.a class="b"` merges the shortcut classes in.
                    let merge = name == "class" && !class.is_empty();
                    if merge {
                        c.out.push_str(&format!("\"{class} #{{"));
                    }
                    let c_start = c.out.len();
                    c.out.push_str(&content[*s..*e]);
                    c.seg(c_start, e_base + s, e_base + e);
                    if merge {
                        c.out.push_str("}\"");
                    }
                }
            }
            c.out.push_str(", ");
        }
        c.out.push_str("})).to_s\n");
        c.text(">");
    }

    let rest = content[pos..].trim_start_matches(['<', '>']);
    let (self_close, rest) = match rest.trim_start().strip_prefix('/') {
        Some(r) => (true, r),
        None => (false, rest),
    };
    if self_close || VOID_ELEMENTS.contains(&tag.as_str()) {
        return;
    }
    let rest = rest.trim_start();
    let off = e_base + content.len() - rest.len();

    if rest.is_empty() {
        c.stack.push(Frame { indent, close: Close::Tag(tag), capture: Capture::None, ruby_block: false });
    } else if let Some(expr) = rest.strip_prefix("==").or_else(|| rest.strip_prefix('=')) {
        let skip = rest.len() - expr.len();
        let expr = expr.trim();
        c.out.push_str("_buf = _buf + (");
        let c_start = c.out.len();
        c.out.push_str(expr);
        c.seg(c_start, off + skip, off + skip + expr.len());
        c.out.push_str("\n).to_s\n");
        c.text(&format!("</{tag}>"));
    } else if let Some(child) = rest.strip_prefix(':').filter(|r| r.starts_with(' ')) {
        // `li: a href="x" text` — nest the rest of the line one level down.
        c.stack.push(Frame { indent, close: Close::Tag(tag), capture: Capture::None, ruby_block: false });
        let child_trim = child.trim_start();
        let child_off = off + rest.len() - child_trim.len();
        line_to_ruby(c, child_trim, indent + 1, child_off, text_block);
    } else {
        c.out.push_str("_buf = _buf + ");
        let c_start = c.out.len();
        c.out.push_str(&ruby_string_literal(rest));
        c.seg(c_start, off, off + rest.len());
        c.out.push('\n');
        // Deeper lines continue the inline text; the frame closes the tag.
        c.stack.push(Frame { indent, close: Close::Tag(tag), capture: Capture::None, ruby_block: false });
        *text_block = Some(indent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ruby(src: &str) -> String {
        compile_slim_mapped(src).0
    }

    #[test]
    fn tag_text_and_shortcuts() {
        let r = ruby("p hello\n.box\n#main\n");
        assert!(r.contains("_buf = _buf + \"<p>\""), "got:\n{r}");
        assert!(r.contains("\"hello\""));
        assert!(r.contains("render_attrs({ class: \"box\", })"), "got:\n{r}");
        assert!(r.contains("render_attrs({ id: \"main\", })"), "got:\n{r}");
        assert_eq!(r.matches("</div>").count(), 2);
    }

    #[test]
    fn attributes_quoted_ruby_and_wrapped() {
        let r = ruby("a href=post_path(@p) title=\"t #{x}\" Go\n");
        assert!(r.contains("\"href\": post_path(@p), \"title\": \"t #{x}\", "), "got:\n{r}");
        assert!(r.contains("\"Go\""), "text after attrs: {r}");
        let r = ruby("input(type=\"checkbox\" checked)\n");
        assert!(r.contains("\"type\": \"checkbox\", \"checked\": true, "), "got:\n{r}");
        assert!(!r.contains("</input>"));
    }

    #[test]
    fn class_shortcut_merges_with_class_attr() {
        let r = ruby("a.btn class=extra x\n");
        assert!(r.contains("\"class\": \"btn #{extra}\""), "got:\n{r}");
    }

    #[test]
    fn output_and_inline_output() {
        let r = ruby("h1 = @post.title\n= yield\n");
        assert!(r.contains("(@post.title\n).to_s"), "got:\n{r}");
        assert!(r.contains("(yield\n).to_s"));
        assert!(r.contains("</h1>"));
    }

    #[test]
    fn code_blocks_close_by_indent_and_else_continues() {
        let r = ruby("- if x\n  p a\n- else\n  p b\n");
        assert_eq!(r.matches("\nend\n").count(), 1, "got:\n{r}");
        assert!(r.contains("else\n"));
    }

    #[test]
    fn output_block_closes_with_end_to_s() {
        let r = ruby("= form_with model: @p do |f|\n  = f.text_field :a\n");
        assert!(r.contains("end).to_s"), "got:\n{r}");
    }

    #[test]
    fn inline_nesting() {
        let r = ruby("ul\n  li: a href=\"/\" Home\n");
        assert!(r.contains("<ul>") && r.contains("<li>") && r.contains("<a"), "got:\n{r}");
        let order = ["</a>", "</li>", "</ul>"].map(|t| r.find(t).unwrap());
        assert!(order[0] < order[1] && order[1] < order[2], "got:\n{r}");
    }

    #[test]
    fn text_blocks_and_comments() {
        let r = ruby("/ hidden\n  also hidden\n| plain\n  more\n' trail\n/! shown\n");
        assert!(!r.contains("hidden"), "got:\n{r}");
        assert!(r.contains("\"plain\"") && r.contains("\"more\\n\""), "got:\n{r}");
        assert!(r.contains("\"trail \""), "got:\n{r}");
        assert!(r.contains("<!--") && r.contains("-->"));
    }

    #[test]
    fn inline_text_continues_on_deeper_lines() {
        let r = ruby("p one\n  two\n  three\nspan x\n");
        assert!(r.contains("\"two\\n\"") && r.contains("\"three\\n\""), "got:\n{r}");
        assert!(r.find("</p>").unwrap() > r.find("three").unwrap(), "got:\n{r}");
        assert!(r.find("</p>").unwrap() < r.find("<span>").unwrap(), "got:\n{r}");
    }

    #[test]
    fn doctype_and_ruby_engine() {
        let r = ruby("doctype html\nruby:\n  x = 1\n");
        assert!(r.contains("<!DOCTYPE html>"), "got:\n{r}");
        assert!(ruby("doctype transitional\n").contains("XHTML 1.0 Transitional"));
        assert!(r.contains("x = 1\n") && !r.contains("\"x = 1"), "got:\n{r}");
    }

    #[test]
    fn multiline_wrapper_and_backslash_continuation() {
        let r = ruby("div[\n  a=\"1\"\n  b='{ \"x\": #{y} }'\n]\n= link_to p,\\\n  rel: 'n'\n");
        assert!(r.contains("\"a\": \"1\", \"b\": '{ \"x\": #{y} }', "), "got:\n{r}");
        assert!(r.contains("link_to p, rel: 'n'"), "got:\n{r}");
        assert!(r.contains("</div>"));
    }

    #[test]
    fn multiline_wrapper_on_inline_child() {
        let r = ruby("div: a.t(\n  href=x\n) = y\n");
        assert!(r.contains("\"href\": x, ") && r.contains("(y\n).to_s"), "got:\n{r}");
    }

    #[test]
    fn javascript_and_css_filters_wrap_text() {
        let r = ruby("javascript:\n  var a = \"#{x}\";\n\ncss:\n  a { b: c }\np z\n");
        assert!(r.contains("\"<script>\"") && r.contains("</script>"), "got:\n{r}");
        assert!(r.contains("var a = \\\"#{x}\\\";"), "got:\n{r}");
        assert!(r.contains("<style>") && r.contains("</style>") && r.contains("<p>"), "got:\n{r}");
    }

    #[test]
    fn raw_attribute_value() {
        let r = ruby("input(placeholder==t('k') type=\"search\"\n)\n");
        assert!(r.contains("\"placeholder\": t('k'), \"type\": \"search\", "), "got:\n{r}");
    }

    #[test]
    fn blank_before_wrapper() {
        let r = ruby("div [\n  a=\"1\"\n  ]\n");
        assert!(r.contains("\"a\": \"1\", ") && r.contains("</div>"), "got:\n{r}");
    }

    #[test]
    fn program_has_buf_prologue_and_epilogue() {
        let r = ruby("p hi\n");
        assert!(r.starts_with("_buf = \"\"\n"));
        assert_eq!(r.trim_end().lines().last(), Some("_buf"));
    }
}
