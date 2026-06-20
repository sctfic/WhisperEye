use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use common::nvs_storage::NvsStorage;
use log::info;
use serde::{Serialize, Deserialize};

/// Static devices — toujours présents, noms en dur dans le code.
/// Ne sont JAMAIS stockés dans devicesKnow NVS.
const STATIC_DEVICES: &[(&str, &str)] = &[
    ("touch", "Touche Tactile (TOUCH)"),
    ("vsense", "Mesure Tension (VSENSE)"),
    ("rla", "Relais A (RLA)"),
    ("rlb", "Relais B (RLB)"),
    ("swpwr", "Coupure Alimentation (SWPWR)"),
    ("isense", "Mesure Courant Pont H (ISENSE)"),
    ("ina", "Sortie INA Pont H (INA)"),
    ("inb", "Sortie INB Pont H (INB)"),
];

pub fn is_static_device(id: &str) -> bool {
    STATIC_DEVICES.iter().any(|(i, _)| *i == id)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceEntry {
    pub name: String,
    pub is_static: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correction_formula: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistEntry {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    correction_formula: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceDisplay {
    pub id: String,
    pub name: String,
    pub is_static: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub present: Option<bool>,
    pub value: String,
    /// Métadonnées du capteur (incertitude, plage, unité) — None pour les actuateurs
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sensor_meta: Option<SensorMeta>,
    /// Formule de correction stockée en NVS (par défaut "x")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correction_formula: Option<String>,
    /// Planifications actives (uniquement pour les actionneurs)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedules: Option<Vec<crate::actuators::ScheduledAction>>,
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
            uncertainty: "±0.1 °C (typ., 0-75 °C)\n±0.2 °C (max.)".to_string(),
            range_min: -40.0,
            range_max: 125.0,
        }),
        id if id.ends_with("_H") && id.contains("0x44") => Some(SensorMeta {
            unit: "%RH".to_string(),
            uncertainty: "±1.0 %RH (typ., 25-75 %RH)\n±1.5 %RH (max.)".to_string(),
            range_min: 0.0,
            range_max: 100.0,
        }),
        // SCD41 (Sensirion) — https://sensirion.com/products/catalog/SCD41
        id if id.contains("0x62") => Some(SensorMeta {
            unit: "ppm".to_string(),
            uncertainty: "±(40 ppm + 5 % de la lecture)".to_string(),
            range_min: 0.0,
            range_max: 40000.0,
        }),
        // DS18B20 (Maxim) — 1-Wire temperature probes
        id if id.starts_with("onewr:") => Some(SensorMeta {
            unit: "°C".to_string(),
            uncertainty: "±0.5 °C (-10 à +85 °C)\n±2 °C (hors plage)".to_string(),
            range_min: -55.0,
            range_max: 125.0,
        }),
        // BME280 (Bosch)
        id if id.ends_with("_T") && (id.contains("0x76") || id.contains("0x77")) => Some(SensorMeta {
            unit: "°C".to_string(),
            uncertainty: "±0.5 °C (à 25 °C)".to_string(),
            range_min: -40.0,
            range_max: 85.0,
        }),
        id if id.ends_with("_H") && (id.contains("0x76") || id.contains("0x77")) => Some(SensorMeta {
            unit: "%RH".to_string(),
            uncertainty: "±3 %RH (20-80 %RH, 25 °C)".to_string(),
            range_min: 0.0,
            range_max: 100.0,
        }),
        id if id.ends_with("_P") && (id.contains("0x76") || id.contains("0x77")) => Some(SensorMeta {
            unit: "hPa".to_string(),
            uncertainty: "±0.12 hPa (éq. ±1 m)".to_string(),
            range_min: 300.0,
            range_max: 1100.0,
        }),
        // Capteurs internes
        "vsense" => Some(SensorMeta {
            unit: "V".to_string(),
            uncertainty: "±0.1V".to_string(),
            range_min: 0.0,
            range_max: 25.0,
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
    // Prefer correction formula stored in devicesKnow (if present), fall back to legacy corr_<id> key
    let storage = nvs.lock().unwrap();
    if let Ok(Some(reg_json)) = storage.get_str("devicesKnow") {
        if let Ok(map) = serde_json::from_str::<HashMap<String, PersistEntry>>(&reg_json) {
            if let Some(pe) = map.get(device_id) {
                if let Some(c) = &pe.correction_formula {
                    return c.clone();
                }
            }
        }
    }
    // legacy key
    let key = format!("corr_{}", device_id);
    storage.get_str(&key).ok().flatten().unwrap_or_else(|| format!("{}.raw", device_id))
}

/// Sauvegarde la formule de correction dans le registre (devicesKnow). Persist only for dynamic devices.
pub fn set_correction_formula(nvs: &Arc<Mutex<NvsStorage>>, device_id: &str, formula: &str) -> Result<(), anyhow::Error> {
    let mut storage = nvs.lock().unwrap();
    // load existing registry (as PersistEntry map)
    let mut persist_map: HashMap<String, PersistEntry> = if let Ok(Some(j)) = storage.get_str("devicesKnow") {
        serde_json::from_str(&j).unwrap_or_default()
    } else { HashMap::new() };
    persist_map.insert(device_id.to_string(), PersistEntry { name: persist_map.get(device_id).map(|p| p.name.clone()).unwrap_or_else(|| device_id.to_string()), correction_formula: Some(formula.to_string()) });
    let new_str = serde_json::to_string(&persist_map)?;
    storage.set_str("devicesKnow", &new_str)?;
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

    /// Load device metadata registry from NVS (dynamic only) + static devices from code
    fn load_registry(&self) -> HashMap<String, DeviceEntry> {
        let mut map: HashMap<String, DeviceEntry> = HashMap::new();
        // 1. Static devices — toujours présents, en dur
        for &(id, name) in STATIC_DEVICES {
            map.insert(id.to_string(), DeviceEntry { name: name.to_string(), is_static: true, correction_formula: None });
        }
        // 2. Dynamic devices from NVS
        let storage = self.nvs.lock().unwrap();
        if let Ok(Some(json_str)) = storage.get_str("devicesKnow") {
            if let Ok(saved) = serde_json::from_str::<HashMap<String, PersistEntry>>(&json_str) {
                for (id, pe) in saved {
                    if is_static_device(&id) { continue; }
                    map.entry(id.clone()).or_insert(DeviceEntry { name: pe.name, is_static: false, correction_formula: pe.correction_formula });
                }
            }
        }
        map
    }

    /// Save device metadata registry to NVS — filtre les devices statiques (en dur dans le code)
    fn save_registry(&self, map: &HashMap<String, DeviceEntry>) {
        let mut persist_map: HashMap<String, PersistEntry> = HashMap::new();
        for (k, v) in map.iter() {
            if is_static_device(k) { continue; }
            persist_map.insert(k.clone(), PersistEntry { name: v.name.clone(), correction_formula: v.correction_formula.clone() });
        }
        let mut storage = self.nvs.lock().unwrap();
        if let Ok(json_str) = serde_json::to_string(&persist_map) {
            let _ = storage.set_str("devicesKnow", &json_str);
        }
    }

    /// Scan dynamic and static devices, merge with custom names from NVS.
    /// Les devices statiques sont en dur (const STATIC_DEVICES), jamais sauvés en NVS.
    pub fn scan_and_register(&mut self, onewr_pins: Vec<String>) -> Result<(), anyhow::Error> {
        let mut saved = self.load_registry(); // contient statiques (dur) + dynamiques (NVS)
        let mut updated = HashMap::new();

        // 1. Static Devices (Always present, hardcoded) — retire de saved pour ne pas les dupliquer
        for &(id, default_name) in STATIC_DEVICES {
            saved.remove(id); // ignore toute entrée NVS résiduelle pour les statiques
            updated.insert(id.to_string(), DeviceEntry {
                name: default_name.to_string(),
                is_static: true,
                correction_formula: None,
            });
        }

        // 2. Dynamic Devices: Screen and Radio
        let screen_present = crate::screen::is_present();
        if screen_present {
            let entry = saved.remove("screen").unwrap_or_else(|| DeviceEntry {
                name: "Écran ST7789".to_string(),
                is_static: false,
                correction_formula: None,
            });
            updated.insert("screen".to_string(), entry);
        }

        let radio_present = crate::radio::is_present();
        if radio_present {
            let entry = saved.remove("radio").unwrap_or_else(|| DeviceEntry {
                name: "Transmetteur Radio RF".to_string(),
                is_static: false,
                correction_formula: None,
            });
            updated.insert("radio".to_string(), entry);
        }

        // 3. Dynamic Devices: 1-Wire (DS18B20) discovered probes
        for addr in onewr_pins {
            let id = format!("onewr:{}", addr);
            let entry = saved.remove(&id).unwrap_or_else(|| DeviceEntry {
                name: format!("Sonde 1-Wire ({})", &addr[..6].to_uppercase()),
                is_static: false,
                correction_formula: None,
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
                    correction_formula: None,
                });
                updated.insert(id_t, entry_t);

                let id_h = format!("i2c:{}:0x{:02x}_H", channel, addr);
                let entry_h = saved.remove(&id_h).unwrap_or_else(|| DeviceEntry {
                    name: "SHT45-Hum".to_string(),
                    is_static: false,
                    correction_formula: None,
                });
                updated.insert(id_h, entry_h);
            } else if addr == 0x62 {
                let id = format!("i2c:{}:0x{:02x}", channel, addr);
                let entry = saved.remove(&id).unwrap_or_else(|| DeviceEntry {
                    name: "Capteur CO2 SCD41".to_string(),
                    is_static: false,
                    correction_formula: None,
                });
                updated.insert(id, entry);
            } else if addr == 0x76 || addr == 0x77 {
                // BME280 : séparer en trois capteurs (Température, Humidité, Pression)
                let id_t = format!("i2c:{}:0x{:02x}_T", channel, addr);
                let entry_t = saved.remove(&id_t).unwrap_or_else(|| DeviceEntry {
                    name: format!("BME280-Temp (i2c:{}:0x{:02x})", channel, addr),
                    is_static: false,
                    correction_formula: None,
                });
                updated.insert(id_t, entry_t);

                let id_h = format!("i2c:{}:0x{:02x}_H", channel, addr);
                let entry_h = saved.remove(&id_h).unwrap_or_else(|| DeviceEntry {
                    name: format!("BME280-Hum (i2c:{}:0x{:02x})", channel, addr),
                    is_static: false,
                    correction_formula: None,
                });
                updated.insert(id_h, entry_h);

                let id_p = format!("i2c:{}:0x{:02x}_P", channel, addr);
                let entry_p = saved.remove(&id_p).unwrap_or_else(|| DeviceEntry {
                    name: format!("BME280-Pres (i2c:{}:0x{:02x})", channel, addr),
                    is_static: false,
                    correction_formula: None,
                });
                updated.insert(id_p, entry_p);
            } else {
                let id = format!("i2c:{}:0x{:02x}", channel, addr);
                let entry = saved.remove(&id).unwrap_or_else(|| DeviceEntry {
                    name: format!("Périphérique I2C (Ch{} 0x{:02x})", channel, addr),
                    is_static: false,
                    correction_formula: None,
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
        schedules: Option<&HashMap<String, Vec<crate::actuators::ScheduledAction>>>,
    ) -> Vec<DeviceDisplay> {
        let registry = self.load_registry(); // construit à partir des statiques + NVS dynamiques
        let mut list = Vec::new();

        for (id, entry) in registry.iter() {
            let mut present = true;
            let mut value = "OK".to_string();
            let sensor_meta = get_sensor_meta(id);
            let correction = entry.correction_formula.clone().unwrap_or_else(|| get_correction_formula(&self.nvs, id));

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

            let is_act = matches!(id.as_str(), "rla" | "rlb" | "swpwr" | "ina" | "inb");
            let dev_schedules = if is_act {
                schedules.and_then(|s| s.get(id).cloned())
            } else {
                None
            };
            list.push(DeviceDisplay {
                id: id.to_string(),
                name: entry.name.clone(),
                is_static: entry.is_static,
                present: if is_act { None } else { Some(present) },
                value,
                sensor_meta,
                correction_formula: if is_act { None } else { Some(correction) },
                schedules: dev_schedules,
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

    /// Rename device (statiques : session uniquement ; dynamiques : persistés en NVS via devicesKnow)
    pub fn rename_device(&mut self, id: &str, new_name: &str) -> Result<(), anyhow::Error> {
        // Update in-session cache
        if let Some(entry) = self.devices.get_mut(id) {
            let trimmed = new_name.trim();
            let name_limit = if trimmed.len() > 64 {
                trimmed[..64].to_string()
            } else {
                trimmed.to_string()
            };
            if name_limit.is_empty() {
                return Err(anyhow::anyhow!("Le nom ne peut pas être vide"));
            }
            info!("Renaming device '{}' to '{}'", id, name_limit);
            entry.name = name_limit.clone();

            // Persist in NVS (only dynamic devices will actually be saved)
            let mut map = self.load_registry();
            if let Some(e) = map.get_mut(id) {
                e.name = name_limit;
            } else {
                map.insert(id.to_string(), DeviceEntry {
                    name: name_limit,
                    is_static: false,
                    correction_formula: None,
                });
            }
            self.save_registry(&map);
            return Ok(());
        }
        Err(anyhow::anyhow!("Périphérique introuvable"))
    }
}
