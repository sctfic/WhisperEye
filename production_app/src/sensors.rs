use std::collections::HashMap;
use std::time::SystemTime;
use std::sync::Mutex;
use crate::one_wire::OneWire;

macro_rules! info {
    ($fmt:literal) => {
        log::info!(concat!("\x1b[36m", $fmt, "\x1b[0m"));
    };
    ($fmt:literal, $($arg:tt)*) => {
        log::info!(concat!("\x1b[36m", $fmt, "\x1b[0m"), $($arg)*);
    };
}
macro_rules! warn {
    ($fmt:literal) => {
        log::warn!(concat!("\x1b[36m", $fmt, "\x1b[0m"));
    };
    ($fmt:literal, $($arg:tt)*) => {
        log::warn!(concat!("\x1b[36m", $fmt, "\x1b[0m"), $($arg)*);
    };
}
macro_rules! debug {
    ($fmt:literal) => {
        log::debug!(concat!("\x1b[36m", $fmt, "\x1b[0m"));
    };
    ($fmt:literal, $($arg:tt)*) => {
        log::debug!(concat!("\x1b[36m", $fmt, "\x1b[0m"), $($arg)*);
    };
}

#[derive(Debug, Clone)]
pub struct SensorReadings {
    pub temperature_sht45: f32,
    pub humidity_sht45: f32,
    pub co2_scd41: i32,
    pub ds18b20_temperatures: HashMap<String, f32>,
}

impl serde::Serialize for SensorReadings {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        
        map.serialize_entry("i2c:0:0x44_T", &self.temperature_sht45)?;
        map.serialize_entry("i2c:0:0x44_H", &self.humidity_sht45)?;
        map.serialize_entry("i2c:0:0x62", &self.co2_scd41)?;
        
        for (addr, temp) in &self.ds18b20_temperatures {
            map.serialize_entry(&format!("onewr:{}", addr), temp)?;
        }
        
        map.end()
    }
}

pub fn read_sensors(
    onewire_bus: Option<&Mutex<OneWire>>,
    ds18b20_probes: &[String],
    names_map: &HashMap<String, String>,
) -> SensorReadings {
    let mut ds_temps = HashMap::new();

    let sensor_names: Vec<String> = ds18b20_probes.iter()
        .map(|addr| names_map.get(addr).cloned().unwrap_or_else(|| format!("Sonde Temp ({})", &addr[..std::cmp::min(addr.len(), 6)])))
        .collect();
    debug!("Demande de collecte des valeurs ({})", sensor_names.join(", "));

    if let Some(bus_mutex) = onewire_bus {
        if let Ok(mut bus) = bus_mutex.lock() {
            // Lancer la conversion de température sur toutes les sondes connectées
            if let Ok(()) = bus.start_conversion() {
                // Attendre la conversion (200ms pour résolution 10-bit). On utilise std::thread::sleep pour libérer le CPU
                std::thread::sleep(std::time::Duration::from_millis(200));

                for addr in ds18b20_probes {
                    // Délai de repos entre les transactions de capteurs distincts
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    
                    let name = names_map.get(addr).cloned().unwrap_or_else(|| format!("Sonde Temp ({})", &addr[..std::cmp::min(addr.len(), 6)]));
                    match bus.read_temperature(addr) {
                        Ok(temp) => {
                            debug!("lecture par capteur, {} : {:.2}°C", name, temp);
                            ds_temps.insert(addr.clone(), temp);
                        }
                        Err(e) => {
                            warn!("lecture par capteur en échec pour {}, erreur : {:?}", name, e);
                            ds_temps.insert(addr.clone(), -255.0);
                        }
                    }
                }
            } else {
                warn!("Failed to start temperature conversion on 1-Wire bus");
                for addr in ds18b20_probes {
                    ds_temps.insert(addr.clone(), -255.0);
                }
            }
        } else {
            warn!("Failed to lock 1-Wire bus");
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
            let temp = 22.8 + offset;
            let name = names_map.get(addr).cloned().unwrap_or_else(|| format!("Sonde Temp ({})", &addr[..std::cmp::min(addr.len(), 6)]));
            info!("lecture par capteur, {} : {:.2}°C (Simulation)", name, temp);
            ds_temps.insert(addr.clone(), temp);
        }
    }

    SensorReadings {
        temperature_sht45: -255.0,
        humidity_sht45: -255.0,
        co2_scd41: -255,
        ds18b20_temperatures: ds_temps,
    }
}
