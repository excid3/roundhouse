//! `#<SPINEL_SOURCE>file:line` markers: the Spinel tree's source map.
//!
//! Spinel's `#line` directives (and with them `-g`/`--debug` stepping,
//! `perf`/`addr2line`, `--warn-widen`, `--check-stores`) name the .rb
//! they were compiled from. For our tree that is lowered Ruby, not the
//! app's `.rb`/`.erb`. A whole-line `#<SPINEL_SOURCE>path:line` comment
//! (spinel#7630, docs/profiling.md) pins the reported position for the
//! lines after it, held rather than counted up, until the next marker
//! or the end of the file. CRuby reads it as a comment.
//!
//! Emission is two halves. While `with_source_markers` is in effect,
//! the expression emitter writes a marker before each statement of a
//! multi-statement `Seq` and `emit_method` writes one at each `def`,
//! from the IR span (view spans already index the raw template). Those
//! strings are built inside-out and indented by their callers, so they
//! cannot know what the previous marker in the FILE was. `finish` does
//! that afterwards: it moves each marker to column 0 (Spinel matches it
//! only there) and drops the ones that repeat the position in effect.
//!
//! Spinel has no "unpin" form, so code with no span after a marker
//! reports the last pinned position. Synthesized methods still get
//! their own `def` marker when the lowerer gave them a span.

use std::cell::RefCell;

use crate::App;
use crate::span::Span;

pub(crate) const MARKER_PREFIX: &str = "#<SPINEL_SOURCE>";

struct Sources {
    /// `FileId(n)` → entry `n - 1`: the app-relative path and the byte
    /// offset each line starts at.
    files: Vec<(String, Vec<u32>)>,
}

thread_local! {
    static SOURCES: RefCell<Option<Sources>> = const { RefCell::new(None) };
}

/// Run `f` with marker emission on, resolving spans against `app`'s
/// sources. Paths are app-relative (`App::root` stripped) under the
/// app directory's own name: `real-blog/app/models/article.rb`. Bare
/// `app/models/article.rb` is also the emitted file's PHYSICAL path,
/// which Spinel reports for lines no marker covers, so the two would
/// read as one file with two sets of line numbers.
pub fn with_source_markers<R>(app: &App, f: impl FnOnce() -> R) -> R {
    let root = app.root.as_str();
    let label = std::path::Path::new(root.trim_end_matches('/'))
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| format!("{n}/"))
        .unwrap_or_default();
    let files = app
        .sources
        .iter()
        .map(|s| {
            let rel = s
                .path
                .strip_prefix(root)
                .map(|r| r.trim_start_matches('/'))
                .unwrap_or(&s.path);
            let rel = format!("{label}{rel}");
            let mut starts = vec![0u32];
            starts.extend(
                s.text.bytes().enumerate().filter(|(_, b)| *b == b'\n').map(|(i, _)| i as u32 + 1),
            );
            (rel, starts)
        })
        .collect();
    let prev = SOURCES.with(|c| c.replace(Some(Sources { files })));
    let r = f();
    SOURCES.with(|c| *c.borrow_mut() = prev);
    r
}

/// The marker naming `span`'s start, or `None` when markers are off,
/// the span is synthetic, or it names no registered file.
pub(super) fn marker_for(span: &Span) -> Option<String> {
    if span.is_synthetic() || span.file.0 == 0 {
        return None;
    }
    SOURCES.with(|c| {
        let c = c.borrow();
        let (path, starts) = c.as_ref()?.files.get(span.file.0 as usize - 1)?;
        let line = starts.partition_point(|&s| s <= span.start);
        Some(format!("{MARKER_PREFIX}{path}:{line}"))
    })
}

/// Normalize the markers in one emitted file: each to column 0, and only
/// where the position changes. A marker immediately followed by another
/// (no code between) is superseded and dropped, as is one at the end.
pub fn finish(content: &str) -> String {
    if !content.contains(MARKER_PREFIX) {
        return content.to_string();
    }
    let mut out: Vec<&str> = Vec::new();
    let mut active: Option<&str> = None;
    // The marker waiting for a code line to make it effective.
    let mut pending: Option<&str> = None;
    for line in content.split('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with(MARKER_PREFIX) {
            pending = Some(trimmed);
            continue;
        }
        if let Some(m) = pending.take() {
            if !trimmed.is_empty() && active != Some(m) {
                out.push(m);
                active = Some(m);
            } else if trimmed.is_empty() {
                // A blank line is not code: keep waiting.
                pending = Some(m);
            }
        }
        out.push(line);
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finish_moves_to_column_zero_and_drops_repeats() {
        let src = "def a\n  #<SPINEL_SOURCE>x.rb:3\n  b\n  #<SPINEL_SOURCE>x.rb:3\n  c\nend\n";
        assert_eq!(finish(src), "def a\n#<SPINEL_SOURCE>x.rb:3\n  b\n  c\nend\n");
    }

    #[test]
    fn finish_drops_superseded_and_trailing_markers() {
        let src = "#<SPINEL_SOURCE>x.rb:1\n#<SPINEL_SOURCE>x.rb:2\n\na\n#<SPINEL_SOURCE>x.rb:9\n";
        assert_eq!(finish(src), "\n#<SPINEL_SOURCE>x.rb:2\na\n");
    }

    #[test]
    fn finish_leaves_files_without_markers_alone() {
        assert_eq!(finish("a\n  b\n"), "a\n  b\n");
    }
}
