pub mod config;
pub mod error;
pub mod extract;
pub mod home;
pub mod lock;
pub mod ops;
pub mod platform;
pub mod registry;
pub mod remote;
pub mod shell;
pub mod store;

pub use error::{IkkError, Result};
pub use home::IkkHome;
