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
    ("H0", "Pont H"),
];

pub fn is_static_device(id: &str) -> bool {
    STATIC_DEVICES.iter().any(|(i, _)| *i == id)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubPwmDevice {
    pub name: String,
    pub pwm_val: u8,
}

/// `RuleCondition` (structure) : Représente une règle planifiée événementielle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleCondition {
    /// `name` (type: Option<String>) : Nom descriptif optionnel de la règle (ex: "moteurs D/G").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `utc` (type: Option<Vec<String>>) : Plage de dates UTC optionnelle sous forme d'un tableau contenant [start, end] (dates ISO 8601).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub utc: Option<Vec<String>>,
    /// `if_expr` (type: String) : Expression de condition logique sur les capteurs (ex: "i2c:7:0x44_H < 50").
    #[serde(rename = "if")]
    pub if_expr: String,
    /// `then_expr` (type: String) : Expression(s) d'affectation ou de calcul si la condition est vraie.
    #[serde(rename = "then")]
    pub then_expr: String,
    /// `else_expr` (type: Option<String>) : Expression(s) d'affectation ou de calcul alternative(s) si la condition est fausse.
    #[serde(skip_serializing_if = "Option::is_none", rename = "else")]
    pub else_expr: Option<String>,
}

/// `MissingInfo` (structure) : Compteur de défaillance d'un capteur.
/// - `since` (type: String) : Horodatage ISO 8601 du début de l'absence.
/// - `count` (type: u32) : Nombre de cycles consécutifs où le capteur est absent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissingInfo {
    pub since: String,
    pub count: u32,
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
    pub inverseur: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ina: Option<SubPwmDevice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inb: Option<SubPwmDevice>,
    /// `rules` (type: Option<Vec<RuleCondition>>) : Liste des règles de planification événementielle associées.
    #[serde(skip_serializing_if = "Option::is_none", rename = "Rule")]
    pub rules: Option<Vec<RuleCondition>>,
    /// `missing` (type: Option<MissingInfo>) : Compteur d'absence consécutive du capteur.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub missing: Option<MissingInfo>,
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
    pub inverseur: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ina: Option<SubPwmDevice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inb: Option<SubPwmDevice>,
    /// `rules` (type: Option<Vec<RuleCondition>>) : Liste des règles de planification événementielle.
    #[serde(skip_serializing_if = "Option::is_none", rename = "Rule")]
    pub rules: Option<Vec<RuleCondition>>,
    /// `missing` (type: Option<MissingInfo>) : Compteur d'absence consécutive du capteur.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub missing: Option<MissingInfo>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub buffer: Option<Vec<f64>>,
    pub inverseur: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ina: Option<SubPwmDevice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inb: Option<SubPwmDevice>,
    /// `rules` (type: Option<Vec<RuleCondition>>) : Liste des règles de planification événementielle.
    #[serde(skip_serializing_if = "Option::is_none", rename = "Rule")]
    pub rules: Option<Vec<RuleCondition>>,
    /// `missing` (type: Option<MissingInfo>) : Informations sur les absences consécutives du capteur.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub missing: Option<MissingInfo>,
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
        // SHT3x (Sensirion) — https://sensirion.com/products/catalog/SHT30-DIS-B
        id if id.ends_with("_T") && id.contains("0x45") => Some(SensorMeta {
            unit: "°C".to_string(),
            uncertainty: "±0.2 °C (typ., 0-65 °C)\n±0.5 °C (max.)".to_string(),
            range: [-40.0, 125.0],
        }),
        id if id.ends_with("_H") && id.contains("0x45") => Some(SensorMeta {
            unit: "%RH".to_string(),
            uncertainty: "±2.0 %RH (typ., 0-90 %RH)\n±4.0 %RH (max.)".to_string(),
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
            inverseur: None,
            ina: None,
            inb: None,
            rules: None,
            missing: None,
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
            let mut entry = DeviceEntry {
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
                inverseur: None,
                ina: None,
                inb: None,
                rules: None,
                missing: None,
            };
            if id == "H0" {
                entry.inverseur = Some(true); // true = mode inverseur par défaut
                entry.ina = Some(SubPwmDevice { name: "open door".to_string(), pwm_val: 30 });
                entry.inb = Some(SubPwmDevice { name: "close door".to_string(), pwm_val: 30 });
            }
            map.insert(id.to_string(), entry);
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
                            entry.rules = pe.rules;
                            if id == "H0" {
                                if let Some(inv) = pe.inverseur { entry.inverseur = Some(inv); }
                                if let Some(i) = pe.ina { entry.ina = Some(i); }
                                if let Some(i) = pe.inb { entry.inb = Some(i); }
                            }
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
                            inverseur: pe.inverseur,
                            ina: pe.ina,
                            inb: pe.inb,
                            rules: pe.rules,
                            missing: pe.missing,
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
                inverseur: v.inverseur,
                ina: v.ina.clone(),
                inb: v.inb.clone(),
                rules: v.rules.clone(),
                missing: v.missing.clone(),
            });
        }
        let mut storage = self.nvs.lock().unwrap();
        if let Ok(json_str) = serde_json::to_string(&persist_map) {
            let _ = storage.set_str("devicesKnow", &json_str);
        }
    }

    /// Scan dynamic and static devices, merge with custom names from NVS.
    pub fn scan_and_register(&mut self, onewr_pins: Vec<String>, i2c: &Arc<Mutex<crate::i2c::I2c>>) -> Result<(), anyhow::Error> {
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
            inverseur: None,
            ina: None,
            inb: None,
            rules: None,
            missing: None,
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
        let i2c_scans = i2c.lock().unwrap().found_devices.clone();
        for (channel, addr) in i2c_scans {
            let addr_str = format!("0x{:02x}", addr);
            if addr == 0x44 || addr == 0x45 {
                // Déterminer le modèle réel (SHT30 ou SHT45)
                let model_name = {
                    let i2c_lock = i2c.lock().unwrap();
                    let mut name = if addr == 0x44 { "SHT45".to_string() } else { "SHT30".to_string() };
                    
                    // Chercher dans sht4xs ou sht3xs selon la liste d'enregistrement d'I2C
                    let found_dev = if addr == 0x44 {
                        i2c_lock.sht4xs.iter().find(|d| d.channel == channel && d.address == addr)
                    } else {
                        i2c_lock.sht3xs.iter().find(|d| d.channel == channel && d.address == addr)
                    };

                    if let Some(dev) = found_dev {
                        if let Some(m) = dev.model {
                            name = match m {
                                crate::i2c::i2c_sht3x_4x::ShtModel::Sht3x => "SHT30".to_string(),
                                crate::i2c::i2c_sht3x_4x::ShtModel::Sht4x => "SHT45".to_string(),
                            };
                        }
                    }
                    name
                };

                let id_t = format!("i2c:{}:0x{:02x}_T", channel, addr);
                let default_name_t = format!("{}-Temp", model_name);
                let mut entry_t = saved.remove(&id_t).unwrap_or_else(|| make_default(default_name_t.clone(), false, true, Some(addr_str.clone())));
                entry_t.present = true;
                if entry_t.address.is_none() { entry_t.address = Some(addr_str.clone()); }
                // Ajuster le nom si c'est l'ancien nom par défaut ou s'il correspond au modèle
                if entry_t.name == "SHT45-Temp" || entry_t.name == "SHT30-Temp" {
                    entry_t.name = default_name_t;
                }
                updated.insert(id_t, entry_t);

                let id_h = format!("i2c:{}:0x{:02x}_H", channel, addr);
                let default_name_h = format!("{}-Hum", model_name);
                let mut entry_h = saved.remove(&id_h).unwrap_or_else(|| make_default(default_name_h.clone(), false, true, Some(addr_str.clone())));
                entry_h.present = true;
                if entry_h.address.is_none() { entry_h.address = Some(addr_str.clone()); }
                if entry_h.name == "SHT45-Hum" || entry_h.name == "SHT30-Hum" {
                    entry_h.name = default_name_h;
                }
                updated.insert(id_h, entry_h);
            } else if addr == 0x62 {
                let id_c = format!("i2c:{}:0x{:02x}_CO2", channel, addr);
                let mut entry_c = saved.remove(&id_c).unwrap_or_else(|| make_default(format!("SCD41-CO2 (i2c:{}:0x{:02x})", channel, addr), false, true, Some(addr_str.clone())));
                entry_c.present = true;
                if entry_c.address.is_none() { entry_c.address = Some(addr_str.clone()); }
                updated.insert(id_c, entry_c);

                let id_t = format!("i2c:{}:0x{:02x}_T", channel, addr);
                let mut entry_t = saved.remove(&id_t).unwrap_or_else(|| make_default(format!("SCD41-Temp (i2c:{}:0x{:02x})", channel, addr), false, true, Some(addr_str.clone())));
                entry_t.present = true;
                if entry_t.address.is_none() { entry_t.address = Some(addr_str.clone()); }
                updated.insert(id_t, entry_t);

                let id_h = format!("i2c:{}:0x{:02x}_H", channel, addr);
                let mut entry_h = saved.remove(&id_h).unwrap_or_else(|| make_default(format!("SCD41-Hum (i2c:{}:0x{:02x})", channel, addr), false, true, Some(addr_str.clone())));
                entry_h.present = true;
                if entry_h.address.is_none() { entry_h.address = Some(addr_str.clone()); }
                updated.insert(id_h, entry_h);
            } else if addr == 0x76 || addr == 0x77 {
                // [Junior Dev Note] : Vérifier si le capteur est un BMP280 (pas d'humidité).
                let is_bmp280 = {
                    if let Ok(i2c_lock) = i2c.lock() {
                        i2c_lock.bme280s.iter().any(|b| b.channel == channel && b.address == addr && b.is_bmp280)
                    } else { false }
                };
                let model_name = if is_bmp280 { "BMP280" } else { "BME280" };

                let id_t = format!("i2c:{}:0x{:02x}_T", channel, addr);
                let mut entry_t = saved.remove(&id_t).unwrap_or_else(|| make_default(format!("{}-Temp (i2c:{}:0x{:02x})", model_name, channel, addr), false, true, Some(addr_str.clone())));
                entry_t.present = true;
                if entry_t.address.is_none() { entry_t.address = Some(addr_str.clone()); }
                updated.insert(id_t, entry_t);

                // BMP280 : pas d'humidité, on saute l'entrée _H
                if !is_bmp280 {
                    let id_h = format!("i2c:{}:0x{:02x}_H", channel, addr);
                    let mut entry_h = saved.remove(&id_h).unwrap_or_else(|| make_default(format!("{}-Hum (i2c:{}:0x{:02x})", model_name, channel, addr), false, true, Some(addr_str.clone())));
                    entry_h.present = true;
                    if entry_h.address.is_none() { entry_h.address = Some(addr_str.clone()); }
                    updated.insert(id_h, entry_h);
                }

                let id_p = format!("i2c:{}:0x{:02x}_P", channel, addr);
                let mut entry_p = saved.remove(&id_p).unwrap_or_else(|| make_default(format!("{}-Pres (i2c:{}:0x{:02x})", model_name, channel, addr), false, true, Some(addr_str.clone())));
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

        // Gérer les devices de l'ancien scan qui ne sont plus dans le nouveau scan
        for (id, mut entry) in saved {
            // `now_iso` (type: String) : Horodatage ISO 8601 courant pour marquer le début de l'absence.
            let now_iso = crate::web_handlers::get_formatted_time();
            if entry.present {
                // Le device était présent au scan précédent → il vient de disparaître → initialiser missing
                entry.missing = Some(MissingInfo {
                    since: now_iso,
                    count: 1,
                });
            } else if let Some(ref mut m) = entry.missing {
                // Le device était déjà absent → incrémenter le compteur
                m.count = m.count.saturating_add(1);
            } else {
                // Absent mais pas de compteur existant (cas rare après migration) → initialiser
                entry.missing = Some(MissingInfo {
                    since: now_iso,
                    count: 1,
                });
            }
            entry.present = false;
            updated.insert(id, entry);
        }

        // Pour les devices présents dans le nouveau scan, réinitialiser le compteur missing si existant
        for (_, entry) in updated.iter_mut() {
            if entry.present && entry.missing.is_some() {
                entry.missing = None;
            }
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
        history: &[crate::cron::MetricEntry],
    ) -> Vec<DeviceDisplay> {
        let registry = self.load_registry(); // construit à partir des statiques + NVS dynamiques
        let mut list = Vec::new();

        let mut raw_values: HashMap<String, f64> = HashMap::new();
        raw_values.insert("vsense".to_string(), (vsense_volts.unwrap_or(0.0) as f64 * 100.0).round() / 100.0);
        raw_values.insert("isense".to_string(), (isense_amps.unwrap_or(0.0) as f64 * 100.0).round() / 100.0);
        raw_values.insert("touch".to_string(), if touch_state { 1.0 } else { 0.0 });
        raw_values.insert("rla".to_string(), if relay_a_on { 1.0 } else { 0.0 });
        raw_values.insert("rlb".to_string(), if relay_b_on { 1.0 } else { 0.0 });
        raw_values.insert("swpwr".to_string(), if swpwr_on { 1.0 } else { 0.0 });
        raw_values.insert("ina".to_string(), if ina_on { 1.0 } else { 0.0 });
        raw_values.insert("inb".to_string(), if inb_on { 1.0 } else { 0.0 });
        
        // [Junior Dev Note] : `raw_values` (type HashMap<String, f64>) sert à stocker les mesures physiques brutes actuelles.
        // On y injecte dynamiquement toutes les lectures I2C lues par le Cron et stockées dans la map globale `I2C_READINGS`.
        // Cela évite d'avoir à déclarer en dur chaque identifiant de canal et adresse.
        {
            let map = crate::i2c::I2C_READINGS.lock().unwrap();
            for (k, &v) in map.iter() {
                raw_values.insert(k.clone(), (v as f64 * 100.0).round() / 100.0);
            }
        }

        for (addr, temp) in ds_readings {
            raw_values.insert(format!("onewr:{}", addr), (*temp as f64 * 100.0).round() / 100.0);
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
                "H0" => {
                    present = vsense_volts.map_or(false, |v| v > 5.0);
                    raw_val = if present { 1.0 } else { 0.0 };
                }
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
                    if let Some(&val) = raw_values.get(id) {
                        raw_val = val;
                        present = val != -255.0 && val != -254.0 && val != -253.0;
                    } else {
                        raw_val = -255.0;
                        present = false;
                    }
                }
                _ => {}
            }

            // 1. Arrondir raw_val à 2 décimales juste après la lecture pour les capteurs numériques
            if present && id != "rla" && id != "rlb" && id != "swpwr" && id != "ina" && id != "inb" && id != "screen" && id != "radio" && id != "touch" {
                raw_val = (raw_val * 100.0).round() / 100.0;
            }

            let mut final_val = raw_val;
            let is_act = matches!(id.as_str(), "rla" | "rlb" | "swpwr" | "ina" | "inb");
            if !is_act && present && correction != "x" && correction != "x.raw" && !correction.is_empty() {
                let tokens = tokenize(&correction, &raw_values, raw_val);
                if let Ok(evaluated) = evaluate_expression(&tokens) {
                    final_val = evaluated;
                }
            }

            // 2. Construire le buffer historique corrigé (10 dernières valeurs)
            let buffer_field = if is_act || id == "screen" || id == "radio" {
                None
            } else {
                let bme_t = *crate::i2c::i2c_bme280::BME280_TEMP.lock().unwrap() as f64;
                let bme_h = *crate::i2c::i2c_bme280::BME280_HUM.lock().unwrap() as f64;
                let bme_p = *crate::i2c::i2c_bme280::BME280_PRESS.lock().unwrap() as f64;
                let mut vals = Vec::new();
                for entry in history {
                    let mut h_raw = -255.0;
                    let mut h_present = true;
                    
                    match id.as_str() {
                        "vsense" => {
                            if let Some(v) = entry.readings.vsense {
                                h_raw = v as f64;
                            } else {
                                h_present = false;
                            }
                        }
                        "isense" => {
                            if let Some(i) = entry.readings.isense {
                                h_raw = i as f64;
                            } else {
                                h_present = false;
                            }
                        }
                        "touch" => {
                            if let Some(t) = entry.readings.touch {
                                h_raw = if t { 1.0 } else { 0.0 };
                            } else {
                                h_present = false;
                            }
                        }
                        _ if id.starts_with("onewr:") => {
                            let addr = &id[6..];
                            if let Some(&temp) = entry.readings.ds18b20_temperatures.get(addr) {
                                h_raw = temp as f64;
                                h_present = temp != -255.0;
                            } else {
                                h_present = false;
                            }
                        }
                        _ if id.starts_with("i2c:") => {
                            if id.ends_with("_T") && id.contains("0x44") {
                                h_raw = entry.readings.temperature_sht45 as f64;
                                h_present = h_raw != -255.0;
                            } else if id.ends_with("_H") && id.contains("0x44") {
                                h_raw = entry.readings.humidity_sht45 as f64;
                                h_present = h_raw != -255.0;
                            } else if id.ends_with("_T") && id.contains("0x62") {
                                h_raw = entry.readings.temp_scd41 as f64;
                                h_present = h_raw != -255.0;
                            } else if id.ends_with("_H") && id.contains("0x62") {
                                h_raw = entry.readings.hum_scd41 as f64;
                                h_present = h_raw != -255.0;
                            } else if id.contains("0x62") {
                                h_raw = entry.readings.co2_scd41 as f64;
                                h_present = h_raw != -255.0;
                            } else if id.ends_with("_T") && (id.contains("0x76") || id.contains("0x77")) {
                                h_raw = bme_t;
                                h_present = bme_t != -255.0 && bme_t != -254.0 && bme_t != -253.0;
                            } else if id.ends_with("_H") && (id.contains("0x76") || id.contains("0x77")) {
                                h_raw = bme_h;
                                h_present = bme_h != -255.0 && bme_h != -254.0 && bme_h != -253.0;
                            } else if id.ends_with("_P") && (id.contains("0x76") || id.contains("0x77")) {
                                h_raw = bme_p;
                                h_present = bme_p != -255.0 && bme_p != -254.0 && bme_p != -253.0;
                            } else {
                                h_present = false;
                            }
                        }
                        _ => { h_present = false; }
                    }
                    
                    if h_present {
                        h_raw = (h_raw * 100.0).round() / 100.0;
                        let mut h_final = h_raw;
                        if correction != "x" && correction != "x.raw" && !correction.is_empty() {
                            let scd_ch = crate::i2c::i2c_scd41::SCD41_CHANNEL.load(std::sync::atomic::Ordering::Relaxed);
                            let mut h_raw_values = HashMap::new();
                            h_raw_values.insert("vsense".to_string(), entry.readings.vsense.unwrap_or(0.0) as f64);
                            h_raw_values.insert("isense".to_string(), entry.readings.isense.unwrap_or(0.0) as f64);
                            h_raw_values.insert("touch".to_string(), if entry.readings.touch.unwrap_or(false) { 1.0 } else { 0.0 });
                            h_raw_values.insert("i2c:0:0x44_T".to_string(), entry.readings.temperature_sht45 as f64);
                            h_raw_values.insert("i2c:0:0x44_H".to_string(), entry.readings.humidity_sht45 as f64);
                            h_raw_values.insert(format!("i2c:{}:0x62_CO2", scd_ch), entry.readings.co2_scd41 as f64);
                            h_raw_values.insert(format!("i2c:{}:0x62_T", scd_ch), entry.readings.temp_scd41 as f64);
                            h_raw_values.insert(format!("i2c:{}:0x62_H", scd_ch), entry.readings.hum_scd41 as f64);
                            for (addr, temp) in &entry.readings.ds18b20_temperatures {
                                h_raw_values.insert(format!("onewr:{}", addr), *temp as f64);
                            }
                            
                            let tokens = tokenize(&correction, &h_raw_values, h_raw);
                            if let Ok(evaluated) = evaluate_expression(&tokens) {
                                h_final = evaluated;
                            }
                        }
                        vals.push((h_final * 100.0).round() / 100.0);
                    }
                }
                Some(vals)
            };

            let value;
            let mut unit = String::new();
            match id.as_str() {
                "rla" | "rlb" | "swpwr" => {
                    value = serde_json::json!(if final_val > 0.5 { "ON" } else { "OFF" });
                }
                "H0" => {
                    let inv = entry.inverseur.unwrap_or(true); // true = inverseur, false = indépendant
                    if !inv {
                        value = serde_json::json!(format!("INA: {}% / INB: {}%",
                            entry.ina.as_ref().map(|i| i.pwm_val).unwrap_or(0),
                            entry.inb.as_ref().map(|i| i.pwm_val).unwrap_or(0)));
                    } else {
                        let speed_a = entry.ina.as_ref().map(|i| i.pwm_val).unwrap_or(0);
                        let speed_b = entry.inb.as_ref().map(|i| i.pwm_val).unwrap_or(0);
                        if speed_a > 0 {
                            value = serde_json::json!(format!("INA: {}%", speed_a));
                        } else if speed_b > 0 {
                            value = serde_json::json!(format!("INB: {}%", speed_b));
                        } else {
                            value = serde_json::json!("OFF");
                        }
                    }
                }
                "touch" => {
                    value = serde_json::json!(if final_val > 0.5 { "TOUCHÉ" } else { "RELÂCHÉ" });
                }
                "vsense" => {
                    let val_rounded = (final_val * 100.0).round() / 100.0;
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
                        let val_rounded = (final_val * 100.0).round() / 100.0;
                        value = serde_json::json!(val_rounded);
                    }
                    unit = "°C".to_string();
                }
                _ => {
                    let val_rounded = (final_val * 100.0).round() / 100.0;
                    value = serde_json::json!(val_rounded);
                }
            }

            let is_act = matches!(id.as_str(), "rla" | "rlb" | "swpwr" | "H0");
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
                if id == "H0" {
                    Some(10)
                } else if id == "rla" || id == "rlb" || id == "swpwr" {
                    Some(100)
                } else {
                    Some(10)
                }
            } else {
                None
            });

            let display_pwm = entry.pwm_val;

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
                buffer: buffer_field,
                inverseur: entry.inverseur,
                ina: entry.ina.clone(),
                inb: entry.inb.clone(),
                rules: entry.rules.clone(),
                missing: entry.missing.clone(),
            });
        }

        // Sort: dynamic first, then static, alphabetically by ID
        list.sort_by(|a: &DeviceDisplay, b: &DeviceDisplay| {
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
                inverseur: None,
                ina: None,
                inb: None,
                rules: None,
                missing: None,
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
        rules: Option<Vec<RuleCondition>>,
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
            if rules.is_some() { entry.rules = rules; }
        }
        self.save_registry(&map);
        self.devices = map;
        Ok(())
    }

    /// Supprime définitivement un périphérique du registre NVS (via devicesKnow).
    pub fn delete_device(&mut self, id: &str) -> Result<(), anyhow::Error> {
        if is_static_device(id) {
            return Err(anyhow::anyhow!("Impossible de supprimer un périphérique statique"));
        }
        let mut map = self.load_registry();
        if map.remove(id).is_some() {
            info!("Device '{}' supprimé du registre devicesKnow", id);
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
    
    // Valeurs dynamiques réelles pour l'évaluation des formules
    raw_values.insert("vsense".to_string(), readings.vsense.unwrap_or(0.0) as f64);
    raw_values.insert("isense".to_string(), readings.isense.unwrap_or(0.0) as f64);
    raw_values.insert("touch".to_string(), if readings.touch.unwrap_or(false) { 1.0 } else { 0.0 });
    raw_values.insert("rla".to_string(), 0.0);
    raw_values.insert("rlb".to_string(), 0.0);
    raw_values.insert("swpwr".to_string(), 0.0);
    raw_values.insert("ina".to_string(), 0.0);
    raw_values.insert("inb".to_string(), 0.0);
    
    // Valeurs réelles des capteurs
    let scd_ch = crate::i2c::i2c_scd41::SCD41_CHANNEL.load(std::sync::atomic::Ordering::Relaxed);
    raw_values.insert("i2c:0:0x44_T".to_string(), readings.temperature_sht45 as f64);
    raw_values.insert("i2c:0:0x44_H".to_string(), readings.humidity_sht45 as f64);
    raw_values.insert(format!("i2c:{}:0x62_CO2", scd_ch), readings.co2_scd41 as f64);
    raw_values.insert(format!("i2c:{}:0x62_T", scd_ch), readings.temp_scd41 as f64);
    raw_values.insert(format!("i2c:{}:0x62_H", scd_ch), readings.hum_scd41 as f64);
    
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
        let scd_ch = crate::i2c::i2c_scd41::SCD41_CHANNEL.load(std::sync::atomic::Ordering::Relaxed);
        let id = format!("i2c:{}:0x62_CO2", scd_ch);
        let correction = get_correction_formula(nvs, &id);
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

    // 6. Corriger vsense
    if let Some(ref mut v) = readings.vsense {
        let id = "vsense";
        let correction = get_correction_formula(nvs, id);
        if correction != "x" && correction != "x.raw" && !correction.is_empty() {
            let tokens = tokenize(&correction, &raw_values, *v as f64);
            if let Ok(evaluated) = evaluate_expression(&tokens) {
                *v = evaluated as f32;
            }
        }
    }

    // 7. Corriger isense
    if let Some(ref mut i) = readings.isense {
        let id = "isense";
        let correction = get_correction_formula(nvs, id);
        if correction != "x" && correction != "x.raw" && !correction.is_empty() {
            let tokens = tokenize(&correction, &raw_values, *i as f64);
            if let Ok(evaluated) = evaluate_expression(&tokens) {
                *i = evaluated as f32;
            }
        }
    }
}

/// `get_corrected_sensor_values` (fonction) : Renvoie un dictionnaire de toutes les mesures corrigées des capteurs.
/// - `nvs` (type: &Arc<Mutex<NvsStorage>>) : Accès à la NVS pour lire les formules de correction.
/// - `readings` (type: &crate::sensors::SensorReadings) : Données de mesures physiques actuelles.
/// Retourne une `HashMap<String, f64>` associant chaque identifiant de capteur à sa valeur corrigée.
pub fn get_corrected_sensor_values(nvs: &Arc<Mutex<NvsStorage>>, readings: &crate::sensors::SensorReadings) -> HashMap<String, f64> {
    let mut map: HashMap<String, f64> = HashMap::new();
    
    // Valeurs brutes initiales
    map.insert("vsense".to_string(), readings.vsense.unwrap_or(0.0) as f64);
    map.insert("isense".to_string(), readings.isense.unwrap_or(0.0) as f64);
    map.insert("touch".to_string(), if readings.touch.unwrap_or(false) { 1.0 } else { 0.0 });
    
    // SHT45 (déjà corrigé si apply_sensor_corrections a été appelée)
    map.insert("i2c:0:0x44_T".to_string(), readings.temperature_sht45 as f64);
    map.insert("i2c:0:0x44_H".to_string(), readings.humidity_sht45 as f64);
    
    // CO2 SCD41 (déjà corrigé)
    let scd_ch = crate::i2c::i2c_scd41::SCD41_CHANNEL.load(std::sync::atomic::Ordering::Relaxed);
    map.insert(format!("i2c:{}:0x62_CO2", scd_ch), readings.co2_scd41 as f64);
    map.insert(format!("i2c:{}:0x62_T", scd_ch), readings.temp_scd41 as f64);
    map.insert(format!("i2c:{}:0x62_H", scd_ch), readings.hum_scd41 as f64);
    
    // DS18B20 1-wire (déjà corrigés)
    for (addr, temp) in &readings.ds18b20_temperatures {
        map.insert(format!("onewr:{}", addr), *temp as f64);
    }
    
    // Capteurs I2C génériques
    {
        if let Ok(i2c_readings) = crate::i2c::I2C_READINGS.lock() {
            for (k, &v) in i2c_readings.iter() {
                map.insert(k.clone(), v as f64);
            }
        }
    }
    
    // Appliquer les formules de correction de la NVS pour chaque capteur
    let registry: HashMap<String, PersistEntry> = {
        let storage = nvs.lock().unwrap();
        if let Ok(Some(json_str)) = storage.get_str("devicesKnow") {
            serde_json::from_str(&json_str).unwrap_or_default()
        } else {
            HashMap::new()
        }
    };
    
    // Remplacer par la valeur corrigée si une formule existe
    for (id, pe) in registry {
        if let Some(ref formula) = pe.correction_formula {
            if formula != "x" && formula != "x.raw" && !formula.is_empty() {
                if let Some(&raw_val) = map.get(&id) {
                    let tokens = tokenize(formula, &map, raw_val);
                    if let Ok(evaluated) = evaluate_expression(&tokens) {
                        map.insert(id.clone(), evaluated);
                    }
                }
            }
        }
    }
    
    map
}

/// `evaluate_logic_condition` (fonction) : Évalue une expression logique de condition.
/// - `condition` (type: &str) : Expression logique à évaluer (ex: "i2c:7:0x44_H < 50 or i2c:0:0x44_T / 2 > 12").
/// - `sensor_values` (type: &HashMap<String, f64>) : Map contenant les valeurs corrigées des capteurs.
/// Retourne `true` si la condition est remplie, `false` sinon.
pub fn evaluate_logic_condition(condition: &str, sensor_values: &HashMap<String, f64>) -> bool {
    let cond_lower: String = condition.to_lowercase();
    // Découper d'abord par "or" (si un seul bloc or est vrai, la condition est vraie)
    let or_parts: Vec<&str> = cond_lower.split("or").collect();
    
    for or_part in or_parts {
        // Chaque partie or doit être entièrement vraie (toutes les sous-parties "and" doivent être vraies)
        let and_parts: Vec<&str> = or_part.split("and").collect();
        let mut and_ok = true;
        
        for and_part in and_parts {
            let and_part_trimmed = and_part.trim();
            if and_part_trimmed.is_empty() {
                continue;
            }
            
            // Trouver l'opérateur de comparaison et évaluer
            if let Some((op, left_str, right_str)) = parse_comparison(and_part_trimmed) {
                // `left_val` (type: f64) : Valeur évaluée du membre gauche.
                let left_val: f64 = evaluate_arithmetic_expr(left_str, sensor_values).unwrap_or(0.0);
                // `right_val` (type: f64) : Valeur évaluée du membre droit.
                let right_val: f64 = evaluate_arithmetic_expr(right_str, sensor_values).unwrap_or(0.0);
                
                // `comp_result` (type: bool) : Résultat booléen de la comparaison logique.
                let comp_result = match op {
                    "<" => left_val < right_val,
                    ">" => left_val > right_val,
                    "==" => (left_val - right_val).abs() < 0.0001,
                    "<=" => left_val <= right_val,
                    ">=" => left_val >= right_val,
                    _ => false,
                };
                log::info!("[EVALUATE IF] Sub-condition '{}' -> Gauche: '{}' ({:.2}), Droite: '{}' ({:.2}) -> {}", 
                    and_part_trimmed, left_str, left_val, right_str, right_val, comp_result);
                if !comp_result {
                    and_ok = false;
                    break;
                }
            } else {
                // Pas de comparateur : si l'expression arithmétique simple est non-nulle, on considère que c'est vrai
                // `val` (type: f64) : Valeur évaluée de l'expression simple.
                let val: f64 = evaluate_arithmetic_expr(and_part_trimmed, sensor_values).unwrap_or(0.0);
                // `comp_result` (type: bool) : Vrai si la valeur évaluée est non-nulle.
                let comp_result = val != 0.0;
                log::info!("[EVALUATE IF] Expression simple '{}' (Valeur: {:.2}) -> {}", and_part_trimmed, val, comp_result);
                if !comp_result {
                    and_ok = false;
                    break;
                }
            }
        }
        
        if and_ok {
            return true; // Un des blocs "or" est vrai
        }
    }
    
    false
}

/// `parse_comparison` (fonction) : Analyse une comparaison et sépare le membre gauche, l'opérateur et le membre droit.
/// - `expr` (type: &str) : Expression logique simple à parser.
/// Retourne une Option contenant (Opérateur, MembreGauche, MembreDroit).
fn parse_comparison(expr: &str) -> Option<(&'static str, &str, &str)> {
    if expr.contains("<=") {
        let parts: Vec<&str> = expr.split("<=").collect();
        if parts.len() == 2 { return Some(("<=", parts[0].trim(), parts[1].trim())); }
    }
    if expr.contains(">=") {
        let parts: Vec<&str> = expr.split(">=").collect();
        if parts.len() == 2 { return Some((">=", parts[0].trim(), parts[1].trim())); }
    }
    if expr.contains("==") {
        let parts: Vec<&str> = expr.split("==").collect();
        if parts.len() == 2 { return Some(("==", parts[0].trim(), parts[1].trim())); }
    }
    if expr.contains('<') {
        let parts: Vec<&str> = expr.split('<').collect();
        if parts.len() == 2 { return Some(("<", parts[0].trim(), parts[1].trim())); }
    }
    if expr.contains('>') {
        let parts: Vec<&str> = expr.split('>') .collect();
        if parts.len() == 2 { return Some((">", parts[0].trim(), parts[1].trim())); }
    }
    None
}

/// `evaluate_arithmetic_expr` (fonction) : Évalue une expression arithmétique simple après substitution des valeurs de capteurs.
/// - `expr` (type: &str) : Expression mathématique.
/// - `sensor_values` (type: &HashMap<String, f64>) : Map de valeurs de capteurs.
/// Retourne le résultat du calcul arithmétique ou une erreur.
pub fn evaluate_arithmetic_expr(expr: &str, sensor_values: &HashMap<String, f64>) -> Result<f64, anyhow::Error> {
    let tokens: Vec<String> = tokenize(expr, sensor_values, 0.0);
    evaluate_expression(&tokens)
}


