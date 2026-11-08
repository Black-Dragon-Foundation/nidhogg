//! What an input method asks a text view, answered from pushed state, as NS answers it: ranges count Emacs
//! characters from the buffer's accessible start (`BEGV`), the selection is the region when the mark is active, and
//! the marked text sits at point.  C pushes the line around point (`text_input_surrounding`) and where point, the
//! mark and the line's start are (`nhg_im_offsets`); text outside the line answers empty.  Platform-neutral, so its
//! tests run on Linux.

/// The pushed line, its start, point and the selection's other end (characters from `BEGV`), and the marked text's
/// length in characters.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Surrounding {
    pub text: String,
    pub line_start: usize,
    pub point: usize,
    pub mark: usize,
    pub marked: usize,
}

impl Surrounding {
    /// `selectedRange`: the region (point and the mark, either order), or point, empty.
    pub fn selected(&self) -> (usize, usize) {
        (self.point.min(self.mark), self.point.abs_diff(self.mark))
    }

    /// `markedRange`: at point, as long as the marked text; `None` without it.
    pub fn marked_range(&self) -> Option<(usize, usize)> {
        (self.marked > 0).then_some((self.point, self.marked))
    }

    /// `accessibilitySelectedText`: the region's text within the line, empty without a region.
    pub fn selected_text(&self) -> String {
        let (l, n) = self.selected();
        self.substring(l, n).map(|(s, _)| s).unwrap_or_default()
    }

    /// `attributedSubstringForProposedRange:actualRange:`: the part of (`location`, `length`) inside the line and the
    /// range it covers; `None` when nothing of it is.
    pub fn substring(&self, location: usize, length: usize) -> Option<(String, (usize, usize))> {
        let total = self.text.chars().count();
        let end = location.saturating_add(length).min(self.line_start + total);
        let start = location.max(self.line_start);
        if start >= end {
            return None;
        }
        let got: String = self
            .text
            .chars()
            .skip(start - self.line_start)
            .take(end - start)
            .collect();
        Some((got, (start, end - start)))
    }
}

#[cfg(test)]
#[path = "ime_text_tests.rs"]
mod tests;
