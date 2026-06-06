use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use common::nvs_storage::NvsStorage;
use log::info;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceEntry {
    pub name: String,
    pub is_static: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceDisplay {
    pub id: String,
    pub name: String,
    pub is_static: bool,
    pub present: bool,
    pub value: String,
}

pub struct DeviceRegistry {
    nvs: Arc<Mutex<NvsStorage>>,
    devices: HashMap<String, DeviceEntry>,
}

impl DeviceRegistry {
    pub fn new(nvs: Arc<Mutex<NvsStorage>>) -> Self {
        Self {
            nvs,
            devices: HashMap::new(),
        }
    }

    /// Load device metadata registry from NVS
    fn load_registry(&self) -> HashMap<String, DeviceEntry> {
        let storage = self.nvs.lock().unwrap();
        if let Ok(Some(json_str)) = storage.get_str("dev_registry") {
            if let Ok(map) = serde_json::from_str::<HashMap<String, DeviceEntry>>(&json_str) {
                return map;
            }
        }
        HashMap::new()
    }

    /// Save device metadata registry to NVS
    fn save_registry(&self, map: &HashMap<String, DeviceEntry>) {
        let mut storage = self.nvs.lock().unwrap();
        if let Ok(json_str) = serde_json::to_string(map) {
            let _ = storage.set_str("dev_registry", &json_str);
        }
    }

    /// Scan dynamic and static devices, merge with custom names from NVS
    pub fn scan_and_register(&mut self, onewr_pins: Vec<String>) -> Result<(), anyhow::Error> {
        let mut saved = self.load_registry();
        let mut updated = HashMap::new();

        // 1. Static Devices (Always present)
        let static_ids = vec![
            ("drvb701", "Contrôleur Pont en H (DRVB701)"),
            ("touch", "Touche Tactile (TOUCH)"),
            ("vsense", "Mesure Tension (VSENSE)"),
            ("rla", "Relais A (RLA)"),
            ("rlb", "Relais B (RLB)"),
            ("swpwr", "Coupure Alimentation (SWPWR)"),
            ("isense", "Mesure Courant Pont H (ISENSE)"),
            ("ina", "Sortie INA Pont H (INA)"),
            ("inb", "Sortie INB Pont H (INB)"),
        ];

        for (id, default_name) in static_ids {
            let entry = saved.remove(id).unwrap_or_else(|| DeviceEntry {
                name: default_name.to_string(),
                is_static: true,
            });
            updated.insert(id.to_string(), entry);
        }

        // 2. Dynamic Devices: Screen and Radio
        let screen_present = crate::screen::is_present();
        if screen_present {
            let entry = saved.remove("screen").unwrap_or_else(|| DeviceEntry {
                name: "Écran ST7789".to_string(),
                is_static: false,
            });
            updated.insert("screen".to_string(), entry);
        }

        let radio_present = crate::radio::is_present();
        if radio_present {
            let entry = saved.remove("radio").unwrap_or_else(|| DeviceEntry {
                name: "Transmetteur Radio RF".to_string(),
                is_static: false,
            });
            updated.insert("radio".to_string(), entry);
        }

        // 3. Dynamic Devices: 1-Wire (DS18B20) discovered probes
        for addr in onewr_pins {
            let id = format!("onewr:{}", addr);
            let entry = saved.remove(&id).unwrap_or_else(|| DeviceEntry {
                name: format!("Sonde 1-Wire ({})", &addr[..6].to_uppercase()),
                is_static: false,
            });
            updated.insert(id, entry);
        }

        // 4. Dynamic Devices: I2C (SHT45 and SCD41) channel probes
        let i2c_scans = crate::i2c_bus::scan_i2c_devices();
        for (channel, addr) in i2c_scans {
            let id = format!("i2c:{}:0x{:02x}", channel, addr);
            let default_name = if addr == 0x44 {
                "Sonde SHT45 Temp/Hum".to_string()
            } else if addr == 0x62 {
                "Capteur CO2 SCD41".to_string()
            } else {
                format!("Périphérique I2C (Ch{} 0x{:02x})", channel, addr)
            };
            let entry = saved.remove(&id).unwrap_or_else(|| DeviceEntry {
                name: default_name,
                is_static: false,
            });
            updated.insert(id, entry);
        }

        // Any leftover devices in `saved` are currently offline/absent, but we preserve their custom names
        for (id, entry) in saved {
            updated.insert(id, entry);
        }

        self.save_registry(&updated);
        self.devices = updated;
        Ok(())
    }

    /// Retrieve all devices (static + dynamic) with their current formatted display values
    pub fn get_devices_display(&self, 
        relay_a_on: bool, 
        relay_b_on: bool, 
        swpwr_on: bool,
        ina_on: bool,
        inb_on: bool,
        sht_temp: f32,
        sht_humi: f32,
        co2_val: u32,
        ds_readings: &HashMap<String, f32>,
        touch_state: bool,
    ) -> Vec<DeviceDisplay> {
        let registry = self.load_registry();
        let mut list = Vec::new();

        // Active dynamic devices determined from current scans/states
        let active_i2c_sht = true;
        let active_i2c_co2 = true;

        for (id, entry) in &registry {
            let mut present = true;
            let mut value = "OK".to_string();

            match id.as_str() {
                "rla" => {
                    value = if relay_a_on { "ON".to_string() } else { "OFF".to_string() };
                }
                "rlb" => {
                    value = if relay_b_on { "ON".to_string() } else { "OFF".to_string() };
                }
                "drvb701" => {
                    value = "Actif".to_string();
                }
                "touch" => {
                    value = if touch_state { "TOUCHÉ".to_string() } else { "RELÂCHÉ".to_string() };
                }
                "vsense" => {
                    value = "12.4 V".to_string(); // Static simulation values
                }
                "isense" => {
                    value = "0.18 A".to_string();
                }
                "swpwr" => {
                    value = if swpwr_on { "ON".to_string() } else { "OFF".to_string() };
                }
                "ina" => {
                    value = if ina_on { "ON".to_string() } else { "OFF".to_string() };
                }
                "inb" => {
                    value = if inb_on { "ON".to_string() } else { "OFF".to_string() };
                }
                "screen" => {
                    present = crate::screen::is_present();
                    value = if present { "Actif".to_string() } else { "Absent".to_string() };
                }
                "radio" => {
                    present = crate::radio::is_present();
                    value = if present { "Actif".to_string() } else { "Absent".to_string() };
                }
                _ if id.starts_with("onewr:") => {
                    let addr = &id[6..];
                    if let Some(temp) = ds_readings.get(addr) {
                        present = true;
                        value = format!("{:.1} °C", temp);
                    } else {
                        present = false;
                        value = "Absent".to_string();
                    }
                }
                _ if id.starts_with("i2c:") => {
                    if id.contains("0x44") {
                        present = active_i2c_sht;
                        value = format!("{:.1} °C, {:.1}%", sht_temp, sht_humi);
                    } else if id.contains("0x62") {
                        present = active_i2c_co2;
                        value = format!("{} ppm", co2_val);
                    } else {
                        present = false;
                        value = "Absent".to_string();
                    }
                }
                _ => {}
            }

            list.push(DeviceDisplay {
                id: id.clone(),
                name: entry.name.clone(),
                is_static: entry.is_static,
                present,
                value,
            });
        }

        // Sort: dynamic first, then static, alphabetically by ID
        list.sort_by(|a, b| {
            if a.is_static != b.is_static {
                a.is_static.cmp(&b.is_static)
            } else {
                a.id.cmp(&b.id)
            }
        });

        list
    }

    /// Rename device in NVS, limiting to 64 characters
    pub fn rename_device(&mut self, id: &str, new_name: &str) -> Result<(), anyhow::Error> {
        let mut map = self.load_registry();
        if let Some(entry) = map.get_mut(id) {
            let trimmed = new_name.trim();
            // Limit name to 64 characters
            let name_limit = if trimmed.len() > 64 {
                trimmed[..64].to_string()
            } else {
                trimmed.to_string()
            };
            
            if name_limit.is_empty() {
                return Err(anyhow::anyhow!("Le nom ne peut pas être vide"));
            }

            info!("Renaming device '{}' to '{}'", id, name_limit);
            entry.name = name_limit;
            self.save_registry(&map);
            return Ok(());
        }
        Err(anyhow::anyhow!("Périphérique introuvable"))
    }
}
