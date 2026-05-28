use serde::Serialize;
use std::time::SystemTime;

#[derive(Serialize, Clone)]
pub struct SensorReadings {
    pub temperature_sht45: f32,
    pub humidity_sht45: f32,
    pub co2_scd41: u32,
    pub temperature_ds18b20: f32,
}

pub fn read_sensors() -> SensorReadings {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
        
    // Generate realistic simulated oscillations based on epoch seconds
    let angle = (now % 360) as f32 * std::f32::consts::PI / 180.0;
    
    let temp_offset = angle.sin();
    let co2_offset = angle.cos();

    SensorReadings {
        temperature_sht45: 23.5 + (temp_offset * 1.5),
        humidity_sht45: 45.2 + (temp_offset * 5.0),
        co2_scd41: (850.0 + (co2_offset * 150.0)) as u32,
        temperature_ds18b20: 22.8 + (temp_offset * 0.8),
    }
}
