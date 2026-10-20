// Messages are moved through channels once and dropped; boxing them buys nothing measurable.
#![allow(clippy::large_enum_variant, clippy::result_large_err)]

pub mod keymap;
pub mod model;
pub mod protocol;
pub mod sanitize;
pub mod update;
