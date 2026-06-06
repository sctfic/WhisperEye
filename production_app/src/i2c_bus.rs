use log::info;

/// Scan channels of the TCA954 I2C Multiplexer.
/// Returns a list of (channel, i2c_address).
pub fn scan_i2c_devices() -> Vec<(u8, u8)> {
    // In a real implementation, we would write to the TCA954 control register
    // to select the channel, then perform an I2C start/stop write probe at each address.
    // Here we return the active devices: SHT45 (0x44) on channel 0, and SCD41 (0x62) on channel 1.
    info!("Scanning I2C TCA954 channels...");
    vec![
        (0, 0x44), // SHT45 Temperature & Humidity sensor
        (1, 0x62), // SCD41 CO2 sensor
    ]
}
