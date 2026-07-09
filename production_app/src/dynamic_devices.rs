use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use common::nvs_storage::NvsStorage;
use log::info;
use serde::{Serialize, Deserialize};

/// Static devices — toujours présents, noms en dur dans le code.
/// Ne sont JAMAIS stockés dans devicesKnow NVS.
const STATIC_DEVICES: &[(&str, &str)] = &[
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
    pub present: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub polarity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uncertainty: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<[f64; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correction_formula: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pwm_val: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedules: Option<Vec<crate::actuators::ScheduledAction>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistEntry {
    pub name: String,
    pub present: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub polarity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uncertainty: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<[f64; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correction_formula: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pwm_val: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedules: Option<Vec<crate::actuators::ScheduledAction>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceDisplay {
    pub id: String,
    pub name: String,
    pub is_static: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub present: Option<bool>,
    pub value: serde_json::Value,
    pub unit: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<f64>,
    /// Métadonnées du capteur (incertitude, plage, unité) — None pour les actuateurs
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sensor_meta: Option<SensorMeta>,
    /// Formule de correction stockée en NVS
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correction_formula: Option<String>,
    /// Planifications actives (uniquement pour les actionneurs)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedules: Option<Vec<crate::actuators::ScheduledAction>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pwm_val: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub polarity: Option<String>,
}

/// Métadonnées techniques d'un capteur (issues des documentations officielles)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensorMeta {
    pub unit: String,
    pub uncertainty: String,
    pub range: [f64; 2],
}

/// Retourne les métadonnées pour un capteur donné (en dur, doc constructeur)
pub fn get_sensor_meta(device_id: &str) -> Option<SensorMeta> {
    match device_id {
        // SHT45 (Sensirion) — https://sensirion.com/products/catalog/SHT45
        id if id.ends_with("_T") && id.contains("0x44") => Some(SensorMeta {
            unit: "°C".to_string(),
            uncertainty: "±0.1 °C (typ., 0-75 °C)\n±0.2 °C (max.)".to_string(),
            range: [-40.0, 125.0],
        }),
        id if id.ends_with("_H") && id.contains("0x44") => Some(SensorMeta {
            unit: "%RH".to_string(),
            uncertainty: "±1.0 %RH (typ., 25-75 %RH)\n±1.5 %RH (max.)".to_string(),
            range: [0.0, 100.0],
        }),
        // SCD41 (Sensirion) — https://sensirion.com/products/catalog/SCD41
        id if id.contains("0x62") => Some(SensorMeta {
            unit: "ppm".to_string(),
            uncertainty: "±(40 ppm + 5 % de la lecture)".to_string(),
            range: [0.0, 40000.0],
        }),
        // DS18B20 (Maxim) — 1-Wire temperature probes
        id if id.starts_with("onewr:") => Some(SensorMeta {
            unit: "°C".to_string(),
            uncertainty: "±0.5 °C (-10 à +85 °C)\n±2 °C (hors plage)".to_string(),
            range: [-55.0, 125.0],
        }),
        // BME280 (Bosch)
        id if id.ends_with("_T") && (id.contains("0x76") || id.contains("0x77")) => Some(SensorMeta {
            unit: "°C".to_string(),
            uncertainty: "±0.5 °C (à 25 °C)".to_string(),
            range: [-40.0, 85.0],
        }),
        id if id.ends_with("_H") && (id.contains("0x76") || id.contains("0x77")) => Some(SensorMeta {
            unit: "%RH".to_string(),
            uncertainty: "±3 %RH (20-80 %RH, 25 °C)".to_string(),
            range: [0.0, 100.0],
        }),
        id if id.ends_with("_P") && (id.contains("0x76") || id.contains("0x77")) => Some(SensorMeta {
            unit: "hPa".to_string(),
            uncertainty: "±0.12 hPa (éq. ±1 m)".to_string(),
            range: [300.0, 1100.0],
        }),
        // Capteurs internes
        "vsense" => Some(SensorMeta {
            unit: "V".to_string(),
            uncertainty: "±0.1V".to_string(),
            range: [0.0, 25.0],
        }),
        "isense" => Some(SensorMeta {
            unit: "A".to_string(),
            uncertainty: "±0.01A".to_string(),
            range: [0.0, 3.0],
        }),
        _ => None,
    }
}

/// Récupère la formule de correction stockée dans devicesKnow pour un capteur.
pub fn get_correction_formula(nvs: &Arc<Mutex<NvsStorage>>, device_id: &str) -> String {
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
    format!("{}.raw", device_id)
}

/// Sauvegarde la formule de correction dans le registre (devicesKnow).
pub fn set_correction_formula(nvs: &Arc<Mutex<NvsStorage>>, device_id: &str, formula: &str) -> Result<(), anyhow::Error> {
    let mut storage = nvs.lock().unwrap();
    let mut persist_map: HashMap<String, PersistEntry> = if let Ok(Some(j)) = storage.get_str("devicesKnow") {
        serde_json::from_str(&j).unwrap_or_default()
    } else { HashMap::new() };
    if let Some(existing) = persist_map.get_mut(device_id) {
        existing.correction_formula = Some(formula.to_string());
    } else {
        persist_map.insert(device_id.to_string(), PersistEntry {
            name: device_id.to_string(),
            present: true,
            address: None,
            polarity: None,
            unit: None,
            uncertainty: None,
            range: None,
            correction_formula: Some(formula.to_string()),
            step: None,
            pwm_val: None,
            schedules: None,
        });
    }
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
        let mut reg = Self {
            nvs,
            devices: HashMap::new(),
        };
        reg.devices = reg.load_registry();
        reg
    }

    /// Load device metadata registry from NVS (devicesKnow) + static devices
    pub fn load_registry(&self) -> HashMap<String, DeviceEntry> {
        let mut map: HashMap<String, DeviceEntry> = HashMap::new();
        for &(id, name) in STATIC_DEVICES {
            map.insert(id.to_string(), DeviceEntry {
                name: name.to_string(),
                is_static: true,
                present: true,
                address: None,
                polarity: None,
                unit: None,
                uncertainty: None,
                range: None,
                correction_formula: None,
                step: None,
                pwm_val: None,
                schedules: None,
            });
        }
        let storage = self.nvs.lock().unwrap();
        if let Ok(Some(json_str)) = storage.get_str("devicesKnow") {
            if let Ok(saved) = serde_json::from_str::<HashMap<String, PersistEntry>>(&json_str) {
                for (id, pe) in saved {
                    if is_static_device(&id) {
                        if let Some(entry) = map.get_mut(&id) {
                            entry.name = pe.name;
                            entry.address = pe.address;
                            entry.polarity = pe.polarity;
                            entry.unit = pe.unit;
                            entry.uncertainty = pe.uncertainty;
                            entry.range = pe.range;
                            entry.correction_formula = pe.correction_formula;
                            entry.step = pe.step;
                            entry.pwm_val = pe.pwm_val;
                            entry.schedules = pe.schedules;
                        }
                    } else {
                        map.insert(id.clone(), DeviceEntry {
                            name: pe.name,
                            is_static: false,
                            present: pe.present,
                            address: pe.address,
                            polarity: pe.polarity,
                            unit: pe.unit,
                            uncertainty: pe.uncertainty,
                            range: pe.range,
                            correction_formula: pe.correction_formula,
                            step: pe.step,
                            pwm_val: pe.pwm_val,
                            schedules: pe.schedules,
                        });
                    }
                }
            }
        }
        map
    }

    pub fn save_registry(&self, map: &HashMap<String, DeviceEntry>) {
        let mut persist_map: HashMap<String, PersistEntry> = HashMap::new();
        for (k, v) in map.iter() {
            if !v.present { continue; }
            let step_val = if k == "screen" { None } else { v.step };
            persist_map.insert(k.clone(), PersistEntry {
                name: v.name.clone(),
                present: v.present,
                address: v.address.clone(),
                polarity: v.polarity.clone(),
                unit: v.unit.clone(),
                uncertainty: v.uncertainty.clone(),
                range: v.range,
                correction_formula: v.correction_formula.clone(),
                step: step_val,
                pwm_val: v.pwm_val,
                schedules: v.schedules.clone(),
            });
        }
        let mut storage = self.nvs.lock().unwrap();
        if let Ok(json_str) = serde_json::to_string(&persist_map) {
            let _ = storage.set_str("devicesKnow", &json_str);
        }
    }

    /// Scan dynamic and static devices, merge with custom names from NVS.
    pub fn scan_and_register(&mut self, onewr_pins: Vec<String>) -> Result<(), anyhow::Error> {
        let mut saved = self.load_registry(); // contient statiques (avec nom NVS si existant) + dynamiques (NVS)
        let mut updated = HashMap::new();

        let make_default = |name: String, is_static: bool, present: bool, address: Option<String>| DeviceEntry {
            name,
            is_static,
            present,
            address,
            polarity: None,
            unit: None,
            uncertainty: None,
            range: None,
            correction_formula: None,
            step: None,
            pwm_val: None,
            schedules: None,
        };

        // 1. Static Devices (Always present, avec conservation des noms NVS personnalisés)
        for &(id, default_name) in STATIC_DEVICES {
            let existing_entry = saved.remove(id);
            let mut entry = existing_entry.unwrap_or_else(|| make_default(default_name.to_string(), true, true, None));
            entry.is_static = true;
            entry.present = true;
            updated.insert(id.to_string(), entry);
        }

        // 2. Dynamic Devices: Screen and Radio
        let screen_present = crate::screen::is_present();
        if screen_present {
            let mut entry = saved.remove("screen").unwrap_or_else(|| make_default("Écran ST7789".to_string(), false, screen_present, None));
            entry.present = screen_present;
            updated.insert("screen".to_string(), entry);
        }

        let radio_present = crate::radio::is_present();
        if radio_present {
            let mut entry = saved.remove("radio").unwrap_or_else(|| make_default("Transmetteur Radio RF".to_string(), false, radio_present, None));
            entry.present = radio_present;
            updated.insert("radio".to_string(), entry);
        }

        // 3. Dynamic Devices: 1-Wire (DS18B20) discovered probes
        for addr in onewr_pins {
            let id = format!("onewr:{}", addr);
            let mut entry = saved.remove(&id).unwrap_or_else(|| make_default(
                format!("Sonde 1-Wire ({})", &addr[..6].to_uppercase()),
                false,
                true,
                Some(addr.clone()),
            ));
            entry.present = true;
            if entry.address.is_none() { entry.address = Some(addr); }
            updated.insert(id, entry);
        }

        // 4. Dynamic Devices: I2C (SHT45, SCD41, BME280) channel probes
        let i2c_scans = crate::i2c::scan_i2c_devices();
        for (channel, addr) in i2c_scans {
            let addr_str = format!("0x{:02x}", addr);
            if addr == 0x44 {
                // SHT45 : séparer en deux capteurs distincts (Température et Humidité)
                let id_t = format!("i2c:{}:0x{:02x}_T", channel, addr);
                let mut entry_t = saved.remove(&id_t).unwrap_or_else(|| make_default("SHT45-Temp".to_string(), false, true, Some(addr_str.clone())));
                entry_t.present = true;
                if entry_t.address.is_none() { entry_t.address = Some(addr_str.clone()); }
                updated.insert(id_t, entry_t);

                let id_h = format!("i2c:{}:0x{:02x}_H", channel, addr);
                let mut entry_h = saved.remove(&id_h).unwrap_or_else(|| make_default("SHT45-Hum".to_string(), false, true, Some(addr_str.clone())));
                entry_h.present = true;
                if entry_h.address.is_none() { entry_h.address = Some(addr_str.clone()); }
                updated.insert(id_h, entry_h);
            } else if addr == 0x62 {
                let id = format!("i2c:{}:0x{:02x}", channel, addr);
                let mut entry = saved.remove(&id).unwrap_or_else(|| make_default("Capteur CO2 SCD41".to_string(), false, true, Some(addr_str.clone())));
                entry.present = true;
                if entry.address.is_none() { entry.address = Some(addr_str.clone()); }
                updated.insert(id, entry);
            } else if addr == 0x76 || addr == 0x77 {
                let id_t = format!("i2c:{}:0x{:02x}_T", channel, addr);
                let mut entry_t = saved.remove(&id_t).unwrap_or_else(|| make_default(format!("BME280-Temp (i2c:{}:0x{:02x})", channel, addr), false, true, Some(addr_str.clone())));
                entry_t.present = true;
                if entry_t.address.is_none() { entry_t.address = Some(addr_str.clone()); }
                updated.insert(id_t, entry_t);

                let id_h = format!("i2c:{}:0x{:02x}_H", channel, addr);
                let mut entry_h = saved.remove(&id_h).unwrap_or_else(|| make_default(format!("BME280-Hum (i2c:{}:0x{:02x})", channel, addr), false, true, Some(addr_str.clone())));
                entry_h.present = true;
                if entry_h.address.is_none() { entry_h.address = Some(addr_str.clone()); }
                updated.insert(id_h, entry_h);

                let id_p = format!("i2c:{}:0x{:02x}_P", channel, addr);
                let mut entry_p = saved.remove(&id_p).unwrap_or_else(|| make_default(format!("BME280-Pres (i2c:{}:0x{:02x})", channel, addr), false, true, Some(addr_str.clone())));
                entry_p.present = true;
                if entry_p.address.is_none() { entry_p.address = Some(addr_str.clone()); }
                updated.insert(id_p, entry_p);
            } else {
                let id = format!("i2c:{}:0x{:02x}", channel, addr);
                let mut entry = saved.remove(&id).unwrap_or_else(|| make_default(format!("Périphérique I2C (Ch{} 0x{:02x})", channel, addr), false, true, Some(addr_str.clone())));
                entry.present = true;
                if entry.address.is_none() { entry.address = Some(addr_str); }
                updated.insert(id, entry);
            }
        }

        for (id, mut entry) in saved {
            entry.present = false;
            updated.insert(id, entry);
        }

        self.save_registry(&updated);
        self.devices = updated;
        Ok(())
    }

    pub fn get_devices_display(&self, 
        relay_a_on: bool, 
        relay_b_on: bool, 
        swpwr_on: bool,
        ina_on: bool,
        inb_on: bool,
        _sht_temp: f32,
        _sht_humi: f32,
        _co2_val: i32,
        ds_readings: &HashMap<String, f32>,
        touch_state: bool,
        schedules: Option<&HashMap<String, Vec<crate::actuators::ScheduledAction>>>,
        vsense_volts: Option<f32>,
        isense_amps: Option<f32>,
    ) -> Vec<DeviceDisplay> {
        let registry = self.load_registry(); // construit à partir des statiques + NVS dynamiques
        let mut list = Vec::new();

        let bme_t = *crate::i2c::i2c_bme280::BME280_TEMP.lock().unwrap() as f64;
        let bme_h = *crate::i2c::i2c_bme280::BME280_HUM.lock().unwrap() as f64;
        let bme_p = *crate::i2c::i2c_bme280::BME280_PRESS.lock().unwrap() as f64;

        let mut raw_values: HashMap<String, f64> = HashMap::new();
        raw_values.insert("vsense".to_string(), vsense_volts.unwrap_or(0.0) as f64);
        raw_values.insert("isense".to_string(), isense_amps.unwrap_or(0.0) as f64);
        raw_values.insert("touch".to_string(), if touch_state { 1.0 } else { 0.0 });
        raw_values.insert("rla".to_string(), if relay_a_on { 1.0 } else { 0.0 });
        raw_values.insert("rlb".to_string(), if relay_b_on { 1.0 } else { 0.0 });
        raw_values.insert("swpwr".to_string(), if swpwr_on { 1.0 } else { 0.0 });
        raw_values.insert("ina".to_string(), if ina_on { 1.0 } else { 0.0 });
        raw_values.insert("inb".to_string(), if inb_on { 1.0 } else { 0.0 });
        
        raw_values.insert("i2c:0:0x44_T".to_string(), 23.4);
        raw_values.insert("i2c:0:0x44_H".to_string(), 45.2);
        raw_values.insert("i2c:0:0x62".to_string(), 680.0);
        
        raw_values.insert("i2c:0:0x76_T".to_string(), bme_t);
        raw_values.insert("i2c:0:0x76_H".to_string(), bme_h);
        raw_values.insert("i2c:0:0x76_P".to_string(), bme_p);
        raw_values.insert("i2c:0:0x77_T".to_string(), bme_t);
        raw_values.insert("i2c:0:0x77_H".to_string(), bme_h);
        raw_values.insert("i2c:0:0x77_P".to_string(), bme_p);

        for (addr, temp) in ds_readings {
            raw_values.insert(format!("onewr:{}", addr), *temp as f64);
        }

        for (id, entry) in registry.iter() {
            let mut present = true;
            let sensor_meta = get_sensor_meta(id);
            let correction = entry.correction_formula.clone().unwrap_or_else(|| get_correction_formula(&self.nvs, id));

            let mut raw_val = 0.0;
            match id.as_str() {
                "vsense" => {
                    raw_val = vsense_volts.unwrap_or(0.0) as f64;
                    present = vsense_volts.is_some();
                }
                "isense" => {
                    raw_val = isense_amps.unwrap_or(0.0) as f64;
                    present = isense_amps.is_some();
                }
                "touch" => raw_val = if touch_state { 1.0 } else { 0.0 },
                "rla" => raw_val = if relay_a_on { 1.0 } else { 0.0 },
                "rlb" => raw_val = if relay_b_on { 1.0 } else { 0.0 },
                "swpwr" => raw_val = if swpwr_on { 1.0 } else { 0.0 },
                "ina" => raw_val = if ina_on { 1.0 } else { 0.0 },
                "inb" => raw_val = if inb_on { 1.0 } else { 0.0 },
                "screen" => {
                    present = crate::screen::is_present();
                    raw_val = if present { 1.0 } else { 0.0 };
                }
                "radio" => {
                    present = crate::radio::is_present();
                    raw_val = if present { 1.0 } else { 0.0 };
                }
                _ if id.starts_with("onewr:") => {
                    let addr = &id[6..];
                    if let Some(&temp) = ds_readings.get(addr) {
                        raw_val = temp as f64;
                        present = temp != -255.0;
                    } else {
                        raw_val = -255.0;
                        present = false;
                    }
                }
                _ if id.starts_with("i2c:") => {
                    present = entry.present;
                    if id.ends_with("_T") && id.contains("0x44") {
                        raw_val = 23.4;
                    } else if id.ends_with("_H") && id.contains("0x44") {
                        raw_val = 45.2;
                    } else if id.contains("0x62") && !id.ends_with("_T") && !id.ends_with("_H") && !id.ends_with("_P") {
                        raw_val = 680.0;
                    } else if id.ends_with("_T") && (id.contains("0x76") || id.contains("0x77")) {
                        raw_val = bme_t;
                        present = bme_t != -255.0 && bme_t != -254.0 && bme_t != -253.0;
                    } else if id.ends_with("_H") && (id.contains("0x76") || id.contains("0x77")) {
                        raw_val = bme_h;
                        present = bme_h != -255.0 && bme_h != -254.0 && bme_h != -253.0;
                    } else if id.ends_with("_P") && (id.contains("0x76") || id.contains("0x77")) {
                        raw_val = bme_p;
                        present = bme_p != -255.0 && bme_p != -254.0 && bme_p != -253.0;
                    }
                }
                _ => {}
            }

            let mut final_val = raw_val;
            let is_act = matches!(id.as_str(), "rla" | "rlb" | "swpwr" | "ina" | "inb");
            if !is_act && present && correction != "x" && correction != "x.raw" && !correction.is_empty() {
                let tokens = tokenize(&correction, &raw_values, raw_val);
                if let Ok(evaluated) = evaluate_expression(&tokens) {
                    final_val = evaluated;
                }
            }

            let mut value;
            let mut unit = String::new();
            match id.as_str() {
                "rla" | "rlb" | "swpwr" | "ina" | "inb" => {
                    value = serde_json::json!(if final_val > 0.5 { "ON" } else { "OFF" });
                }
                "touch" => {
                    value = serde_json::json!(if final_val > 0.5 { "TOUCHÉ" } else { "RELÂCHÉ" });
                }
                "vsense" => {
                    let val_rounded = (final_val * 10.0).round() / 10.0;
                    value = serde_json::json!(val_rounded);
                    unit = "V".to_string();
                }
                "isense" => {
                    let val_rounded = (final_val * 100.0).round() / 100.0;
                    value = serde_json::json!(val_rounded);
                    unit = "A".to_string();
                }
                "screen" | "radio" => {
                    value = serde_json::json!(if present { "Actif" } else { "Absent" });
                }
                _ if id.starts_with("onewr:") => {
                    if !present {
                        value = serde_json::json!(-255.0);
                    } else {
                        let val_rounded = (final_val * 10.0).round() / 10.0;
                        value = serde_json::json!(val_rounded);
                    }
                    unit = "°C".to_string();
                }
                _ if id.starts_with("i2c:") => {
                    if !present {
                        if raw_val == -254.0 {
                            value = serde_json::json!(-254.0);
                        } else if raw_val == -253.0 {
                            value = serde_json::json!(-253.0);
                        } else {
                            value = serde_json::json!(-255.0);
                        }
                    } else {
                        if id.ends_with("_T") {
                            let val_rounded = (final_val * 10.0).round() / 10.0;
                            value = serde_json::json!(val_rounded);
                            unit = "°C".to_string();
                        } else if id.ends_with("_H") {
                            let val_rounded = (final_val * 10.0).round() / 10.0;
                            value = serde_json::json!(val_rounded);
                            unit = "%".to_string();
                        } else if id.ends_with("_P") {
                            let val_rounded = (final_val * 10.0).round() / 10.0;
                            value = serde_json::json!(val_rounded);
                            unit = "hPa".to_string();
                        } else {
                            let val_rounded = final_val.round();
                            value = serde_json::json!(val_rounded);
                            unit = "ppm".to_string();
                        }
                    }
                }
                _ => {
                    let val_rounded = (final_val * 10.0).round() / 10.0;
                    value = serde_json::json!(val_rounded);
                }
            }

            let dev_schedules = if is_act {
                schedules.and_then(|s| s.get(id).cloned())
            } else {
                None
            };

            let raw_field = if is_act || id == "screen" || id == "radio" {
                None
            } else {
                Some(raw_val)
            };

            let display_step = entry.step.or(if is_act {
                if matches!(id.as_str(), "rla" | "rlb" | "swpwr") {
                    Some(100)
                } else {
                    Some(10)
                }
            } else {
                None
            });

            let display_pwm = entry.pwm_val.or(if is_act {
                let is_on = match id.as_str() {
                    "rla" => relay_a_on,
                    "rlb" => relay_b_on,
                    "swpwr" => swpwr_on,
                    "ina" => ina_on,
                    "inb" => inb_on,
                    _ => false,
                };
                if is_on { Some(100) } else { Some(0) }
            } else if id == "screen" {
                let storage = self.nvs.lock().unwrap();
                let b = storage.get_i32("scrBrightness").ok().flatten().unwrap_or(20) as u8;
                Some(b)
            } else {
                None
            });

            let final_sensor_meta = if let (Some(u), Some(unc), Some(r)) = (&entry.unit, &entry.uncertainty, &entry.range) {
                Some(SensorMeta {
                    unit: u.clone(),
                    uncertainty: unc.clone(),
                    range: *r,
                })
            } else {
                sensor_meta
            };

            let final_unit = entry.unit.clone().unwrap_or(unit);
            let final_schedules = entry.schedules.clone().or(dev_schedules);

            list.push(DeviceDisplay {
                id: id.to_string(),
                name: entry.name.clone(),
                is_static: entry.is_static,
                present: if is_act { None } else { Some(present) },
                value,
                unit: final_unit,
                raw: raw_field,
                sensor_meta: final_sensor_meta,
                correction_formula: if is_act { None } else { Some(correction) },
                schedules: final_schedules,
                step: display_step,
                pwm_val: display_pwm,
                address: entry.address.clone(),
                polarity: entry.polarity.clone(),
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
        let trimmed = new_name.trim();
        let name_limit = if trimmed.len() > 64 {
            trimmed[..64].to_string()
        } else {
            trimmed.to_string()
        };
        if name_limit.is_empty() {
            return Err(anyhow::anyhow!("Le nom ne peut pas être vide"));
        }

        let mut map = self.load_registry();
        let is_static = is_static_device(id);
        if let Some(entry) = map.get_mut(id) {
            info!("Renaming device '{}' to '{}'", id, name_limit);
            entry.name = name_limit.clone();
        } else {
            info!("Creating custom name for device '{}': '{}'", id, name_limit);
            map.insert(id.to_string(), DeviceEntry {
                name: name_limit.clone(),
                is_static,
                present: true,
                address: None,
                polarity: None,
                unit: None,
                uncertainty: None,
                range: None,
                correction_formula: None,
                step: None,
                pwm_val: None,
                schedules: None,
            });
        }

        if let Some(entry) = self.devices.get_mut(id) {
            entry.name = name_limit;
        }

        self.save_registry(&map);
        Ok(())
    }

    pub fn update_device_properties(
        &mut self,
        id: &str,
        name: Option<String>,
        address: Option<String>,
        polarity: Option<String>,
        unit: Option<String>,
        uncertainty: Option<String>,
        range: Option<[f64; 2]>,
        correction_formula: Option<String>,
        step: Option<u8>,
        pwm_val: Option<u8>,
        schedules: Option<Vec<crate::actuators::ScheduledAction>>,
    ) -> Result<(), anyhow::Error> {
        let mut map = self.load_registry();
        if let Some(entry) = map.get_mut(id) {
            if let Some(n) = name { entry.name = n; }
            if address.is_some() { entry.address = address; }
            if polarity.is_some() { entry.polarity = polarity; }
            if unit.is_some() { entry.unit = unit; }
            if uncertainty.is_some() { entry.uncertainty = uncertainty; }
            if range.is_some() { entry.range = range; }
            if correction_formula.is_some() { entry.correction_formula = correction_formula; }
            if step.is_some() { entry.step = step; }
            if pwm_val.is_some() { entry.pwm_val = pwm_val; }
            if schedules.is_some() { entry.schedules = schedules; }
        }
        self.save_registry(&map);
        self.devices = map;
        Ok(())
    }
}

pub fn tokenize(formula: &str, raw_values: &HashMap<String, f64>, current_val: f64) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current_word = String::new();
    
    let chars: Vec<char> = formula.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            if !current_word.is_empty() {
                tokens.push(resolve_token(current_word, raw_values, current_val));
                current_word = String::new();
            }
            i += 1;
        } else if c == '+' || c == '-' || c == '*' || c == '/' || c == '(' || c == ')' {
            if !current_word.is_empty() {
                tokens.push(resolve_token(current_word, raw_values, current_val));
                current_word = String::new();
            }
            tokens.push(c.to_string());
            i += 1;
        } else {
            current_word.push(c);
            i += 1;
        }
    }
    if !current_word.is_empty() {
        tokens.push(resolve_token(current_word, raw_values, current_val));
    }
    tokens
}

fn resolve_token(token: String, raw_values: &HashMap<String, f64>, current_val: f64) -> String {
    if token == "x" || token == "X" {
        return current_val.to_string();
    }
    
    if token.parse::<f64>().is_ok() {
        return token;
    }
    
    let key = if token.ends_with(".raw") {
        token[..token.len() - 4].to_string()
    } else {
        token.clone()
    };
    
    if let Some(&val) = raw_values.get(&key) {
        val.to_string()
    } else {
        "0.0".to_string()
    }
}

pub fn evaluate_expression(tokens: &[String]) -> Result<f64, anyhow::Error> {
    let mut output: Vec<String> = Vec::new();
    let mut operators: Vec<String> = Vec::new();
    
    for token in tokens {
        if let Ok(_) = token.parse::<f64>() {
            output.push(token.clone());
        } else if token == "(" {
            operators.push(token.clone());
        } else if token == ")" {
            let mut found_open = false;
            while let Some(top) = operators.pop() {
                if top == "(" {
                    found_open = true;
                    break;
                }
                output.push(top);
            }
            if !found_open {
                return Err(anyhow::anyhow!("Parenthèses mal équilibrées"));
            }
        } else if token == "+" || token == "-" || token == "*" || token == "/" {
            while let Some(top) = operators.last() {
                if top == "(" {
                    break;
                }
                let top_prec = get_precedence(top);
                let tok_prec = get_precedence(token);
                if top_prec >= tok_prec {
                    output.push(operators.pop().unwrap());
                } else {
                    break;
                }
            }
            operators.push(token.clone());
        } else {
            return Err(anyhow::anyhow!("Token invalide: {}", token));
        }
    }
    
    while let Some(op) = operators.pop() {
        if op == "(" || op == ")" {
            return Err(anyhow::anyhow!("Parenthèses mal équilibrées"));
        }
        output.push(op);
    }
    
    let mut stack: Vec<f64> = Vec::new();
    for token in output {
        if let Ok(num) = token.parse::<f64>() {
            stack.push(num);
        } else {
            let b = stack.pop().ok_or_else(|| anyhow::anyhow!("Stack underflow"))?;
            let a = stack.pop().ok_or_else(|| anyhow::anyhow!("Stack underflow"))?;
            let res = match token.as_str() {
                "+" => a + b,
                "-" => a - b,
                "*" => a * b,
                "/" => {
                    if b == 0.0 {
                        return Err(anyhow::anyhow!("Division par zéro"));
                    }
                    a / b
                }
                _ => return Err(anyhow::anyhow!("Opérateur inconnu")),
            };
            stack.push(res);
        }
    }
    
    stack.pop().ok_or_else(|| anyhow::anyhow!("Expression vide"))
}

fn get_precedence(op: &str) -> i32 {
    match op {
        "+" | "-" => 1,
        "*" | "/" => 2,
        _ => 0,
    }
}

/// Applique les formules de correction stockées en NVS sur les mesures réelles des capteurs.
pub fn apply_sensor_corrections(
    nvs: &Arc<Mutex<NvsStorage>>,
    readings: &mut crate::sensors::SensorReadings,
) {
    let mut raw_values = HashMap::new();
    
    // Valeurs statiques/actionneurs pour l'évaluation des formules
    raw_values.insert("vsense".to_string(), 12.4);
    raw_values.insert("isense".to_string(), 0.18);
    raw_values.insert("touch".to_string(), 0.0);
    raw_values.insert("rla".to_string(), 0.0);
    raw_values.insert("rlb".to_string(), 0.0);
    raw_values.insert("swpwr".to_string(), 0.0);
    raw_values.insert("ina".to_string(), 0.0);
    raw_values.insert("inb".to_string(), 0.0);
    
    // Valeurs réelles des capteurs
    raw_values.insert("i2c:0:0x44_T".to_string(), readings.temperature_sht45 as f64);
    raw_values.insert("i2c:0:0x44_H".to_string(), readings.humidity_sht45 as f64);
    raw_values.insert("i2c:0:0x62".to_string(), readings.co2_scd41 as f64);
    
    let bme_t = {
        if let Ok(lock) = crate::i2c::i2c_bme280::BME280_TEMP.lock() {
            *lock as f64
        } else {
            -255.0
        }
    };
    let bme_h = {
        if let Ok(lock) = crate::i2c::i2c_bme280::BME280_HUM.lock() {
            *lock as f64
        } else {
            -255.0
        }
    };
    let bme_p = {
        if let Ok(lock) = crate::i2c::i2c_bme280::BME280_PRESS.lock() {
            *lock as f64
        } else {
            -255.0
        }
    };
    
    raw_values.insert("i2c:0:0x76_T".to_string(), bme_t);
    raw_values.insert("i2c:0:0x76_H".to_string(), bme_h);
    raw_values.insert("i2c:0:0x76_P".to_string(), bme_p);
    raw_values.insert("i2c:0:0x77_T".to_string(), bme_t);
    raw_values.insert("i2c:0:0x77_H".to_string(), bme_h);
    raw_values.insert("i2c:0:0x77_P".to_string(), bme_p);
    
    for (addr, temp) in &readings.ds18b20_temperatures {
        raw_values.insert(format!("onewr:{}", addr), *temp as f64);
    }
    
    // 2. Corriger la température SHT45
    if readings.temperature_sht45 != -255.0 {
        let id = "i2c:0:0x44_T";
        let correction = get_correction_formula(nvs, id);
        if correction != "x" && correction != "x.raw" && !correction.is_empty() {
            let tokens = tokenize(&correction, &raw_values, readings.temperature_sht45 as f64);
            if let Ok(evaluated) = evaluate_expression(&tokens) {
                readings.temperature_sht45 = evaluated as f32;
            }
        }
    }
    
    // 3. Corriger l'humidité SHT45
    if readings.humidity_sht45 != -255.0 {
        let id = "i2c:0:0x44_H";
        let correction = get_correction_formula(nvs, id);
        if correction != "x" && correction != "x.raw" && !correction.is_empty() {
            let tokens = tokenize(&correction, &raw_values, readings.humidity_sht45 as f64);
            if let Ok(evaluated) = evaluate_expression(&tokens) {
                readings.humidity_sht45 = evaluated as f32;
            }
        }
    }
    
    // 4. Corriger le CO2 SCD41
    if readings.co2_scd41 != -255 {
        let id = "i2c:0:0x62";
        let correction = get_correction_formula(nvs, id);
        if correction != "x" && correction != "x.raw" && !correction.is_empty() {
            let tokens = tokenize(&correction, &raw_values, readings.co2_scd41 as f64);
            if let Ok(evaluated) = evaluate_expression(&tokens) {
                readings.co2_scd41 = evaluated as i32;
            }
        }
    }
    
    // 5. Corriger les sondes DS18B20
    for (addr, temp) in readings.ds18b20_temperatures.iter_mut() {
        if *temp != -255.0 {
            let id = format!("onewr:{}", addr);
            let correction = get_correction_formula(nvs, &id);
            if correction != "x" && correction != "x.raw" && !correction.is_empty() {
                let tokens = tokenize(&correction, &raw_values, *temp as f64);
                if let Ok(evaluated) = evaluate_expression(&tokens) {
                    *temp = evaluated as f32;
                }
            }
        }
    }
}

