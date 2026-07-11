pub use super::i2c_sht4x::{I2cSht4x as I2cSht3x, Sht4xReadings as Sht3xReadings};

pub const DETECT_ADDRESSES: &[u8] = &[0x45];
