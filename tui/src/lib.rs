// Messages are moved through channels once and dropped; boxing them buys nothing measurable.
#![allow(clippy::large_enum_variant, clippy::result_large_err)]

pub mod app;
pub mod config;
pub mod keymap;
pub mod model;
pub mod net;
pub mod offer;
pub mod playback;
pub mod protocol;
pub mod sanitize;
pub mod terminal;
pub mod text;
pub mod theme;
pub mod update;
pub mod view;
pub mod waterfall;
