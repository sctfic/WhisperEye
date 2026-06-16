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
    /// Métadonnées du capteur (incertitude, plage, unité) — None pour les actuateurs
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sensor_meta: Option<SensorMeta>,
    /// Formule de correction stockée en NVS (par défaut "x")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correction_formula: Option<String>,
}

/// Métadonnées techniques d'un capteur (issues des documentations officielles)
#[derive(Debug, Clone, Serialize)]
pub struct SensorMeta {
    pub unit: String,
    pub uncertainty: String,
    pub range_min: f64,
    pub range_max: f64,
}

/// Retourne les métadonnées pour un capteur donné (en dur, doc constructeur)
pub fn get_sensor_meta(device_id: &str) -> Option<SensorMeta> {
    match device_id {
        // SHT45 (Sensirion) — https://sensirion.com/products/catalog/SHT45
        id if id.ends_with("_T") && id.contains("0x44") => Some(SensorMeta {
            unit: "°C".to_string(),
            uncertainty: "±0.1°C (20-60°C)".to_string(),
            range_min: -40.0,
            range_max: 125.0,
        }),
        id if id.ends_with("_H") && id.contains("0x44") => Some(SensorMeta {
            unit: "%RH".to_string(),
            uncertainty: "±1.0%RH (20-80%RH)".to_string(),
            range_min: 0.0,
            range_max: 100.0,
        }),
        // SCD41 (Sensirion) — https://sensirion.com/products/catalog/SCD41
        id if id.contains("0x62") => Some(SensorMeta {
            unit: "ppm".to_string(),
            uncertainty: "±40 ppm + 5% m.v.".to_string(),
            range_min: 0.0,
            range_max: 40000.0,
        }),
        // DS18B20 (Maxim) — 1-Wire temperature probes
        id if id.starts_with("onewr:") => Some(SensorMeta {
            unit: "°C".to_string(),
            uncertainty: "±0.5°C (-10 à +85°C)".to_string(),
            range_min: -55.0,
            range_max: 125.0,
        }),
        // BME280 (Bosch) — capteurs futurs
        id if id.ends_with("_T") && (id.contains("0x76") || id.contains("0x77")) => Some(SensorMeta {
            unit: "°C".to_string(),
            uncertainty: "±1.0°C".to_string(),
            range_min: -40.0,
            range_max: 85.0,
        }),
        id if id.ends_with("_H") && (id.contains("0x76") || id.contains("0x77")) => Some(SensorMeta {
            unit: "%RH".to_string(),
            uncertainty: "±3%RH".to_string(),
            range_min: 0.0,
            range_max: 100.0,
        }),
        id if id.ends_with("_P") && (id.contains("0x76") || id.contains("0x77")) => Some(SensorMeta {
            unit: "hPa".to_string(),
            uncertainty: "±1.0 hPa".to_string(),
            range_min: 300.0,
            range_max: 1100.0,
        }),
        // Capteurs internes
        "vsense" => Some(SensorMeta {
            unit: "V".to_string(),
            uncertainty: "±0.1V".to_string(),
            range_min: 0.0,
            range_max: 30.0,
        }),
        "isense" => Some(SensorMeta {
            unit: "A".to_string(),
            uncertainty: "±0.01A".to_string(),
            range_min: 0.0,
            range_max: 3.0,
        }),
        _ => None,
    }
}

/// Récupère la formule de correction stockée en NVS pour un capteur.
/// Clé NVS : `corr_<device_id>` (ex: corr_i2c:0:0x44_T).
/// Par défaut: "<device_id>.raw" (la valeur réelle = la valeur brute).
pub fn get_correction_formula(nvs: &Arc<Mutex<NvsStorage>>, device_id: &str) -> String {
    let storage = nvs.lock().unwrap();
    let key = format!("corr_{}", device_id);
    storage.get_str(&key).ok().flatten().unwrap_or_else(|| format!("{}.raw", device_id))
}

/// Sauvegarde la formule de correction dans la NVS.
pub fn set_correction_formula(nvs: &Arc<Mutex<NvsStorage>>, device_id: &str, formula: &str) -> Result<(), anyhow::Error> {
    let mut storage = nvs.lock().unwrap();
    let key = format!("corr_{}", device_id);
    storage.set_str(&key, formula)?;
    Ok(())
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
            if addr == 0x44 {
                // SHT45 : séparer en deux capteurs distincts (Température et Humidité)
                let id_t = format!("i2c:{}:0x{:02x}_T", channel, addr);
                let entry_t = saved.remove(&id_t).unwrap_or_else(|| DeviceEntry {
                    name: "SHT45-Temp".to_string(),
                    is_static: false,
                });
                updated.insert(id_t, entry_t);

                let id_h = format!("i2c:{}:0x{:02x}_H", channel, addr);
                let entry_h = saved.remove(&id_h).unwrap_or_else(|| DeviceEntry {
                    name: "SHT45-Hum".to_string(),
                    is_static: false,
                });
                updated.insert(id_h, entry_h);
            } else if addr == 0x62 {
                let id = format!("i2c:{}:0x{:02x}", channel, addr);
                let entry = saved.remove(&id).unwrap_or_else(|| DeviceEntry {
                    name: "Capteur CO2 SCD41".to_string(),
                    is_static: false,
                });
                updated.insert(id, entry);
            } else if addr == 0x76 || addr == 0x77 {
                // BME280 : séparer en trois capteurs (Température, Humidité, Pression)
                let id_t = format!("i2c:{}:0x{:02x}_T", channel, addr);
                let entry_t = saved.remove(&id_t).unwrap_or_else(|| DeviceEntry {
                    name: format!("BME280-Temp (i2c:{}:0x{:02x})", channel, addr),
                    is_static: false,
                });
                updated.insert(id_t, entry_t);

                let id_h = format!("i2c:{}:0x{:02x}_H", channel, addr);
                let entry_h = saved.remove(&id_h).unwrap_or_else(|| DeviceEntry {
                    name: format!("BME280-Hum (i2c:{}:0x{:02x})", channel, addr),
                    is_static: false,
                });
                updated.insert(id_h, entry_h);

                let id_p = format!("i2c:{}:0x{:02x}_P", channel, addr);
                let entry_p = saved.remove(&id_p).unwrap_or_else(|| DeviceEntry {
                    name: format!("BME280-Pres (i2c:{}:0x{:02x})", channel, addr),
                    is_static: false,
                });
                updated.insert(id_p, entry_p);
            } else {
                let id = format!("i2c:{}:0x{:02x}", channel, addr);
                let entry = saved.remove(&id).unwrap_or_else(|| DeviceEntry {
                    name: format!("Périphérique I2C (Ch{} 0x{:02x})", channel, addr),
                    is_static: false,
                });
                updated.insert(id, entry);
            }
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

        for (id, entry) in &registry {
            let mut present = true;
            let mut value = "OK".to_string();
            let sensor_meta = get_sensor_meta(id);
            let correction = get_correction_formula(&self.nvs, id);

            match id.as_str() {
                "rla" => {
                    value = if relay_a_on { "ON".to_string() } else { "OFF".to_string() };
                }
                "rlb" => {
                    value = if relay_b_on { "ON".to_string() } else { "OFF".to_string() };
                }
                "touch" => {
                    value = if touch_state { "TOUCHÉ".to_string() } else { "RELÂCHÉ".to_string() };
                }
                "vsense" => {
                    value = "12.4 V".to_string();
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
                        value = format!("{:.1} °C", *temp);
                    } else {
                        present = false;
                        value = "Absent".to_string();
                    }
                }
                _ if id.starts_with("i2c:") => {
                    // SHT45 Temperature (i2c:X:0x44_T)
                    if id.ends_with("_T") && id.contains("0x44") {
                        present = true;
                        value = format!("{:.1} °C", sht_temp);
                    }
                    // SHT45 Humidity (i2c:X:0x44_H)
                    else if id.ends_with("_H") && id.contains("0x44") {
                        present = true;
                        value = format!("{:.1} %", sht_humi);
                    }
                    // SCD41 CO2 (i2c:X:0x62)
                    else if id.contains("0x62") && !id.ends_with("_T") && !id.ends_with("_H") && !id.ends_with("_P") {
                        present = true;
                        value = format!("{} ppm", co2_val);
                    }
                    // BME280 Temperature / Humidity / Pressure (futur) — valeurs simulées pour l'instant
                    else if id.ends_with("_T") && (id.contains("0x76") || id.contains("0x77")) {
                        present = false; // Pas encore de capteur BME280 physique
                        value = "N/A".to_string();
                    }
                    else if id.ends_with("_H") && (id.contains("0x76") || id.contains("0x77")) {
                        present = false;
                        value = "N/A".to_string();
                    }
                    else if id.ends_with("_P") && (id.contains("0x76") || id.contains("0x77")) {
                        present = false;
                        value = "N/A".to_string();
                    }
                    else {
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
                sensor_meta,
                correction_formula: Some(correction),
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

    /// Retrieve all registered devices from NVS
    #[allow(dead_code)]
    pub fn get_devices(&self) -> HashMap<String, DeviceEntry> {
        self.load_registry()
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
