use unicode_segmentation::UnicodeSegmentation;

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

impl LineInput {
    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn edit(&mut self, edit: LineEdit) -> bool {
        let before = (self.value.clone(), self.cursor);
        match edit {
            LineEdit::Insert(character) => {
                self.value.insert(self.cursor, character);
                self.cursor += character.len_utf8();
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
}
