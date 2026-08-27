use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEdit {
    Insert(char),
    Start,
    NextCharacter,
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
            LineEdit::NextCharacter => {
                self.cursor += self.value[self.cursor..]
                    .graphemes(true)
                    .next()
                    .map(str::len)
                    .unwrap_or_default();
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
}
