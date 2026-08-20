#![allow(clippy::module_name_repetitions)]
#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::case_sensitive_file_extension_comparisons,
    clippy::items_after_statements,
    clippy::cast_sign_loss,
    clippy::unreadable_literal,
    clippy::doc_markdown,
    clippy::similar_names,
    clippy::bool_to_int_with_if
)]

pub mod config;
pub mod error;
pub mod home;
pub mod lock;
pub mod ops;
pub mod platform;
pub mod processor;
pub mod progress;
pub mod registry;
pub mod remote;
pub mod shell;
pub mod source;
pub mod store;

pub use error::{IkkError, Result};
pub use home::IkkHome;
