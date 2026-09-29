//! `exhaust-core`: gas dynamics and acoustics of vehicle exhaust systems.

pub mod edit;
pub mod elements;
pub mod engine;
pub mod error;
pub mod fabricate;
pub mod fourpole;
pub mod gas;
pub mod gas1d;
pub mod geometry;
pub mod layout;
pub mod manifest;
pub mod math;
pub mod metrics;
pub mod model;
pub mod preview;
pub mod project;
pub mod radiation;
pub mod scan;
pub mod solid;
pub mod solve;
pub mod spectrum;
pub mod thermal;
pub mod validation;

pub use error::{Error, Result};
