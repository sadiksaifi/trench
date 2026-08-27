use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEdit {
    Insert(char),
    Start,
    End,
    PreviousCharacter,
    NextCharacter,
    DeletePreviousCharacter,
    DeleteNextCharacter,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineInput {
    value: String,
    cursor: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineWindow {
    pub before_cursor: String,
    pub after_cursor: String,
}

impl LineInput {
    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn window(&self, width: usize) -> LineWindow {
        let before = &self.value[..self.cursor];
        let after = &self.value[self.cursor..];
        if UnicodeWidthStr::width(self.value.as_str()) <= width {
            return LineWindow {
                before_cursor: before.to_string(),
                after_cursor: after.to_string(),
            };
        }

        let before_cursor = tail_ellipsize(before, width);
        let remaining = width.saturating_sub(UnicodeWidthStr::width(before_cursor.as_str()));
        LineWindow {
            before_cursor,
            after_cursor: head_ellipsize(after, remaining),
        }
    }

    pub fn edit(&mut self, edit: LineEdit) -> bool {
        let before = (self.value.clone(), self.cursor);
        match edit {
            LineEdit::Insert(character) => {
                self.value.insert(self.cursor, character);
                self.cursor += character.len_utf8();
                self.cursor = self
                    .value
                    .grapheme_indices(true)
                    .map(|(index, _)| index)
                    .chain(std::iter::once(self.value.len()))
                    .find(|boundary| *boundary >= self.cursor)
                    .unwrap_or(self.value.len());
            }
            LineEdit::Start => self.cursor = 0,
            LineEdit::End => self.cursor = self.value.len(),
            LineEdit::PreviousCharacter => {
                self.cursor = self.value[..self.cursor]
                    .grapheme_indices(true)
                    .next_back()
                    .map(|(index, _)| index)
                    .unwrap_or_default();
            }
            LineEdit::NextCharacter => {
                self.cursor += self.value[self.cursor..]
                    .graphemes(true)
                    .next()
                    .map(str::len)
                    .unwrap_or_default();
            }
            LineEdit::DeletePreviousCharacter => {
                let previous = self.value[..self.cursor]
                    .grapheme_indices(true)
                    .next_back()
                    .map(|(index, _)| index)
                    .unwrap_or(self.cursor);
                self.value.drain(previous..self.cursor);
                self.cursor = previous;
            }
            LineEdit::DeleteNextCharacter => {
                if let Some(grapheme) = self.value[self.cursor..].graphemes(true).next() {
                    self.value
                        .drain(self.cursor..self.cursor.saturating_add(grapheme.len()));
                }
            }
        }
        before != (self.value.clone(), self.cursor)
    }
}

fn tail_ellipsize(value: &str, width: usize) -> String {
    if UnicodeWidthStr::width(value) <= width {
        return value.to_string();
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".to_string();
    }
    let budget = width - 1;
    let mut used = 0;
    let mut tail = Vec::new();
    for grapheme in value.graphemes(true).rev() {
        let grapheme_width = UnicodeWidthStr::width(grapheme);
        if used + grapheme_width > budget {
            break;
        }
        tail.push(grapheme);
        used += grapheme_width;
    }
    tail.reverse();
    format!("…{}", tail.concat())
}

fn head_ellipsize(value: &str, width: usize) -> String {
    if UnicodeWidthStr::width(value) <= width {
        return value.to_string();
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".to_string();
    }
    let budget = width - 1;
    let mut used = 0;
    let mut head = Vec::new();
    for grapheme in value.graphemes(true) {
        let grapheme_width = UnicodeWidthStr::width(grapheme);
        if used + grapheme_width > budget {
            break;
        }
        head.push(grapheme);
        used += grapheme_width;
    }
    format!("{}…", head.concat())
}

impl From<&str> for LineInput {
    fn from(value: &str) -> Self {
        Self {
            value: value.to_string(),
            cursor: value.len(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{LineEdit, LineInput};

    #[test]
    fn user_can_edit_in_the_middle_without_splitting_a_unicode_grapheme() {
        let mut input = LineInput::from("a👨‍👩‍👧‍👦界");

        input.edit(LineEdit::Start);
        input.edit(LineEdit::NextCharacter);
        input.edit(LineEdit::DeleteNextCharacter);
        input.edit(LineEdit::Insert('b'));

        assert_eq!(input.value(), "ab界");
    }

    #[test]
    fn user_can_move_backward_and_delete_the_previous_unicode_grapheme() {
        let mut input = LineInput::from("a👨‍👩‍👧‍👦界");

        input.edit(LineEdit::End);
        input.edit(LineEdit::PreviousCharacter);
        input.edit(LineEdit::DeletePreviousCharacter);

        assert_eq!(input.value(), "a界");
    }

    #[test]
    fn inserted_combining_marks_never_leave_the_cursor_inside_a_grapheme() {
        let mut input = LineInput::from("\u{301}x");

        input.edit(LineEdit::Start);
        input.edit(LineEdit::Insert('a'));
        input.edit(LineEdit::DeletePreviousCharacter);

        assert_eq!(input.value(), "x");
    }
}
