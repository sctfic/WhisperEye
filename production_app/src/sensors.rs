use serde::Serialize;
use std::collections::HashMap;
use std::time::SystemTime;
use std::sync::Mutex;
use crate::ds18b20::OneWire;

#[derive(Serialize, Debug, Clone)]
pub struct SensorReadings {
    pub temperature_sht45: f32,
    pub humidity_sht45: f32,
    pub co2_scd41: i32,
    pub ds18b20_temperatures: HashMap<String, f32>,
}

pub fn read_sensors(
    onewire_bus: Option<&Mutex<OneWire>>,
    ds18b20_probes: &[String]
) -> SensorReadings {
    let mut ds_temps = HashMap::new();

    if let Some(bus_mutex) = onewire_bus {
        if let Ok(mut bus) = bus_mutex.lock() {
            // Lancer la conversion de température sur toutes les sondes connectées
            if let Ok(()) = bus.start_conversion() {
                // Attendre la conversion (750ms). On utilise std::thread::sleep pour libérer le CPU
                std::thread::sleep(std::time::Duration::from_millis(750));

                for addr in ds18b20_probes {
                    match bus.read_temperature(addr) {
                        Ok(temp) => {
                            ds_temps.insert(addr.clone(), temp);
                        }
                        Err(e) => {
                            log::warn!("Failed to read temperature for probe {}: {:?}", addr, e);
                            ds_temps.insert(addr.clone(), -255.0);
                        }
                    }
                }
            } else {
                log::warn!("Failed to start temperature conversion on 1-Wire bus");
                for addr in ds18b20_probes {
                    ds_temps.insert(addr.clone(), -255.0);
                }
            }
        } else {
            log::warn!("Failed to lock 1-Wire bus");
            for addr in ds18b20_probes {
                ds_temps.insert(addr.clone(), -255.0);
            }
        }
    } else {
        // Fallback simulateur/mock si aucun bus physique n'est fourni
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let angle = (now % 360) as f32 * std::f32::consts::PI / 180.0;
        let temp_offset = angle.sin();

        for (i, addr) in ds18b20_probes.iter().enumerate() {
            let offset = (i as f32) * 0.15 + temp_offset * 0.8;
            ds_temps.insert(addr.clone(), 22.8 + offset);
        }
    }

    SensorReadings {
        temperature_sht45: -255.0,
        humidity_sht45: -255.0,
        co2_scd41: -255,
        ds18b20_temperatures: ds_temps,
    }
}

