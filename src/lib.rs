//! Control AOC monitors' settings on Linux over DDC/CI, including the AOC-only
//! settings (shadow control, game color, overdrive, Low Blue, ...) that standard
//! tools can't reach. The `aocctl` binary is a thin CLI over this library.

pub mod ddc;
pub mod detect;
pub mod edid;
pub mod i2c;
pub mod models;
pub mod monitor;
pub mod presets;
pub mod verify;
