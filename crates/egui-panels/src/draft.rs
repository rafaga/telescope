//! A value being edited next to its saved version.

/// The saved value and the copy being edited, so a settings screen knows what
/// changed, which pages to mark and what to restore on Cancel.
///
/// ```
/// use egui_panels::Draft;
///
/// #[derive(Clone, PartialEq)]
/// struct Settings { volume: u8, language: String }
///
/// let mut draft = Draft::new(Settings { volume: 5, language: "en".into() });
/// draft.current_mut().volume = 7;
/// assert!(draft.is_dirty());
/// assert!(draft.differs(|s| s.volume));      // the "Sound" page gets a dot
/// assert!(!draft.differs(|s| s.language.clone()));
/// draft.revert();
/// assert_eq!(draft.current().volume, 5);
/// ```
#[derive(Clone, Debug, Default)]
pub struct Draft<T> {
    saved: T,
    current: T,
}

impl<T: Clone> Draft<T> {
    /// A draft of `value`, with nothing changed yet.
    pub fn new(value: T) -> Self {
        Self {
            current: value.clone(),
            saved: value,
        }
    }

    /// The value being edited.
    pub fn current(&self) -> &T {
        &self.current
    }

    /// The value being edited, to change it.
    pub fn current_mut(&mut self) -> &mut T {
        &mut self.current
    }

    /// The last saved value.
    pub fn saved(&self) -> &T {
        &self.saved
    }

    /// Makes the edited value the saved one (after writing it somewhere).
    pub fn commit(&mut self) {
        self.saved = self.current.clone();
    }

    /// Throws the edits away.
    pub fn revert(&mut self) {
        self.current = self.saved.clone();
    }

    /// Replaces both copies: the value changed outside the editor.
    pub fn reset(&mut self, value: T) {
        self.current = value.clone();
        self.saved = value;
    }

    /// Whether the part of the value `part` picks differs from the saved one:
    /// one call per page tells which pages have changes.
    pub fn differs<V: PartialEq>(&self, part: impl Fn(&T) -> V) -> bool {
        part(&self.current) != part(&self.saved)
    }
}

impl<T: Clone + PartialEq> Draft<T> {
    /// Whether anything changed since the last commit.
    pub fn is_dirty(&self) -> bool {
        self.current != self.saved
    }
}

#[cfg(test)]
mod tests {
    use super::Draft;

    #[test]
    fn commit_and_revert() {
        let mut draft = Draft::new(1);
        assert!(!draft.is_dirty());
        *draft.current_mut() = 2;
        assert!(draft.is_dirty());
        draft.commit();
        assert!(!draft.is_dirty());
        assert_eq!(*draft.saved(), 2);
        *draft.current_mut() = 3;
        draft.revert();
        assert_eq!(*draft.current(), 2);
        draft.reset(9);
        assert_eq!((*draft.current(), *draft.saved()), (9, 9));
    }

    #[test]
    fn differs_looks_at_one_part() {
        let mut draft = Draft::new((1, "a"));
        draft.current_mut().0 = 2;
        assert!(draft.differs(|value| value.0));
        assert!(!draft.differs(|value| value.1));
    }
}
