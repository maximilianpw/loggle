use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
};

use serde::Serialize;

use crate::{
    filter::LogFilter,
    model::{Level, LogEvent, PropertyValue},
};

pub const MIN_FACET_RECORD_LIMIT: usize = 1;
pub const DEFAULT_FACET_RECORD_LIMIT: usize = 10_000;
pub const MAX_FACET_RECORD_LIMIT: usize = 100_000;
pub const MIN_FACET_BUCKET_LIMIT: usize = 1;
pub const DEFAULT_FACET_BUCKET_LIMIT: usize = 20;
pub const MAX_FACET_BUCKET_LIMIT: usize = 100;

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FacetKind {
    Source,
    Level,
    PropertyKey,
    PropertyValue,
}

impl FacetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Level => "level",
            Self::PropertyKey => "property_key",
            Self::PropertyValue => "property_value",
        }
    }
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FacetValueType {
    String,
    Number,
    Boolean,
    Null,
    Text,
}

impl FacetValueType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Number => "number",
            Self::Boolean => "boolean",
            Self::Null => "null",
            Self::Text => "text",
        }
    }
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FacetBucket {
    pub value: String,
    pub count: usize,
    pub value_types: Vec<FacetValueType>,
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FacetGroup {
    pub schema_version: u32,
    pub facet: FacetKind,
    pub property_key: Option<String>,
    pub available_records: usize,
    pub window_records: usize,
    pub window_truncated: bool,
    pub matched_records: usize,
    pub eligible_records: usize,
    pub total_buckets: usize,
    pub truncated: bool,
    pub buckets: Vec<FacetBucket>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacetOptions {
    bucket_limit: usize,
    property_key: Option<String>,
}

impl FacetOptions {
    pub fn new(
        bucket_limit: usize,
        property_key: Option<String>,
    ) -> Result<Self, FacetOptionsError> {
        if !(MIN_FACET_BUCKET_LIMIT..=MAX_FACET_BUCKET_LIMIT).contains(&bucket_limit) {
            return Err(FacetOptionsError::InvalidBucketLimit {
                value: bucket_limit,
            });
        }

        if property_key
            .as_deref()
            .is_some_and(|key| key.trim().is_empty())
        {
            return Err(FacetOptionsError::EmptyPropertyKey);
        }

        Ok(Self {
            bucket_limit,
            property_key,
        })
    }

    pub fn bucket_limit(&self) -> usize {
        self.bucket_limit
    }

    pub fn property_key(&self) -> Option<&str> {
        self.property_key.as_deref()
    }
}

impl Default for FacetOptions {
    fn default() -> Self {
        Self {
            bucket_limit: DEFAULT_FACET_BUCKET_LIMIT,
            property_key: None,
        }
    }
}

#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FacetOptionsError {
    InvalidBucketLimit { value: usize },
    EmptyPropertyKey,
}

impl fmt::Display for FacetOptionsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBucketLimit { value } => write!(
                f,
                "facet bucket limit {value} is outside {}..={}",
                MIN_FACET_BUCKET_LIMIT, MAX_FACET_BUCKET_LIMIT
            ),
            Self::EmptyPropertyKey => f.write_str("facet property key must not be empty"),
        }
    }
}

impl Error for FacetOptionsError {}

#[derive(Debug, Default)]
struct ValueCount {
    count: usize,
    value_types: BTreeSet<FacetValueType>,
}

pub(crate) fn aggregate_facets<'a, I>(
    events: I,
    record_limit: usize,
    filter: &LogFilter,
    options: &FacetOptions,
) -> Vec<FacetGroup>
where
    I: ExactSizeIterator<Item = &'a LogEvent> + DoubleEndedIterator,
{
    debug_assert!((MIN_FACET_RECORD_LIMIT..=MAX_FACET_RECORD_LIMIT).contains(&record_limit));

    let available_records = events.len();
    let window = events.rev().take(record_limit).collect::<Vec<_>>();
    let window_records = window.len();
    let window_truncated = available_records > window_records;

    let source_filter = filter.without_source();
    let level_filter = filter.without_level();
    let property_value_filter = options
        .property_key()
        .map(|key| filter.without_property_key(key));

    let mut matched_records = 0;
    let mut source_eligible_records = 0;
    let mut level_eligible_records = 0;
    let mut property_value_eligible_records = 0;
    let mut source_counts = BTreeMap::<String, usize>::new();
    let mut level_counts = [0usize; 7];
    let mut property_key_counts = BTreeMap::<String, usize>::new();
    let mut property_value_counts = BTreeMap::<String, ValueCount>::new();

    for event in window {
        let matches_complete_filter = filter.matches(event);
        if matches_complete_filter {
            matched_records += 1;
            let mut seen_keys = BTreeSet::new();
            for property in &event.properties {
                if seen_keys.insert(property.key.as_str()) {
                    *property_key_counts.entry(property.key.clone()).or_default() += 1;
                }
            }
        }

        if source_filter.matches(event) {
            source_eligible_records += 1;
            *source_counts
                .entry(event.source.to_ascii_lowercase())
                .or_default() += 1;
        }

        if level_filter.matches(event) {
            level_eligible_records += 1;
            level_counts[level_index(event.level)] += 1;
        }

        if let (Some(key), Some(value_filter)) =
            (options.property_key(), property_value_filter.as_ref())
            && value_filter.matches(event)
        {
            property_value_eligible_records += 1;
            if let Some(property) = event.property(key) {
                let value = property.value.as_display_str().into_owned();
                let entry = property_value_counts.entry(value).or_default();
                entry.count += 1;
                entry.value_types.insert(value_type(&property.value));
            }
        }
    }

    let metadata = GroupMetadata {
        available_records,
        window_records,
        window_truncated,
        matched_records,
    };
    let mut groups = vec![
        make_group(
            FacetKind::Source,
            None,
            source_eligible_records,
            sorted_count_buckets(source_counts),
            options.bucket_limit(),
            metadata,
        ),
        make_group(
            FacetKind::Level,
            None,
            level_eligible_records,
            level_buckets(level_counts),
            options.bucket_limit(),
            metadata,
        ),
        make_group(
            FacetKind::PropertyKey,
            None,
            matched_records,
            sorted_count_buckets(property_key_counts),
            options.bucket_limit(),
            metadata,
        ),
    ];

    if let Some(property_key) = options.property_key() {
        groups.push(make_group(
            FacetKind::PropertyValue,
            Some(property_key.to_string()),
            property_value_eligible_records,
            sorted_value_buckets(property_value_counts),
            options.bucket_limit(),
            metadata,
        ));
    }

    groups
}

#[derive(Debug, Clone, Copy)]
struct GroupMetadata {
    available_records: usize,
    window_records: usize,
    window_truncated: bool,
    matched_records: usize,
}

fn make_group(
    facet: FacetKind,
    property_key: Option<String>,
    eligible_records: usize,
    mut buckets: Vec<FacetBucket>,
    bucket_limit: usize,
    metadata: GroupMetadata,
) -> FacetGroup {
    let total_buckets = buckets.len();
    buckets.truncate(bucket_limit);
    FacetGroup {
        schema_version: 1,
        facet,
        property_key,
        available_records: metadata.available_records,
        window_records: metadata.window_records,
        window_truncated: metadata.window_truncated,
        matched_records: metadata.matched_records,
        eligible_records,
        total_buckets,
        truncated: buckets.len() < total_buckets,
        buckets,
    }
}

fn sorted_count_buckets(counts: BTreeMap<String, usize>) -> Vec<FacetBucket> {
    let mut buckets = counts
        .into_iter()
        .map(|(value, count)| FacetBucket {
            value,
            count,
            value_types: Vec::new(),
        })
        .collect::<Vec<_>>();
    sort_buckets(&mut buckets);
    buckets
}

fn sorted_value_buckets(counts: BTreeMap<String, ValueCount>) -> Vec<FacetBucket> {
    let mut buckets = counts
        .into_iter()
        .map(|(value, count)| FacetBucket {
            value,
            count: count.count,
            value_types: count.value_types.into_iter().collect(),
        })
        .collect::<Vec<_>>();
    sort_buckets(&mut buckets);
    buckets
}

fn sort_buckets(buckets: &mut [FacetBucket]) {
    buckets.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.value.cmp(&right.value))
    });
}

fn level_buckets(counts: [usize; 7]) -> Vec<FacetBucket> {
    const LEVELS: [Level; 7] = [
        Level::Fatal,
        Level::Error,
        Level::Warn,
        Level::Info,
        Level::Debug,
        Level::Trace,
        Level::Unknown,
    ];

    LEVELS
        .into_iter()
        .zip(counts)
        .filter(|(_, count)| *count > 0)
        .map(|(level, count)| FacetBucket {
            value: level.as_str().to_string(),
            count,
            value_types: Vec::new(),
        })
        .collect()
}

fn level_index(level: Level) -> usize {
    match level {
        Level::Fatal => 0,
        Level::Error => 1,
        Level::Warn => 2,
        Level::Info => 3,
        Level::Debug => 4,
        Level::Trace => 5,
        Level::Unknown => 6,
    }
}

fn value_type(value: &PropertyValue) -> FacetValueType {
    match value {
        PropertyValue::String(_) => FacetValueType::String,
        PropertyValue::Number(_) => FacetValueType::Number,
        PropertyValue::Bool(_) => FacetValueType::Boolean,
        PropertyValue::Null => FacetValueType::Null,
        PropertyValue::Text(_) => FacetValueType::Text,
    }
}

pub fn escape_facet_text(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());
    for character in input.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '\t' => escaped.push_str("\\t"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            character if character.is_control() => {
                use fmt::Write as _;
                let _ = write!(escaped, "\\u{{{:04X}}}", character as u32);
            }
            character => escaped.push(character),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        filter::{PropertyFilterUpdate, PropertyPredicate},
        model::LogProperty,
    };

    fn event(
        sequence: u64,
        source: &str,
        level: Level,
        message: &str,
        properties: Vec<(&str, PropertyValue)>,
    ) -> LogEvent {
        LogEvent {
            sequence,
            source: source.to_string(),
            timestamp: None,
            level,
            raw: message.to_string(),
            message: message.to_string(),
            properties: properties
                .into_iter()
                .map(|(key, value)| LogProperty {
                    key: key.to_string(),
                    value,
                })
                .collect(),
        }
    }

    fn options(limit: usize, key: Option<&str>) -> FacetOptions {
        FacetOptions::new(limit, key.map(str::to_string)).unwrap()
    }

    fn group(groups: &[FacetGroup], kind: FacetKind) -> &FacetGroup {
        groups.iter().find(|group| group.facet == kind).unwrap()
    }

    #[test]
    fn options_validate_bounds_and_keys() {
        let defaults = FacetOptions::default();
        assert_eq!(defaults.bucket_limit(), DEFAULT_FACET_BUCKET_LIMIT);
        assert_eq!(defaults.property_key(), None);
        assert!(matches!(
            FacetOptions::new(0, None),
            Err(FacetOptionsError::InvalidBucketLimit { value: 0 })
        ));
        assert!(matches!(
            FacetOptions::new(MAX_FACET_BUCKET_LIMIT + 1, None),
            Err(FacetOptionsError::InvalidBucketLimit { .. })
        ));
        assert_eq!(
            FacetOptions::new(1, Some("  ".to_string())),
            Err(FacetOptionsError::EmptyPropertyKey)
        );
        let configured = options(7, Some("tenantId"));
        assert_eq!(configured.bucket_limit(), 7);
        assert_eq!(configured.property_key(), Some("tenantId"));
    }

    #[test]
    fn fixes_newest_window_before_filtering() {
        let events = [
            event(0, "old", Level::Error, "match", vec![]),
            event(1, "new", Level::Info, "ignore", vec![]),
        ];
        let filter = LogFilter {
            text: Some("match".to_string()),
            ..LogFilter::default()
        };

        let groups = aggregate_facets(events.iter(), 1, &filter, &FacetOptions::default());
        assert_eq!(group(&groups, FacetKind::Source).available_records, 2);
        assert_eq!(group(&groups, FacetKind::Source).window_records, 1);
        assert!(group(&groups, FacetKind::Source).window_truncated);
        assert_eq!(group(&groups, FacetKind::Source).matched_records, 0);
        assert!(group(&groups, FacetKind::Source).buckets.is_empty());
    }

    #[test]
    fn applies_each_self_exclusion_while_retaining_other_filters() {
        let events = [
            event(
                0,
                "api",
                Level::Error,
                "wanted",
                vec![("tenant", PropertyValue::String("one".into()))],
            ),
            event(
                1,
                "web",
                Level::Error,
                "wanted",
                vec![("tenant", PropertyValue::String("one".into()))],
            ),
            event(
                2,
                "api",
                Level::Info,
                "wanted",
                vec![("tenant", PropertyValue::String("one".into()))],
            ),
            event(
                3,
                "api",
                Level::Error,
                "wanted",
                vec![("tenant", PropertyValue::String("two".into()))],
            ),
            event(
                4,
                "api",
                Level::Error,
                "other",
                vec![("tenant", PropertyValue::String("one".into()))],
            ),
        ];
        let filter = LogFilter {
            text: Some("wanted".into()),
            source: Some("api".into()),
            level: Some(Level::Error),
            property_includes: vec![PropertyPredicate::exact("tenant", "one")],
            ..LogFilter::default()
        };
        let groups = aggregate_facets(events.iter(), 100, &filter, &options(20, Some("tenant")));

        assert_eq!(group(&groups, FacetKind::Source).eligible_records, 2);
        assert_eq!(group(&groups, FacetKind::Level).eligible_records, 2);
        assert_eq!(group(&groups, FacetKind::PropertyKey).eligible_records, 1);
        assert_eq!(group(&groups, FacetKind::PropertyValue).eligible_records, 2);
        assert_eq!(group(&groups, FacetKind::Source).buckets.len(), 2);
        assert_eq!(group(&groups, FacetKind::Level).buckets.len(), 2);
        assert_eq!(group(&groups, FacetKind::PropertyValue).buckets.len(), 2);
    }

    #[test]
    fn counts_duplicate_keys_once_and_uses_first_value() {
        let events = [event(
            0,
            "api",
            Level::Info,
            "row",
            vec![
                ("tenant", PropertyValue::String("first".into())),
                ("tenant", PropertyValue::String("second".into())),
            ],
        )];
        let groups = aggregate_facets(
            events.iter(),
            100,
            &LogFilter::default(),
            &options(20, Some("tenant")),
        );

        assert_eq!(group(&groups, FacetKind::PropertyKey).buckets[0].count, 1);
        assert_eq!(group(&groups, FacetKind::PropertyValue).buckets.len(), 1);
        assert_eq!(
            group(&groups, FacetKind::PropertyValue).buckets[0].value,
            "first"
        );
    }

    #[test]
    fn groups_sources_case_insensitively_and_property_keys_exactly() {
        let events = [
            event(
                0,
                "API",
                Level::Info,
                "one",
                vec![("Tenant", PropertyValue::String("one".into()))],
            ),
            event(
                1,
                "api",
                Level::Info,
                "two",
                vec![("tenant", PropertyValue::String("two".into()))],
            ),
        ];
        let groups = aggregate_facets(
            events.iter(),
            100,
            &LogFilter::default(),
            &options(20, None),
        );
        assert_eq!(group(&groups, FacetKind::Source).buckets[0].value, "api");
        assert_eq!(group(&groups, FacetKind::Source).buckets[0].count, 2);
        assert_eq!(
            group(&groups, FacetKind::PropertyKey)
                .buckets
                .iter()
                .map(|bucket| bucket.value.as_str())
                .collect::<Vec<_>>(),
            vec!["Tenant", "tenant"]
        );
    }

    #[test]
    fn reports_all_types_for_display_collisions_in_stable_order() {
        let events = [
            event(
                0,
                "a",
                Level::Info,
                "",
                vec![("v", PropertyValue::String("1".into()))],
            ),
            event(
                1,
                "a",
                Level::Info,
                "",
                vec![("v", PropertyValue::Number("1".into()))],
            ),
            event(
                2,
                "a",
                Level::Info,
                "",
                vec![("v", PropertyValue::Text("1".into()))],
            ),
            event(
                3,
                "a",
                Level::Info,
                "",
                vec![("v", PropertyValue::Bool(true))],
            ),
            event(
                4,
                "a",
                Level::Info,
                "",
                vec![("v", PropertyValue::String("true".into()))],
            ),
            event(5, "a", Level::Info, "", vec![("v", PropertyValue::Null)]),
            event(
                6,
                "a",
                Level::Info,
                "",
                vec![("v", PropertyValue::Text("null".into()))],
            ),
        ];
        let groups = aggregate_facets(
            events.iter(),
            100,
            &LogFilter::default(),
            &options(20, Some("v")),
        );
        let values = &group(&groups, FacetKind::PropertyValue).buckets;
        assert_eq!(values[0].value, "1");
        assert_eq!(
            values[0].value_types,
            vec![
                FacetValueType::String,
                FacetValueType::Number,
                FacetValueType::Text
            ]
        );
        assert_eq!(
            values[1].value_types,
            vec![FacetValueType::Null, FacetValueType::Text]
        );
        assert_eq!(
            values[2].value_types,
            vec![FacetValueType::String, FacetValueType::Boolean]
        );
    }

    #[test]
    fn sorts_ties_and_levels_deterministically_and_discloses_bucket_truncation() {
        let events = [
            event(0, "z", Level::Info, "", vec![]),
            event(1, "a", Level::Trace, "", vec![]),
            event(2, "z", Level::Fatal, "", vec![]),
            event(3, "a", Level::Error, "", vec![]),
        ];
        let groups = aggregate_facets(events.iter(), 100, &LogFilter::default(), &options(1, None));
        let source = group(&groups, FacetKind::Source);
        assert_eq!(source.total_buckets, 2);
        assert!(source.truncated);
        assert_eq!(source.buckets[0].value, "a");
        let levels = group(&groups, FacetKind::Level);
        assert_eq!(levels.total_buckets, 4);
        assert!(levels.truncated);
        assert_eq!(levels.buckets[0].value, "fatal");
    }

    #[test]
    fn emits_empty_requested_groups() {
        let events = Vec::<LogEvent>::new();
        let groups = aggregate_facets(
            events.iter(),
            100,
            &LogFilter::default(),
            &options(20, Some("tenant")),
        );
        assert_eq!(groups.len(), 4);
        assert!(groups.iter().all(|group| group.buckets.is_empty()));
        assert!(groups.iter().all(|group| group.available_records == 0));
    }

    #[test]
    fn escaping_distinguishes_literal_sequences_and_controls() {
        assert_eq!(escape_facet_text(r"literal\n"), r"literal\\n");
        assert_eq!(escape_facet_text("actual\n"), r"actual\n");
        assert_eq!(escape_facet_text("a\tb\rc"), r"a\tb\rc");
        assert_eq!(escape_facet_text("control\u{0007}"), r"control\u{0007}");
    }

    #[test]
    fn property_value_self_exclusion_retains_other_property_predicates() {
        let events = [
            event(
                0,
                "api",
                Level::Info,
                "",
                vec![
                    ("tenant", PropertyValue::String("one".into())),
                    ("region", PropertyValue::String("eu".into())),
                ],
            ),
            event(
                1,
                "api",
                Level::Info,
                "",
                vec![
                    ("tenant", PropertyValue::String("two".into())),
                    ("region", PropertyValue::String("us".into())),
                ],
            ),
        ];
        let mut filter = LogFilter::default();
        filter.add_property_filter(PropertyFilterUpdate {
            exclude: false,
            predicate: PropertyPredicate::exact("tenant", "one"),
        });
        filter.add_property_filter(PropertyFilterUpdate {
            exclude: false,
            predicate: PropertyPredicate::exact("region", "eu"),
        });
        let groups = aggregate_facets(events.iter(), 100, &filter, &options(20, Some("tenant")));
        let values = group(&groups, FacetKind::PropertyValue);
        assert_eq!(values.eligible_records, 1);
        assert_eq!(values.buckets[0].value, "one");
    }
}
