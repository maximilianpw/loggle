mod app;
mod buffer;
mod commands;
mod config;
mod facet;
mod filter;
mod model;
mod page_log;
mod runtime;
mod ui;

#[cfg(feature = "perf-harness")]
pub mod perf;

pub use config::{
    ConfigEnv, ConfigError, StartConfig, load_config_file, load_named_config, load_project_config,
    named_config_path, parse_config, project_config_path,
};
pub use facet::{
    DEFAULT_FACET_BUCKET_LIMIT, DEFAULT_FACET_RECORD_LIMIT, FacetBucket, FacetGroup, FacetKind,
    FacetOptions, FacetOptionsError, FacetValueType, MAX_FACET_BUCKET_LIMIT,
    MAX_FACET_RECORD_LIMIT, MIN_FACET_BUCKET_LIMIT, MIN_FACET_RECORD_LIMIT, escape_facet_text,
};
pub use model::{Level as LogLevel, SourceConfig};
pub use page_log::{
    ActiveLogPage, FacetQueryError, LogPageError, LogPageId, LogPageIdError, LogPageQueryOptions,
    LogPageRecord, LogPageTailOptions, active_log_pages, print_log_page_query, print_log_page_tail,
    print_log_page_tail_with_options, query_log_page_facets, query_log_page_records,
};
pub use runtime::{
    NamedCommand, ReadySpec, RuntimeConfig, RuntimeError, RuntimeInput, StartCommand, run,
};
