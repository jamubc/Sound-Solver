//! `exhaust-core`: gas dynamics and acoustics of vehicle exhaust systems.

pub mod elements;
pub mod engine;
pub mod error;
pub mod fourpole;
pub mod gas;
pub mod gas1d;
pub mod geometry;
pub mod math;
pub mod metrics;
pub mod model;
pub mod preview;
pub mod project;
pub mod radiation;
pub mod solve;
pub mod spectrum;
pub mod thermal;
pub mod validation;

pub use error::{Error, Result};
