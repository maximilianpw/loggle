#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct SearchableListState {
    query: String,
    selected: usize,
}

impl SearchableListState {
    pub(super) fn selected(&self) -> usize {
        self.selected
    }

    pub(super) fn query(&self) -> &str {
        &self.query
    }

    pub(super) fn move_down(&mut self, amount: usize, len: usize) {
        self.selected = self.selected.saturating_add(amount);
        self.sync(len);
    }

    pub(super) fn move_up(&mut self, amount: usize) {
        self.selected = self.selected.saturating_sub(amount);
    }

    pub(super) fn push_query_char(&mut self, value: char) {
        self.query.push(value);
    }

    pub(super) fn pop_query_char(&mut self) {
        self.query.pop();
    }

    pub(super) fn sync(&mut self, len: usize) {
        self.selected = self.selected.min(len.saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_moves_and_clamps_to_available_rows() {
        let mut state = SearchableListState::default();

        state.move_down(2, 5);
        assert_eq!(state.selected(), 2);

        state.move_down(usize::MAX, 5);
        assert_eq!(state.selected(), 4);

        state.move_up(usize::MAX);
        assert_eq!(state.selected(), 0);

        state.move_down(3, 0);
        assert_eq!(state.selected(), 0);
    }

    #[test]
    fn query_mutation_preserves_selection_until_synced() {
        let mut state = SearchableListState::default();
        state.move_down(2, 5);

        state.push_query_char('r');
        state.push_query_char('e');
        assert_eq!(state.query(), "re");
        assert_eq!(state.selected(), 2);

        state.pop_query_char();
        assert_eq!(state.query(), "r");

        state.sync(1);
        assert_eq!(state.selected(), 0);
    }
}
