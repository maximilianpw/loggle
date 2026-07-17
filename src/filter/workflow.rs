use crate::model::{Level, LogProperty};

use super::{LogFilter, PropertyFilterId, PropertyFilterUpdate, PropertyPredicate};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterEdit {
    Text,
    Source,
    Level,
    IncludeProperty,
    ExcludeProperty,
    EditPropertyFilter,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyFilterRow {
    pub id: PropertyFilterId,
    pub kind: &'static str,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterPresetRow {
    pub index: usize,
    pub name: String,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FilterPreset {
    name: String,
    filters: LogFilter,
}

#[derive(Debug, Default)]
pub struct FilterWorkflow {
    filters: LogFilter,
    history: Vec<LogFilter>,
    presets: Vec<FilterPreset>,
}

impl FilterWorkflow {
    pub fn filters(&self) -> &LogFilter {
        &self.filters
    }

    #[cfg(test)]
    pub fn filters_mut(&mut self) -> &mut LogFilter {
        &mut self.filters
    }

    #[cfg(test)]
    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    pub fn prompt_value(&self, edit: FilterEdit, property: Option<&LogProperty>) -> String {
        match edit {
            FilterEdit::Text => self.filters.text.clone().unwrap_or_default(),
            FilterEdit::Source => self.filters.source.clone().unwrap_or_default(),
            FilterEdit::Level => self
                .filters
                .level
                .map(|level| level.to_string())
                .unwrap_or_default(),
            FilterEdit::IncludeProperty | FilterEdit::ExcludeProperty => {
                property.map(property_prompt_value).unwrap_or_default()
            }
            FilterEdit::EditPropertyFilter => String::new(),
        }
    }

    pub fn apply_prompt(
        &mut self,
        edit: FilterEdit,
        value: &str,
        editing_property_filter: Option<PropertyFilterId>,
    ) {
        let value = value.trim().to_string();
        let previous_filters = self.filters.clone();
        match edit {
            FilterEdit::Text => self.filters.text = (!value.is_empty()).then_some(value),
            FilterEdit::Source => self.filters.source = (!value.is_empty()).then_some(value),
            FilterEdit::Level => {
                self.filters.level = if value.is_empty() {
                    None
                } else {
                    Level::parse(&value)
                };
            }
            FilterEdit::IncludeProperty => {
                if let Some(update) = PropertyFilterUpdate::parse(&value, false) {
                    self.filters.add_property_filter(update);
                }
            }
            FilterEdit::ExcludeProperty => {
                if let Some(update) = PropertyFilterUpdate::parse(&value, true) {
                    self.filters.add_property_filter(update);
                }
            }
            FilterEdit::EditPropertyFilter => {
                if let Some(update) = PropertyFilterUpdate::parse(&value, false) {
                    if let Some(id) = editing_property_filter {
                        self.filters.replace_property_filter(id, update);
                    }
                }
            }
        }

        self.remember_change(previous_filters);
    }

    pub fn clear(&mut self) {
        let previous_filters = self.filters.clone();
        self.filters.clear();
        self.remember_change(previous_filters);
    }

    pub fn follow_property(&mut self, property: &LogProperty) {
        let previous_filters = self.filters.clone();
        let predicate =
            PropertyPredicate::exact(&property.key, property.value.as_display_str().into_owned());
        self.filters.add_property_filter(PropertyFilterUpdate {
            exclude: false,
            predicate,
        });
        self.remember_change(previous_filters);
    }

    pub fn replace_source_from_facet(&mut self, source: &str) -> bool {
        if self
            .filters
            .source
            .as_deref()
            .is_some_and(|current| current.eq_ignore_ascii_case(source))
        {
            return false;
        }

        let previous_filters = self.filters.clone();
        self.filters.source = Some(source.to_string());
        self.remember_change(previous_filters);
        true
    }

    pub fn replace_level_from_facet(&mut self, level: Level) -> bool {
        if self.filters.level == Some(level) {
            return false;
        }

        let previous_filters = self.filters.clone();
        self.filters.level = Some(level);
        self.remember_change(previous_filters);
        true
    }

    pub fn replace_property_value_from_facet(&mut self, key: &str, value: &str) -> bool {
        let matching_includes = self
            .filters
            .property_includes
            .iter()
            .filter(|predicate| predicate.key == key)
            .collect::<Vec<_>>();
        let has_matching_exclude = self
            .filters
            .property_excludes
            .iter()
            .any(|predicate| predicate.key == key);
        if !has_matching_exclude
            && matching_includes.len() == 1
            && matching_includes[0].value.as_deref() == Some(value)
        {
            return false;
        }

        let previous_filters = self.filters.clone();
        self.filters
            .property_includes
            .retain(|predicate| predicate.key != key);
        self.filters
            .property_excludes
            .retain(|predicate| predicate.key != key);
        self.filters.add_property_filter(PropertyFilterUpdate {
            exclude: false,
            predicate: PropertyPredicate::exact(key, value),
        });
        self.remember_change(previous_filters);
        true
    }

    pub fn undo(&mut self) -> bool {
        let Some(filters) = self.history.pop() else {
            return false;
        };

        self.filters = filters;
        true
    }

    pub fn save_preset(&mut self) {
        if !self.filters.has_active_filters() {
            return;
        }

        let preset = FilterPreset {
            name: format!("Preset {}", self.presets.len() + 1),
            filters: self.filters.clone(),
        };
        if self
            .presets
            .iter()
            .any(|existing| existing.filters == preset.filters)
        {
            return;
        }

        self.presets.push(preset);
    }

    pub fn apply_preset(&mut self, index: usize) -> bool {
        let Some(preset) = self.presets.get(index) else {
            return false;
        };

        let previous_filters = self.filters.clone();
        self.filters = preset.filters.clone();
        self.remember_change(previous_filters);
        true
    }

    pub fn delete_preset(&mut self, index: usize) -> bool {
        if index >= self.presets.len() {
            return false;
        }

        self.presets.remove(index);
        true
    }

    pub fn property_filter(&self, id: PropertyFilterId) -> Option<&PropertyPredicate> {
        self.filters.property_filter(id)
    }

    pub fn remove_property_filter(&mut self, id: PropertyFilterId) -> Option<PropertyPredicate> {
        let previous_filters = self.filters.clone();
        let removed = self.filters.remove_property_filter(id);
        self.remember_change(previous_filters);
        removed
    }

    pub fn property_filter_rows(&self, query: &str) -> Vec<PropertyFilterRow> {
        self.all_property_filter_rows()
            .into_iter()
            .filter(|row| property_filter_row_matches(row, query))
            .collect()
    }

    pub fn property_filter_row_count(&self, query: &str) -> usize {
        self.all_property_filter_rows()
            .into_iter()
            .filter(|row| property_filter_row_matches(row, query))
            .count()
    }

    pub fn filter_preset_rows(&self, query: &str) -> Vec<FilterPresetRow> {
        self.presets
            .iter()
            .enumerate()
            .map(|(index, preset)| FilterPresetRow {
                index,
                name: preset.name.clone(),
                summary: filter_summary(&preset.filters),
            })
            .filter(|row| {
                query.is_empty()
                    || super::contains_ignore_ascii_case(&row.name, query)
                    || super::contains_ignore_ascii_case(&row.summary, query)
            })
            .collect()
    }

    pub fn filter_preset_row_count(&self, query: &str) -> usize {
        self.presets
            .iter()
            .filter(|preset| {
                query.is_empty()
                    || super::contains_ignore_ascii_case(&preset.name, query)
                    || super::contains_ignore_ascii_case(&filter_summary(&preset.filters), query)
            })
            .count()
    }

    fn all_property_filter_rows(&self) -> Vec<PropertyFilterRow> {
        self.filters
            .property_includes
            .iter()
            .enumerate()
            .map(|(index, predicate)| PropertyFilterRow {
                id: PropertyFilterId {
                    exclude: false,
                    index,
                },
                kind: "show",
                summary: predicate.summary_for(false),
            })
            .chain(
                self.filters
                    .property_excludes
                    .iter()
                    .enumerate()
                    .map(|(index, predicate)| PropertyFilterRow {
                        id: PropertyFilterId {
                            exclude: true,
                            index,
                        },
                        kind: "ignore",
                        summary: predicate.summary_for(true),
                    }),
            )
            .collect()
    }

    fn remember_change(&mut self, previous_filters: LogFilter) {
        if previous_filters != self.filters {
            self.history.push(previous_filters);
        }
    }
}

fn property_prompt_value(property: &LogProperty) -> String {
    format!("{}={}", property.key, property.value)
}

fn property_filter_row_matches(row: &PropertyFilterRow, query: &str) -> bool {
    query.is_empty()
        || super::contains_ignore_ascii_case(row.kind, query)
        || super::contains_ignore_ascii_case(&row.summary, query)
}

fn filter_summary(filters: &LogFilter) -> String {
    let mut parts = Vec::new();
    if let Some(text) = filters.text.as_ref().filter(|text| !text.is_empty()) {
        parts.push(format!("search={text}"));
    }
    if let Some(source) = filters.source.as_ref().filter(|source| !source.is_empty()) {
        parts.push(format!("source={source}"));
    }
    if let Some(level) = filters.level {
        parts.push(format!("level={level}"));
    }
    parts.extend(
        filters
            .property_includes
            .iter()
            .map(|predicate| format!("show {}", predicate.summary())),
    );
    parts.extend(
        filters
            .property_excludes
            .iter()
            .map(|predicate| format!("hide {}", predicate.exclude_summary())),
    );

    if parts.is_empty() {
        "-".to_string()
    } else {
        parts.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{LogProperty, PropertyValue};

    fn property(key: &str, value: &str) -> LogProperty {
        LogProperty {
            key: key.to_string(),
            value: PropertyValue::String(value.to_string()),
        }
    }

    #[test]
    fn applies_prompt_edits_and_remembers_previous_filters() {
        let mut workflow = FilterWorkflow::default();

        workflow.apply_prompt(FilterEdit::Text, "database", None);
        workflow.apply_prompt(FilterEdit::Level, "error", None);

        assert_eq!(workflow.filters().text.as_deref(), Some("database"));
        assert_eq!(workflow.filters().level, Some(Level::Error));
        assert_eq!(workflow.history_len(), 2);
    }

    #[test]
    fn follows_selected_property_as_include_filter() {
        let mut workflow = FilterWorkflow::default();

        workflow.follow_property(&property("tenantId", "tenant-1"));

        assert_eq!(workflow.filters().property_includes.len(), 1);
        assert_eq!(
            workflow.filters().property_includes[0].summary(),
            "tenantId=tenant-1"
        );
    }

    #[test]
    fn facet_source_and_level_replacements_are_undoable_and_semantic_no_ops() {
        let mut workflow = FilterWorkflow::default();
        assert!(workflow.replace_source_from_facet("API"));
        assert_eq!(workflow.history_len(), 1);
        assert!(!workflow.replace_source_from_facet("api"));
        assert_eq!(workflow.history_len(), 1);

        assert!(workflow.replace_level_from_facet(Level::Error));
        assert_eq!(workflow.history_len(), 2);
        assert!(!workflow.replace_level_from_facet(Level::Error));
        assert_eq!(workflow.history_len(), 2);

        assert!(workflow.undo());
        assert_eq!(workflow.filters().level, None);
        assert_eq!(workflow.filters().source.as_deref(), Some("API"));
    }

    #[test]
    fn facet_property_replacement_ignores_vector_order_for_semantic_no_op() {
        let mut workflow = FilterWorkflow::default();
        workflow.filters.property_includes = vec![
            PropertyPredicate::exact("region", "eu"),
            PropertyPredicate::exact("tenant", "one"),
            PropertyPredicate::exact("mode", "prod"),
        ];

        assert!(!workflow.replace_property_value_from_facet("tenant", "one"));
        assert_eq!(workflow.history_len(), 0);

        workflow
            .filters
            .property_excludes
            .push(PropertyPredicate::exact("tenant", "two"));
        assert!(workflow.replace_property_value_from_facet("tenant", "one"));
        assert_eq!(workflow.history_len(), 1);
        assert_eq!(
            workflow
                .filters()
                .property_includes
                .iter()
                .filter(|predicate| predicate.key == "tenant")
                .collect::<Vec<_>>(),
            vec![&PropertyPredicate::exact("tenant", "one")]
        );
        assert!(
            workflow
                .filters()
                .property_excludes
                .iter()
                .all(|predicate| predicate.key != "tenant")
        );
    }

    #[test]
    fn preset_rows_summarize_saved_filters() {
        let mut workflow = FilterWorkflow::default();
        workflow.apply_prompt(FilterEdit::Text, "database", None);

        workflow.save_preset();
        let rows = workflow.filter_preset_rows("");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Preset 1");
        assert_eq!(rows[0].summary, "search=database");
    }
}
