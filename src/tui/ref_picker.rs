use crate::{
    ref_catalog::{RefCandidate, RefSnapshot},
    tui::{
        line_input::{LineEdit, LineInput},
        search,
    },
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
        let mut picker = Self {
            refs,
            configured_base: configured_base.map(ToOwned::to_owned),
            selected,
            query: LineInput::default(),
            selection: 0,
            origin_refresh: OriginRefresh::Idle,
        };
        // Highlight the base in use, wherever the list places it.
        picker.open();
        picker
    }

    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// The refs that match the typed filter, best match first.
    ///
    /// With an empty filter the base in use comes first, followed by the
    /// catalog order: the list opens on the current choice instead of
    /// leaving it at the bottom. The sort is stable, so equal matches keep
    /// that order too.
    pub fn candidates(&self) -> Vec<RefCandidate> {
        let mut ranked = picker_candidates(&self.refs, self.selected.as_deref())
            .into_iter()
            .filter_map(|candidate| {
                let score = search::fuzzy_score(&candidate.name, self.query.value())?;
                let in_use = self.query.value().is_empty()
                    && self.selected.as_deref() == Some(candidate.name.as_str());
                Some(((in_use, score), candidate))
            })
            .collect::<Vec<_>>();
        ranked.sort_by_key(|(rank, _)| std::cmp::Reverse(*rank));
        ranked.into_iter().map(|(_, candidate)| candidate).collect()
    }

    /// Whether the repository has an `origin` remote to fetch from.
    pub fn has_origin(&self) -> bool {
        self.refs.has_origin
    }

    /// Whether the repository has any branch to pick at all.
    pub fn has_refs(&self) -> bool {
        !self.refs.candidates().is_empty()
    }

    pub fn open(&mut self) {
        self.query = LineInput::default();
        self.selection = self
            .selected
            .as_deref()
            .and_then(|selected| {
                self.candidates()
                    .iter()
                    .position(|candidate| candidate.name == selected)
            })
            .unwrap_or(0);
    }

    #[cfg(test)]
    pub fn query(&self) -> &str {
        self.query.value()
    }

    pub fn query_input(&self) -> &LineInput {
        &self.query
    }

    pub fn selection(&self) -> usize {
        self.selection
    }

    pub fn set_origin_refresh(&mut self, refresh: OriginRefresh) {
        self.origin_refresh = refresh;
    }

    pub fn update_refs(&mut self, refs: RefSnapshot) {
        let highlighted = self
            .candidates()
            .get(self.selection)
            .map(|candidate| candidate.name.clone());
        let selected_still_exists = self
            .selected
            .as_deref()
            .and_then(|base| refs.resolve(base))
            .is_some();
        if !selected_still_exists {
            self.selected = refs.default_base(self.configured_base.as_deref()).ok();
        }
        self.refs = refs;
        let candidates = self.candidates();
        self.selection = highlighted
            .as_deref()
            .or(self.selected.as_deref())
            .and_then(|highlighted| {
                candidates
                    .iter()
                    .position(|candidate| candidate.name == highlighted)
            })
            .unwrap_or_else(|| self.selection.min(candidates.len().saturating_sub(1)));
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

fn picker_candidates(refs: &RefSnapshot, preferred: Option<&str>) -> Vec<RefCandidate> {
    let mut candidates = refs.candidates();
    if let Some(preferred) = preferred.filter(|preferred| preferred.starts_with("origin/")) {
        if refs.resolve(preferred).is_some()
            && !candidates
                .iter()
                .any(|candidate| candidate.name == preferred)
        {
            candidates.push(RefCandidate::remote(preferred));
        }
    }
    candidates
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
        let mut picker = RefPicker::new(refs(), Some("release"));

        assert_eq!(picker.selected(), Some("release"));
        assert_eq!(picker.selection(), 0, "the base in use is listed first");
        assert_eq!(
            picker
                .candidates()
                .iter()
                .map(|candidate| candidate.name.as_str())
                .collect::<Vec<_>>(),
            ["release", "main", "origin/topic/two"]
        );
        assert_eq!(
            picker.handle_key(RefPickerKey::Enter),
            Some(RefPickerEffect::Selected("release".to_string()))
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

    #[test]
    fn origin_head_alias_remains_visible_and_submits_the_exact_remote_base() {
        let refs = RefSnapshot::from_parts(
            ["alpha", "main"],
            ["origin/main"],
            Some("origin/main"),
            Some("main"),
            true,
        );
        let mut picker = RefPicker::new(refs, None);

        let selected = &picker.candidates()[picker.selection()];
        assert_eq!(selected.name, "origin/main");
        assert_eq!(
            picker.handle_key(RefPickerKey::Enter),
            Some(RefPickerEffect::Selected("origin/main".to_string()))
        );
    }

    #[test]
    fn explicit_remote_alias_remains_visible_when_the_local_name_exists() {
        let refs = RefSnapshot::from_parts(
            ["alpha", "release"],
            ["origin/release"],
            Some("origin/release"),
            Some("release"),
            true,
        );
        let mut picker = RefPicker::new(refs, Some("origin/release"));

        let selected = &picker.candidates()[picker.selection()];
        assert_eq!(selected.name, "origin/release");
        assert_eq!(
            picker.handle_key(RefPickerKey::Enter),
            Some(RefPickerEffect::Selected("origin/release".to_string()))
        );
    }
}
