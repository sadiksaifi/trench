use crate::{
    ref_catalog::{RefCandidate, RefSnapshot},
    tui::line_input::{LineEdit, LineInput},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefPicker {
    refs: RefSnapshot,
    configured_base: Option<String>,
    selected: Option<String>,
    query: LineInput,
    selection: usize,
    origin_refresh: OriginRefresh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefPickerKey {
    Character(char),
    Backspace,
    Edit(LineEdit),
    Up,
    Down,
    Enter,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefPickerEffect {
    Selected(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OriginRefresh {
    Idle,
    Loading,
    Failed,
}

impl RefPicker {
    pub fn new(refs: RefSnapshot, configured_base: Option<&str>) -> Self {
        let selected = refs.default_base(configured_base).ok();
        Self {
            refs,
            configured_base: configured_base.map(ToOwned::to_owned),
            selected,
            query: LineInput::default(),
            selection: 0,
            origin_refresh: OriginRefresh::Idle,
        }
    }

    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    pub fn candidates(&self) -> Vec<RefCandidate> {
        self.refs
            .candidates()
            .into_iter()
            .filter(|candidate| fuzzy_matches(&candidate.name, self.query.value()))
            .collect()
    }

    pub fn open(&mut self) {
        self.query = LineInput::default();
        self.selection = 0;
    }

    pub fn query(&self) -> &str {
        self.query.value()
    }

    pub fn selection(&self) -> usize {
        self.selection
    }

    pub fn set_origin_refresh(&mut self, refresh: OriginRefresh) {
        self.origin_refresh = refresh;
    }

    pub fn update_refs(&mut self, refs: RefSnapshot) {
        let selected_still_exists = self
            .selected
            .as_deref()
            .and_then(|base| refs.resolve(base))
            .is_some();
        if !selected_still_exists {
            self.selected = refs.default_base(self.configured_base.as_deref()).ok();
        }
        self.refs = refs;
        self.selection = self
            .selection
            .min(self.candidates().len().saturating_sub(1));
    }

    pub fn origin_spinner_visible(&self) -> bool {
        self.origin_refresh == OriginRefresh::Loading
    }

    pub fn warning(&self) -> Option<&'static str> {
        (self.origin_refresh == OriginRefresh::Failed)
            .then_some("Could not update origin; showing local and stale refs")
    }

    pub fn handle_key(&mut self, key: RefPickerKey) -> Option<RefPickerEffect> {
        match key {
            RefPickerKey::Character(character) => {
                self.edit_query(LineEdit::Insert(character));
                None
            }
            RefPickerKey::Backspace => {
                self.edit_query(LineEdit::DeletePreviousCharacter);
                None
            }
            RefPickerKey::Edit(edit) => {
                self.edit_query(edit);
                None
            }
            RefPickerKey::Up => {
                self.selection = self.selection.saturating_sub(1);
                None
            }
            RefPickerKey::Down => {
                self.selection = self
                    .selection
                    .saturating_add(1)
                    .min(self.candidates().len().saturating_sub(1));
                None
            }
            RefPickerKey::Enter => {
                let selected = self
                    .candidates()
                    .get(self.selection)
                    .map(|candidate| candidate.name.clone())?;
                self.selected = Some(selected.clone());
                Some(RefPickerEffect::Selected(selected))
            }
        }
    }

    fn edit_query(&mut self, edit: LineEdit) {
        let before = self.query.value().to_string();
        self.query.edit(edit);
        if self.query.value() != before {
            self.selection = 0;
        }
    }
}

fn fuzzy_matches(candidate: &str, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let mut query = query.chars().flat_map(char::to_lowercase);
    let mut next = query.next();
    for candidate in candidate.chars().flat_map(char::to_lowercase) {
        if next == Some(candidate) {
            next = query.next();
            if next.is_none() {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ref_catalog::RefSnapshot;

    fn refs() -> RefSnapshot {
        RefSnapshot::from_parts(
            ["main", "release"],
            ["origin/main", "origin/release", "origin/topic/two"],
            Some("origin/main"),
            Some("main"),
            true,
        )
    }

    #[test]
    fn picker_starts_at_the_configured_base_and_preserves_remote_only_refs() {
        let picker = RefPicker::new(refs(), Some("release"));

        assert_eq!(picker.selected(), Some("release"));
        assert_eq!(
            picker
                .candidates()
                .iter()
                .map(|candidate| candidate.name.as_str())
                .collect::<Vec<_>>(),
            ["main", "release", "origin/topic/two"]
        );
    }

    #[test]
    fn picker_filters_and_confirms_a_base_from_keyboard_input() {
        let mut picker = RefPicker::new(refs(), None);

        picker.open();
        assert_eq!(picker.handle_key(RefPickerKey::Character('r')), None);
        assert_eq!(picker.handle_key(RefPickerKey::Character('e')), None);
        assert_eq!(
            picker
                .candidates()
                .iter()
                .map(|candidate| candidate.name.as_str())
                .collect::<Vec<_>>(),
            ["release"]
        );
        assert_eq!(
            picker.handle_key(RefPickerKey::Enter),
            Some(RefPickerEffect::Selected("release".to_string()))
        );
        assert_eq!(picker.selected(), Some("release"));
    }

    #[test]
    fn picker_query_supports_mid_line_unicode_edits() {
        use crate::tui::line_input::LineEdit;

        let mut picker = RefPicker::new(refs(), None);
        for character in "r👨‍👩‍👧‍👦e".chars() {
            picker.handle_key(RefPickerKey::Character(character));
        }

        picker.handle_key(RefPickerKey::Edit(LineEdit::Start));
        picker.handle_key(RefPickerKey::Edit(LineEdit::NextCharacter));
        picker.handle_key(RefPickerKey::Edit(LineEdit::DeleteNextCharacter));

        assert_eq!(picker.query(), "re");
        assert_eq!(
            picker
                .candidates()
                .iter()
                .map(|candidate| candidate.name.as_str())
                .collect::<Vec<_>>(),
            ["release"]
        );
    }
}
