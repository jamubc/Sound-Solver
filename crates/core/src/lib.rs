//! `exhaust-core`: gas dynamics and acoustics of vehicle exhaust systems.

pub mod audio;
pub mod edit;
pub mod elements;
pub mod engine;
pub mod error;
pub mod fabricate;
pub mod fourpole;
pub mod gas;
pub mod gas1d;
pub mod geometry;
pub mod inputs;
pub mod layout;
pub mod manifest;
pub mod math;
pub mod measure;
pub mod metrics;
pub mod model;
pub mod preview;
pub mod project;
pub mod radiation;
pub mod render;
pub mod scan;
pub mod sensitivity;
pub mod solid;
pub mod solve;
pub mod spectrum;
pub mod thermal;
pub mod tune;
pub mod validation;

pub use error::{Error, Result};

/// Hash of this build's solver source (`build.rs`): a result made by another build of the
/// solver is never taken for this one's.
pub const SOURCE: &str = env!("EXHAUST_CORE_SOURCE");
