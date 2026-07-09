// screen_browse.rs IHM

use embedded_graphics::{
    prelude::*,
    mono_font::{iso_8859_1::{FONT_10X20, FONT_6X10}, MonoTextStyleBuilder},
    pixelcolor::Rgb565,
    text::Text,
    primitives::{Rectangle, PrimitiveStyle},
};
use std::sync::{Arc, Mutex};
use common::nvs_storage::NvsStorage;
use crate::actuators::{Actuators, ActuatorsState};
use crate::board::Board;
use crate::wifi::NetManager;
use qrcodegen::{QrCode, QrCodeEcc};

pub const SLIDER_STEPS: &[u8] = &[1, 2, 4, 5, 10, 20, 25, 50, 100];

fn get_step_idx_from_val(val: u8) -> usize {
    SLIDER_STEPS.iter().position(|&s| s == val).unwrap_or(4)
}

/// Liste des versions OTA disponibles : (version_string, is_recommended, url)
pub static OTA_VERSIONS: std::sync::Mutex<Option<Vec<(String, bool, String)>>> = std::sync::Mutex::new(None);
pub static OTA_FETCH_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmActionType {
    MiseEnVeille,
    MiseAJour,
    ConnexionWifi,
    ResetConfiguration,
}

/// Machine à états (AppState) pour la navigation exclusive et l'ajustement d'IHM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppState {
    /// Mode NaviguerSousMenu :
    /// - `main_index` : Index du menu principal actif (0: Capteurs, 1: Actionneurs, 2: Setting)
    /// - `sub_index` : Index de la ligne sélectionnée dans le sous-menu actuel
    NaviguerSousMenu {
        main_index: usize,
        sub_index: usize,
    },
    /// Mode AfficherProprietes :
    /// - `main_index` : Index du menu principal
    /// - `sub_index` : Index du sous-menu sélectionné pour afficher la fiche/schéma détaillée
    AfficherProprietes {
        main_index: usize,
        sub_index: usize,
    },
    /// Mode AjusterSlider :
    /// - `main_index` : Menu (1: Actionneurs)
    /// - `sub_index` : Actionneur modifié
    /// - `value` : Valeur du slider (0 à 100%)
    /// - `step_idx` : Index du pas d'incrément dans `SLIDER_STEPS`
    AjusterSlider {
        main_index: usize,
        sub_index: usize,
        value: u8,
        step_idx: usize,
    },
    /// Mode ConfirmerAction :
    /// - `action_type` : type d'action à confirmer
    /// - `choice` : false = Cancel, true = OK
    ConfirmerAction {
        action_type: ConfirmActionType,
        choice: bool,
    },
}

impl Default for AppState {
    fn default() -> Self {
        AppState::NaviguerSousMenu {
            main_index: 0,
            sub_index: 0,
        }
    }
}

pub struct SensorItemInfo {
    pub id: String,
    pub name: String, // Nom NVS
    pub desc: String,
    pub corrected_val: String,
    pub raw_val: String,
    pub unit: String,
    pub model_bus: String,
}

pub struct BrowseController {
    pub state: AppState,
    pub encoder_acc: i32,
    pub last_encoder_raw: i32,
    pub ap_active_until: Option<std::time::Instant>,
    pub ota_confirm_active: bool,
    pub selected_ota_ver: String,
    pub ota_cancel_choice: bool, // false = OK (installer), true = Annuler
    pub slider_step_idx: usize,
    pub needs_redraw: bool,
    pub refresh_ticks: u32,
    pub last_rendered_state: Option<AppState>,
    pub last_main_index: Option<usize>,
    pub last_sub_index: Option<usize>,
    pub last_focus: Option<usize>,
    pub selected_ver_idx: usize,
    pub selected_wifi_idx: usize,
    pub value_changed: bool,
    pub last_ap_active: Option<bool>,
}

fn has_layout_changed(s1: Option<AppState>, s2: AppState) -> bool {
    let old = match s1 {
        Some(s) => s,
        None => return true,
    };
    match (old, s2) {
        (AppState::NaviguerSousMenu { main_index: m1, sub_index: sb1 }, AppState::NaviguerSousMenu { main_index: m2, sub_index: sb2 }) => m1 != m2 || sb1 != sb2,
        (AppState::AfficherProprietes { main_index: m1, sub_index: sb1 }, AppState::AfficherProprietes { main_index: m2, sub_index: sb2 }) => m1 != m2 || sb1 != sb2,
        (AppState::AjusterSlider { main_index: m1, sub_index: sb1, .. }, AppState::AjusterSlider { main_index: m2, sub_index: sb2, .. }) => m1 != m2 || sb1 != sb2,
        (AppState::ConfirmerAction { .. }, AppState::ConfirmerAction { .. }) => false,
        _ => true,
    }
}

impl BrowseController {
    pub fn new() -> Self {
        Self {
            state: AppState::default(),
            encoder_acc: 0,
            last_encoder_raw: 0,
            ap_active_until: None,
            ota_confirm_active: false,
            selected_ota_ver: String::new(),
            ota_cancel_choice: false,
            slider_step_idx: 4, // Step par défaut 10% (index 4 dans [1, 2, 4, 5, 10, 20, 25, 50, 100])
            needs_redraw: true,
            value_changed: false,
            refresh_ticks: 0,
            last_rendered_state: None,
            last_main_index: None,
            last_sub_index: None,
            last_focus: None,
            selected_ver_idx: 1, // index 1 (version actuelle v1.2.13-0008)
            selected_wifi_idx: 0,
            last_ap_active: None,
        }
    }

    /// Récupère la liste des capteurs avec leur nom NVS, valeurs corrigées & brutes
    pub fn get_sensors_list(
        nvs: &Arc<Mutex<NvsStorage>>,
        board: &Arc<Mutex<Board>>,
        actuators_state: &Arc<Mutex<ActuatorsState>>,
    ) -> Vec<SensorItemInfo> {
        let mut result = Vec::new();

        let (ina_act, inb_act) = {
            let act = actuators_state.lock().unwrap();
            (act.ina, act.inb)
        };

        let readings = {
            let mut b = board.lock().unwrap();
            b.read_value(ina_act, inb_act)
        };

        // Charger le registre dynamique NVS
        let reg = crate::dynamic_devices::DeviceRegistry::new(Arc::clone(nvs));
        let registry_map = reg.load_registry();

        // 1. VSENSE
        let vsense_name = registry_map.get("vsense").map(|e| e.name.clone()).unwrap_or_else(|| "Tension Alim".to_string());
        let vsense_v = readings.vsense_volts.unwrap_or(0.0);
        result.push(SensorItemInfo {
            id: "vsense".to_string(),
            name: vsense_name,
            desc: "Mesure tension principale (VSENSE)".to_string(),
            corrected_val: format!("{:.2}", vsense_v),
            raw_val: format!("{:.2}", vsense_v),
            unit: "V".to_string(),
            model_bus: "ADC sur Pin 1".to_string(),
        });

        // 2. ISENSE
        let isense_name = registry_map.get("isense").map(|e| e.name.clone()).unwrap_or_else(|| "Courant Moteur".to_string());
        let isense_a = readings.isense_amps.unwrap_or(0.0);
        result.push(SensorItemInfo {
            id: "isense".to_string(),
            name: isense_name,
            desc: "Mesure courant pont H (ISENSE)".to_string(),
            corrected_val: format!("{:.2}", isense_a),
            raw_val: format!("{:.2}", isense_a),
            unit: "A".to_string(),
            model_bus: "ADC sur Pin 2".to_string(),
        });


        // 4. BME280 (Température, Humidité, Pression) si présent
        let is_bme_found = crate::i2c::i2c_bme280::BME280_FOUND.load(std::sync::atomic::Ordering::Relaxed);
        if is_bme_found {
            let t = *crate::i2c::i2c_bme280::BME280_TEMP.lock().unwrap();
            let h = *crate::i2c::i2c_bme280::BME280_HUM.lock().unwrap();
            let p = *crate::i2c::i2c_bme280::BME280_PRESS.lock().unwrap();

            let bme_t_name = registry_map.get("i2c:0:0x76_T").or_else(|| registry_map.get("i2c:0:0x77_T"))
                .map(|e| e.name.clone()).unwrap_or_else(|| "Temp Ambiante".to_string());
            result.push(SensorItemInfo {
                id: "bme280_t".to_string(),
                name: bme_t_name,
                desc: "Temperature environnement BME280".to_string(),
                corrected_val: format!("{:.1}", t),
                raw_val: format!("{:.2}", t),
                unit: "°C".to_string(),
                model_bus: "BME280 sur I2C:0 (SDA=38, SCL=37)".to_string(),
            });

            let bme_h_name = registry_map.get("i2c:0:0x76_H").or_else(|| registry_map.get("i2c:0:0x77_H"))
                .map(|e| e.name.clone()).unwrap_or_else(|| "Humidite Air".to_string());
            result.push(SensorItemInfo {
                id: "bme280_h".to_string(),
                name: bme_h_name,
                desc: "Humidite relative BME280".to_string(),
                corrected_val: format!("{:.1}", h),
                raw_val: format!("{:.2}", h),
                unit: "%RH".to_string(),
                model_bus: "BME280 sur I2C:0 (SDA=38, SCL=37)".to_string(),
            });

            let bme_p_name = registry_map.get("i2c:0:0x76_P").or_else(|| registry_map.get("i2c:0:0x77_P"))
                .map(|e| e.name.clone()).unwrap_or_else(|| "Pression Atmo".to_string());
            result.push(SensorItemInfo {
                id: "bme280_p".to_string(),
                name: bme_p_name,
                desc: "Pression atmospherique BME280".to_string(),
                corrected_val: format!("{:.0}", p),
                raw_val: format!("{:.2}", p),
                unit: "hPa".to_string(),
                model_bus: "BME280 sur I2C:0 (SDA=38, SCL=37)".to_string(),
            });
        }

        // 5. Sondes 1-Wire DS18B20
        let mut onewire_devices: Vec<(String, String)> = registry_map
            .iter()
            .filter(|(k, _)| k.starts_with("onewr:"))
            .map(|(k, v)| (k.clone(), v.name.clone()))
            .collect();
        onewire_devices.sort_by(|a, b| a.0.cmp(&b.0));

        let opt_temps = crate::one_wire::ONEWIRE_TEMPERATURES.lock().ok();
        for (idx, (key, ds_name)) in onewire_devices.into_iter().enumerate() {
            let addr = key[6..].to_string();
            let raw_temp = opt_temps.as_ref()
                .and_then(|guard| guard.as_ref())
                .and_then(|map| map.get(&addr).copied())
                .unwrap_or(-255.0);

            let (corrected_val, raw_val) = if raw_temp == -255.0 {
                ("--.--".to_string(), "--.--".to_string())
            } else {
                let mut final_val = raw_temp as f64;
                if let Some(entry) = registry_map.get(&key) {
                    let correction = entry.correction_formula.clone().unwrap_or_else(|| {
                        crate::dynamic_devices::get_correction_formula(nvs, &key)
                    });
                    if correction != "x" && correction != "x.raw" && !correction.is_empty() {
                        let mut raw_values = std::collections::HashMap::new();
                        raw_values.insert(key.clone(), raw_temp as f64);
                        let tokens = crate::dynamic_devices::tokenize(&correction, &raw_values, raw_temp as f64);
                        if let Ok(evaluated) = crate::dynamic_devices::evaluate_expression(&tokens) {
                            final_val = evaluated;
                        }
                    }
                }
                (format!("{:.1}", final_val), format!("{:.2}", raw_temp))
            };

            result.push(SensorItemInfo {
                id: key,
                name: ds_name,
                desc: format!("Sonde de temperature etanche 1-Wire #{}", idx + 1),
                corrected_val,
                raw_val,
                unit: "°C".to_string(),
                model_bus: format!("DS18B20 sur 1-Wire (Pin 39, Addr: {})", &addr[..std::cmp::min(addr.len(), 8)]),
            });
        }

        result
    }

    /// Traite les événements physiques de l'encodeur et des boutons pour faire évoluer la machine à états.
    pub fn process_inputs(
        &mut self,
        encoder_raw: i32,
        btn2_clicked: bool,
        btn3_clicked: bool,
        nvs: &Arc<Mutex<NvsStorage>>,
        board: &Arc<Mutex<Board>>,
        actuators: &Arc<Mutex<Actuators>>,
        actuators_state: &Arc<Mutex<ActuatorsState>>,
        wifi_manager: &Arc<Mutex<NetManager>>,
    ) {
        self.value_changed = false;

        let diff = encoder_raw - self.last_encoder_raw;
        self.last_encoder_raw = encoder_raw;

        let mut ticks_delta = 0;
        if diff != 0 {
            self.encoder_acc += diff;
            while self.encoder_acc >= 4 {
                ticks_delta += 1;
                self.encoder_acc -= 4;
            }
            while self.encoder_acc <= -4 {
                ticks_delta -= 1;
                self.encoder_acc += 4;
            }
        }

        if ticks_delta != 0 || btn2_clicked || btn3_clicked {
            self.needs_redraw = true;
        }

        let main_menu_count = 4;

        match self.state {
            AppState::NaviguerSousMenu { main_index, sub_index } => {
                // 1. Bouton 3 : Changement instantané de menu principal (0 -> 1 -> 2 -> 3 -> 0)
                if btn3_clicked {
                    let next_main = (main_index + 1) % main_menu_count;
                    let next_sub_len = match next_main {
                        0 => {
                            let sensors = Self::get_sensors_list(nvs, board, actuators_state);
                            sensors.len() + 1
                        }
                        1 => 5, // INA, INB, RLA, RLB, SWPWR
                        2 => 1, // Schema
                        3 => 6, // Wifi Client, Wifi AP, TOTP, Ecran, Update, Reset
                        _ => 1,
                    };
                    let next_sub = sub_index.min(next_sub_len - 1);
                    self.state = AppState::NaviguerSousMenu {
                        main_index: next_main,
                        sub_index: next_sub,
                    };
                    return;
                }

                let sub_len = match main_index {
                    0 => {
                        let sensors = Self::get_sensors_list(nvs, board, actuators_state);
                        sensors.len() + 1
                    }
                    1 => 5, // INA, INB, RLA, RLB, SWPWR
                    2 => 1, // Schema
                    3 => 6, // Wifi Client, Wifi AP, TOTP, Ecran, Update, Reset
                    _ => 1,
                };

                // 2. Roue codeuse : déplace le curseur de sélection haut/bas
                let mut new_sub = sub_index as i32;
                if ticks_delta != 0 {
                    new_sub += ticks_delta;
                    if new_sub < 0 {
                        new_sub = 0;
                    }
                    if new_sub >= sub_len as i32 {
                        new_sub = (sub_len - 1) as i32;
                    }
                    self.state = AppState::NaviguerSousMenu {
                        main_index,
                        sub_index: new_sub as usize,
                    };
                }

                // 3. Bouton 2 (Clic) : Valide l'élément
                if btn2_clicked {
                    match main_index {
                        0 => {
                            self.state = AppState::AfficherProprietes {
                                main_index,
                                sub_index: new_sub as usize,
                            };
                        }
                        1 => {
                            if new_sub == 4 {
                                self.state = AppState::ConfirmerAction { action_type: ConfirmActionType::MiseEnVeille, choice: false };
                            } else {
                                let (current_val, default_step) = {
                                    let acts = actuators.lock().unwrap();
                                    let val = match new_sub {
                                        0 => if acts.ina.is_active() { acts.ina.get_speed() as u8 } else { 0 },
                                        1 => if acts.inb.is_active() { acts.inb.get_speed() as u8 } else { 0 },
                                        2 => if acts.relay_a.is_active() { 100 } else { 0 },
                                        3 => if acts.relay_b.is_active() { 100 } else { 0 },
                                        _ => 0,
                                    };
                                    let actuator_id = match new_sub {
                                        0 => "ina",
                                        1 => "inb",
                                        2 => "rla",
                                        3 => "rlb",
                                        _ => "",
                                    };
                                    let step_val = {
                                        let reg = crate::dynamic_devices::DeviceRegistry::new(Arc::clone(nvs));
                                        let map = reg.load_registry();
                                        map.get(actuator_id).and_then(|e| e.step)
                                    };
                                    let step_idx = if let Some(sv) = step_val {
                                        get_step_idx_from_val(sv)
                                    } else {
                                        if new_sub == 2 || new_sub == 3 { 8 } else { self.slider_step_idx }
                                    };
                                    (val, step_idx)
                                };
                                self.state = AppState::AjusterSlider {
                                    main_index,
                                    sub_index: new_sub as usize,
                                    value: current_val,
                                    step_idx: default_step,
                                };
                            }
                        }
                        2 => {
                            self.state = AppState::AfficherProprietes {
                                main_index,
                                sub_index: 0,
                            };
                        }
                        3 => {
                            if new_sub == 1 {
                                let mut net = wifi_manager.lock().unwrap();
                                net.state = crate::wifi::NetState::ApPairing;
                                net.pairing_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(120));
                                self.ap_active_until = net.pairing_until;
                                let _ = net.setup_provisioning_ap();
                                self.state = AppState::AfficherProprietes { main_index, sub_index: 1 };
                            } else if new_sub == 3 {
                                let current_brightness = nvs.lock().unwrap().get_i32("scrBrightness").ok().flatten().unwrap_or(20) as u8;
                                self.state = AppState::AjusterSlider {
                                    main_index,
                                    sub_index: 3,
                                    value: current_brightness,
                                    step_idx: 3, // step 5%
                                };
                            } else if new_sub == 5 {
                                self.state = AppState::ConfirmerAction { action_type: ConfirmActionType::ResetConfiguration, choice: false };
                            } else {
                                self.state = AppState::AfficherProprietes {
                                    main_index,
                                    sub_index: new_sub as usize,
                                };
                            }
                        }
                        _ => {}
                    }
                }
            }

            AppState::AfficherProprietes { main_index, sub_index } => {
                if main_index == 3 && sub_index == 0 {
                    let known_wifis = nvs.lock().unwrap().get_known_networks().unwrap_or_default();
                    let mut all_known = Vec::new();
                    for (ssid, entry) in known_wifis.iter() {
                        let is_present = wifi_manager.lock().unwrap().scan_cache.contains(ssid);
                        all_known.push((ssid.clone(), entry.psk.clone(), is_present));
                    }
                    all_known.sort_by(|a, b| a.0.cmp(&b.0));

                    if !all_known.is_empty() {
                        if ticks_delta != 0 {
                            let mut new_idx = self.selected_wifi_idx as i32 + ticks_delta;
                            if new_idx < 0 { new_idx = 0; }
                            if new_idx >= all_known.len() as i32 { new_idx = all_known.len() as i32 - 1; }
                            self.selected_wifi_idx = new_idx as usize;
                            self.needs_redraw = false;
                            self.value_changed = true;
                        }
                        if btn2_clicked {
                            if self.selected_wifi_idx < all_known.len() {
                                self.state = AppState::ConfirmerAction {
                                    action_type: ConfirmActionType::ConnexionWifi,
                                    choice: false,
                                };
                            }
                        }
                    }
                    if btn3_clicked {
                        self.state = AppState::NaviguerSousMenu { main_index, sub_index };
                    }
                } else if main_index == 3 && sub_index == 4 {
                    if ticks_delta != 0 {
                        let mut new_ver = self.selected_ver_idx as i32 + ticks_delta;
                        if new_ver < 0 { new_ver = 0; }
                        if new_ver >= 4 { new_ver = 3; }
                        self.selected_ver_idx = new_ver as usize;
                        self.needs_redraw = false;
                        self.value_changed = true;
                    }
                    if btn2_clicked {
                        self.state = AppState::ConfirmerAction { action_type: ConfirmActionType::MiseAJour, choice: false };
                    }
                    if btn3_clicked {
                        self.state = AppState::NaviguerSousMenu { main_index, sub_index };
                    }
                } else {
                    if btn2_clicked || btn3_clicked {
                        self.state = AppState::NaviguerSousMenu { main_index, sub_index };
                    }
                }
            }

            AppState::AjusterSlider { main_index, sub_index, value, step_idx } => {
                let mut current_step_idx = step_idx;
                if btn3_clicked {
                    if main_index == 3 && sub_index == 3 {
                        let current_timeout = nvs.lock().unwrap().get_i32("scrTimeout").ok().flatten().unwrap_or(5);
                        let next_timeout = match current_timeout {
                            1 => 5,
                            5 => 30,
                            30 => 0, // Jamais
                            _ => 1,
                        };
                        let _ = nvs.lock().unwrap().set_i32("scrTimeout", next_timeout);
                    } else {
                        current_step_idx = (current_step_idx + 1) % SLIDER_STEPS.len();
                        self.slider_step_idx = current_step_idx;

                        // Enregistrer le pas mis à jour dans la NVS
                        if main_index == 1 {
                            let actuator_id = match sub_index {
                                0 => "ina",
                                1 => "inb",
                                2 => "rla",
                                3 => "rlb",
                                _ => "",
                            };
                            if !actuator_id.is_empty() {
                                let registry = crate::dynamic_devices::DeviceRegistry::new(Arc::clone(nvs));
                                let mut map = registry.load_registry();
                                if let Some(entry) = map.get_mut(actuator_id) {
                                    entry.step = Some(SLIDER_STEPS[current_step_idx]);
                                }
                                registry.save_registry(&map);
                            }
                        }
                        self.needs_redraw = false;
                        self.value_changed = true;
                    }
                }

                let step_val = SLIDER_STEPS[current_step_idx] as i32;

                let mut val = value as i32;
                if ticks_delta != 0 {
                    val += ticks_delta * step_val;
                    let min_val = if main_index == 3 && sub_index == 3 { 5 } else { 0 };
                    if val < min_val { val = min_val; }
                    if val > 100 { val = 100; }
                    self.needs_redraw = false;
                    self.value_changed = true;

                    if main_index == 3 && sub_index == 3 {
                        let _ = nvs.lock().unwrap().set_i32("scrBrightness", val);
                        let registry = crate::dynamic_devices::DeviceRegistry::new(Arc::clone(nvs));
                        let mut map = registry.load_registry();
                        if let Some(entry) = map.get_mut("screen") {
                            entry.pwm_val = Some(val as u8);
                            entry.step = None;
                        } else {
                            map.insert("screen".to_string(), crate::dynamic_devices::DeviceEntry {
                                name: "Ecran ST7789".to_string(),
                                is_static: false,
                                present: true,
                                address: None,
                                polarity: None,
                                unit: None,
                                uncertainty: None,
                                range: None,
                                correction_formula: None,
                                step: None,
                                pwm_val: Some(val as u8),
                                schedules: None,
                            });
                        }
                        registry.save_registry(&map);
                    } else {
                        let is_active = val > 0;
                        let mut acts = actuators.lock().unwrap();
                        let mut st = actuators_state.lock().unwrap();
                        let actuator_id = match sub_index {
                            0 => {
                                let _ = acts.write("ina", is_active);
                                let _ = acts.ina.set_speed(val);
                                st.ina = is_active;
                                "ina"
                            }
                            1 => {
                                let _ = acts.write("inb", is_active);
                                let _ = acts.inb.set_speed(val);
                                st.inb = is_active;
                                "inb"
                            }
                            2 => {
                                let _ = acts.write("rla", is_active);
                                st.rla = is_active;
                                "rla"
                            }
                            3 => {
                                let _ = acts.write("rlb", is_active);
                                st.rlb = is_active;
                                "rlb"
                            }
                            _ => "",
                        };
                        if !actuator_id.is_empty() {
                            let registry = crate::dynamic_devices::DeviceRegistry::new(Arc::clone(nvs));
                            let mut map = registry.load_registry();
                            if let Some(entry) = map.get_mut(actuator_id) {
                                entry.pwm_val = Some(val as u8);
                                entry.step = Some(SLIDER_STEPS[current_step_idx]);
                            }
                            registry.save_registry(&map);
                        }
                    }
                }

                if btn2_clicked {
                    if main_index == 3 && sub_index == 3 {
                        // Just exit
                    } else {
                        let is_active = val > 0;
                        let mut acts = actuators.lock().unwrap();
                        let mut st = actuators_state.lock().unwrap();

                        let actuator_id = match sub_index {
                            0 => {
                                let _ = acts.write("ina", is_active);
                                let _ = acts.ina.set_speed(val);
                                st.ina = is_active;
                                "ina"
                            }
                            1 => {
                                let _ = acts.write("inb", is_active);
                                let _ = acts.inb.set_speed(val);
                                st.inb = is_active;
                                "inb"
                            }
                            2 => {
                                let _ = acts.write("rla", is_active);
                                st.rla = is_active;
                                "rla"
                            }
                            3 => {
                                let _ = acts.write("rlb", is_active);
                                st.rlb = is_active;
                                "rlb"
                            }
                            _ => "",
                        };
                        if !actuator_id.is_empty() {
                            let registry = crate::dynamic_devices::DeviceRegistry::new(Arc::clone(nvs));
                            let mut map = registry.load_registry();
                            if let Some(entry) = map.get_mut(actuator_id) {
                                entry.pwm_val = Some(val as u8);
                                entry.step = Some(SLIDER_STEPS[current_step_idx]);
                            }
                            registry.save_registry(&map);
                        }
                    }

                    self.state = AppState::NaviguerSousMenu { main_index, sub_index };
                } else {
                    self.state = AppState::AjusterSlider {
                        main_index,
                        sub_index,
                        value: val as u8,
                        step_idx: current_step_idx,
                    };
                }
            }
            AppState::ConfirmerAction { action_type, mut choice } => {
                if ticks_delta != 0 {
                    choice = !choice;
                    self.state = AppState::ConfirmerAction { action_type, choice };
                }

                if btn2_clicked {
                    match action_type {
                        ConfirmActionType::MiseEnVeille => {
                            if choice {
                                log::info!("SWPWR désactivé (Mise en veille) !");
                                let mut acts = actuators.lock().unwrap();
                                let _ = acts.write("swpwr", false);
                                let mut st = actuators_state.lock().unwrap();
                                st.swpwr = false;
                            }
                            self.state = AppState::NaviguerSousMenu { main_index: 1, sub_index: 4 };
                        }
                        ConfirmActionType::MiseAJour => {
                            if choice {
                                log::info!("Lancement de la mise à jour OTA...");
                                let versions = ["v1.2.13-0007", "v1.2.13-0008", "v1.2.13-0009", "v1.2.13-0010"];
                                let selected = versions[self.selected_ver_idx];
                                log::info!("Mise à jour vers {} validée !", selected);
                                let mut st = nvs.lock().unwrap();
                                let _ = st.set_str("otaVersionRequested", selected);
                                let _ = st.set_i32("otaTrigger", 1);
                                unsafe { esp_idf_sys::esp_restart(); }
                            }
                            self.state = AppState::AfficherProprietes { main_index: 3, sub_index: 4 };
                        }
                        ConfirmActionType::ConnexionWifi => {
                            if choice {
                                {
                                    *crate::wifi::CURRENT_SSID.lock().unwrap() = String::new();
                                    *crate::wifi::CURRENT_IP.lock().unwrap() = String::new();
                                }
                                self.needs_redraw = true;

                                let known_wifis = nvs.lock().unwrap().get_known_networks().unwrap_or_default();
                                let mut all_known = Vec::new();
                                for (ssid, entry) in known_wifis.iter() {
                                    let is_present = wifi_manager.lock().unwrap().scan_cache.contains(ssid);
                                    all_known.push((ssid.clone(), entry.psk.clone(), is_present));
                                }
                                all_known.sort_by(|a, b| a.0.cmp(&b.0));

                                if self.selected_wifi_idx < all_known.len() {
                                    let (ssid, psk, _) = &all_known[self.selected_wifi_idx];
                                    let ssid = ssid.clone();
                                    let psk = psk.clone();

                                    log::info!("Manuel Wi-Fi connection trigger to: {}", ssid);
                                    let wifi_manager_clone = Arc::clone(&wifi_manager);
                                    let nvs_clone = Arc::clone(nvs);
                                    let _ = std::thread::Builder::new()
                                        .name("wifi_manual_connect".to_string())
                                        .stack_size(8192)
                                        .spawn(move || {
                                            let mut net = wifi_manager_clone.lock().unwrap();
                                            if net.try_sta_connect(&ssid, &psk, false, 0).unwrap_or(false) {
                                                net.state = crate::wifi::NetState::WifiOk;
                                                net.retry_count = 0;
                                                let _ = net.stop_provisioning_ap_if_not_pairing();
                                                common::led::set_sta_status(common::led::LedStaStatus::WifiOk);
                                                common::led::set_ap_status(common::led::LedApStatus::Off);
                                                if let Ok(mut storage) = nvs_clone.lock() {
                                                    let _ = storage.set_default_network_by_ssid(&ssid);
                                                    let _ = storage.update_wifi_last_seen(&ssid);
                                                }
                                            } else {
                                                net.state = crate::wifi::NetState::WifiPreferred;
                                            }
                                        });
                                }
                            }
                            self.state = AppState::AfficherProprietes { main_index: 3, sub_index: 0 };
                        }
                        ConfirmActionType::ResetConfiguration => {
                            if choice {
                                log::info!("Demande de reset usine...");
                                if let Ok(mut storage) = nvs.lock() {
                                    let _ = storage.factory_reset();
                                }
                                unsafe { esp_idf_sys::esp_restart(); }
                            }
                            self.state = AppState::NaviguerSousMenu { main_index: 3, sub_index: 5 };
                        }
                    }
                }

                if btn3_clicked {
                    match action_type {
                        ConfirmActionType::MiseEnVeille => {
                            self.state = AppState::NaviguerSousMenu { main_index: 1, sub_index: 4 };
                        }
                        ConfirmActionType::MiseAJour => {
                            self.state = AppState::AfficherProprietes { main_index: 3, sub_index: 4 };
                        }
                        ConfirmActionType::ConnexionWifi => {
                            self.state = AppState::AfficherProprietes { main_index: 3, sub_index: 0 };
                        }
                        ConfirmActionType::ResetConfiguration => {
                            self.state = AppState::NaviguerSousMenu { main_index: 3, sub_index: 5 };
                        }
                    }
                }
            }
        }
    }

    /// Rendu graphique de l'IHM dans la zone centrale (Y=17..218, X=0..320)
    pub fn draw<D>(
        &mut self,
        display: &mut D,
        nvs: &Arc<Mutex<NvsStorage>>,
        board: &Arc<Mutex<Board>>,
        actuators: &Arc<Mutex<Actuators>>,
        actuators_state: &Arc<Mutex<ActuatorsState>>,
        wifi_manager: &Arc<Mutex<NetManager>>,
        periodic_update: bool,
    ) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        if !self.needs_redraw && !self.value_changed && !periodic_update {
            return Ok(());
        }

        let _needs_redraw_full = self.needs_redraw;
        self.needs_redraw = false;
        let val_changed = self.value_changed;
        self.value_changed = false;
        let periodic_update = periodic_update && !_needs_redraw_full;

        let layout_changed = has_layout_changed(self.last_rendered_state, self.state);

        let (current_main, current_sub) = match self.state {
            AppState::NaviguerSousMenu { main_index, sub_index } => (main_index, sub_index),
            AppState::AfficherProprietes { main_index, sub_index } => (main_index, sub_index),
            AppState::AjusterSlider { main_index, sub_index, .. } => (main_index, sub_index),
            AppState::ConfirmerAction { action_type, .. } => match action_type {
                ConfirmActionType::MiseEnVeille => (1, 4),
                ConfirmActionType::MiseAJour => (3, 4),
                ConfirmActionType::ConnexionWifi => (3, 0),
                ConfirmActionType::ResetConfiguration => (3, 5),
            },
        };

        let current_focus = match self.state {
            AppState::NaviguerSousMenu { .. } => 1, // sous-menu
            _ => 2, // détail
        };

        let in_modal = matches!(self.state, AppState::ConfirmerAction { .. });

        // Zone 1: Main menu (Y=17..32) -> Jamais effacée, juste réécrite.

        let exited_modal = matches!(self.last_rendered_state, Some(AppState::ConfirmerAction { .. })) && !matches!(self.state, AppState::ConfirmerAction { .. });
        let main_changed = self.last_main_index != Some(current_main) || exited_modal || _needs_redraw_full;
        let sub_changed = self.last_sub_index != Some(current_sub) || main_changed || layout_changed || _needs_redraw_full;

        if !in_modal && !periodic_update && !val_changed {
            if current_main == 2 {
                if main_changed {
                    // Efface toute la zone centrale (sous-menu + détail) lors du passage au mode Schéma
                    let _ = Rectangle::new(Point::new(0, 33), Size::new(320, 186))
                        .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
                        .draw(display);
                    self.last_main_index = Some(current_main);
                    self.last_sub_index = Some(current_sub);
                }
            } else {
                // Zone 2: Sous-menus de gauche (X=0..101, Y=33..218) -> Effacée seulement au changement de main menu ou à la sortie de la modale
                if main_changed {
                    // Si on change de menu principal, on invalide la liste OTA pour forcer un rafraîchissement au prochain affichage
                    if let Ok(mut guard) = OTA_VERSIONS.lock() {
                        *guard = None;
                    }
                    // Efface le volet sous-menu de gauche lors d'un changement de menu principal
                    let _ = Rectangle::new(Point::new(0, 33), Size::new(101, 186))
                        .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
                        .draw(display);
                    self.last_main_index = Some(current_main);
                }

                // Zone 3: Zone de détail à droite (X=101..320, Y=33..218) -> Effacée au changement de menu ou d'état/mode
                if sub_changed {
                    // Efface le panneau de détail de droite lors d'un changement de sous-menu
                    let _ = Rectangle::new(Point::new(101, 33), Size::new(219, 186))
                        .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
                        .draw(display);
                    self.last_sub_index = Some(current_sub);
                }
            }
        }

        // Zone 4: Dessin du cadre actif (skip pendant la modale, skip en mode Schéma, skip si periodic_update ou val_changed)
        if !in_modal && current_main != 2 && !periodic_update && !val_changed {
            // Effacer les anciens cadres de focus en les redessinant en noir
            let clear_style = PrimitiveStyle::with_stroke(Rgb565::BLACK, 1);
            let _ = Rectangle::new(Point::new(0, 33), Size::new(101, 186)).into_styled(clear_style).draw(display);
            let _ = Rectangle::new(Point::new(101, 33), Size::new(219, 186)).into_styled(clear_style).draw(display);

            // Dessiner le cadre de la zone active (en vert)
            let active_style = PrimitiveStyle::with_stroke(Rgb565::GREEN, 1);
            if current_focus == 1 {
                let _ = Rectangle::new(Point::new(0, 33), Size::new(101, 186)).into_styled(active_style).draw(display);
            } else if current_focus == 2 {
                let _ = Rectangle::new(Point::new(101, 33), Size::new(219, 186)).into_styled(active_style).draw(display);
            }
            self.last_focus = Some(current_focus);
        }

        let font_small_white = MonoTextStyleBuilder::new().font(&FONT_6X10).text_color(Rgb565::WHITE).background_color(Rgb565::BLACK).build();
        let font_small_green = MonoTextStyleBuilder::new().font(&FONT_6X10).text_color(Rgb565::GREEN).background_color(Rgb565::BLACK).build();
        let font_small_gray  = MonoTextStyleBuilder::new().font(&FONT_6X10).text_color(Rgb565::new(15, 30, 15)).background_color(Rgb565::BLACK).build();
        let font_small_red   = MonoTextStyleBuilder::new().font(&FONT_6X10).text_color(Rgb565::RED).background_color(Rgb565::BLACK).build();
        let font_big_white   = MonoTextStyleBuilder::new().font(&FONT_10X20).text_color(Rgb565::WHITE).background_color(Rgb565::BLACK).build();
        let font_big_green   = MonoTextStyleBuilder::new().font(&FONT_10X20).text_color(Rgb565::GREEN).background_color(Rgb565::BLACK).build();
        let font_menu_active = MonoTextStyleBuilder::new().font(&FONT_6X10).text_color(Rgb565::BLACK).background_color(Rgb565::GREEN).build();

        let (current_main, current_sub) = match self.state {
            AppState::NaviguerSousMenu { main_index, sub_index } => (main_index, sub_index),
            AppState::AfficherProprietes { main_index, sub_index } => (main_index, sub_index),
            AppState::AjusterSlider { main_index, sub_index, .. } => (main_index, sub_index),
            AppState::ConfirmerAction { action_type, .. } => match action_type {
                ConfirmActionType::MiseEnVeille => (1, 4),
                ConfirmActionType::MiseAJour => (3, 4),
                ConfirmActionType::ConnexionWifi => (3, 0),
                ConfirmActionType::ResetConfiguration => (3, 5),
            },
        };

        if !periodic_update && !val_changed {
            // ── 1. LIGNE DES MENUS PRINCIPAUX (Y=18 à 30) ──
            let main_menus = ["Capteurs", "Actionneurs", "Schema", "Setting"];
            let mut x_offset = 5;
            for (idx, title) in main_menus.iter().enumerate() {
                let is_selected = idx == current_main;
                let style = if is_selected { font_menu_active } else { font_small_white };
                let _ = Text::new(title, Point::new(x_offset, 27), style).draw(display);
                x_offset += (title.len() as i32 * 6) + 12;
            }

            let _ = Rectangle::new(Point::new(0, 32), Size::new(320, 1))
                .into_styled(PrimitiveStyle::with_fill(Rgb565::new(10, 20, 10)))
                .draw(display);
        }

        if !in_modal {
            // ── 2. COLONNE DES SOUS-MENUS (X=2..98, Y=38) ──
            let sensors_list = Self::get_sensors_list(nvs, board, actuators_state);

            let sub_titles: Vec<String> = match current_main {
            0 => {
                let mut list: Vec<String> = sensors_list.iter().map(|s| {
                    format!("{:.15}", s.name)
                }).collect();
                list.push("Dashboard".to_string());
                list
            }
            1 => {
                let reg = crate::dynamic_devices::DeviceRegistry::new(Arc::clone(nvs));
                let map = reg.load_registry();
                let mut list = Vec::new();
                for id in &["ina", "inb", "rla", "rlb", "swpwr"] {
                    let default = match *id {
                        "ina" => "Sortie INA",
                        "inb" => "Sortie INB",
                        "rla" => "Relais A",
                        "rlb" => "Relais B",
                        _ => "Coupure SWPWR",
                    };
                    let name = map.get(*id).map(|e| e.name.as_str()).unwrap_or(default);
                    list.push(format!("{:.15}", name));
                }
                list
            }
            2 => vec![],
            3 => vec![
                "Wifi".to_string(),
                "AccessPoint".to_string(),
                "TOTP".to_string(),
                "Ecran".to_string(),
                "Update".to_string(),
                "Reset".to_string(),
            ],
            _ => vec![],
        };

        if !periodic_update && !val_changed {
            for (idx, stitle) in sub_titles.iter().enumerate() {
                let y_pos = 45 + (idx as i32 * 14);
                if y_pos > 210 { break; }

                let is_sub_sel = idx == current_sub;
                let prefix = if is_sub_sel { ">" } else { " " };
                let style = if is_sub_sel { font_small_green } else { font_small_white };

                let _ = Text::new(prefix, Point::new(1, y_pos), style).draw(display);
                let _ = Text::new(stitle, Point::new(8, y_pos), style).draw(display);
            }
        }

        // ── 3. PANNEAU DE DROITE (PROPRIÉTÉS / SLIDER / SCHÉMA / DASHBOARD) (X=104..316) ──
        let right_x = 104;

        match current_main {
            0 => {
                if current_sub < sensors_list.len() {
                    let s = &sensors_list[current_sub];

                    if !periodic_update && !val_changed {
                        if sub_changed {
                            let _ = Text::new(&format!("Nom: {:<20}", s.name), Point::new(right_x, 48), font_small_green).draw(display);
                            let _ = Text::new(&format!("{:<30}", s.desc), Point::new(right_x, 62), font_small_white).draw(display);

                            let model_str = format!("Bus: {}", s.model_bus);
                            let _ = Text::new(&format!("{:<30}", model_str), Point::new(right_x, 145), font_small_gray).draw(display);
                        }
                    }

                    let val_str = format!("{} {}   ", s.corrected_val, s.unit);
                    let _ = Text::new(&val_str, Point::new(right_x, 95), font_big_green).draw(display);

                    let raw_str = format!("Val brut: {} {}   ", s.raw_val, s.unit);
                    let _ = Text::new(&raw_str, Point::new(right_x, 125), font_small_gray).draw(display);
                } else if current_sub == sensors_list.len() {
                    for (i, s) in sensors_list.iter().take(15).enumerate() {
                        let col = (i % 3) as i32;
                        let row = (i / 3) as i32;
                        let cx = right_x + (col * 72);
                        let cy = 42 + (row * 33);

                        let short_name = if s.name.len() > 10 { format!("{:.9}…", s.name) } else { s.name.clone() };
                        if !periodic_update && !val_changed && sub_changed {
                            let _ = Text::new(&format!("{:<11}", short_name), Point::new(cx, cy), font_small_gray).draw(display);
                        }
                        let _ = Text::new(&format!("{:<11}", format!("{} {}", s.corrected_val, s.unit)), Point::new(cx, cy + 12), font_small_white).draw(display);
                    }
                } else {
                    if sub_changed {
                        let _ = Text::new("MODE SCHEMA", Point::new(right_x, 60), font_big_white).draw(display);
                        let _ = Text::new("Implementation ulterieure", Point::new(right_x, 90), font_small_gray).draw(display);
                    }
                }
            }
            1 => {
                if current_sub == 4 {
                    if !periodic_update && !val_changed {
                        if sub_changed {
                            let _ = Text::new("SWPWR - COUPE-CIRCUIT (GPIO21)", Point::new(right_x, 50), font_small_green).draw(display);
                            let _ = Text::new("Passe en economie d'energie.", Point::new(right_x, 70), font_small_white).draw(display);
                            let _ = Text::new("Rallumer via TOUCH", Point::new(right_x, 100), font_small_green).draw(display);
                        }
                    }
                } else {
                    if !periodic_update || val_changed || sub_changed {
                        let act_names = ["Sortie INA", "Sortie INB", "Relais A", "Relais B"];
                        let act_gpios = ["GPIO36", "GPIO35", "GPIO48", "GPIO47"];
                        let act_descs = [
                            "Commande moteur INA (PWM)",
                            "Commande moteur INB (PWM)",
                            "Relais puissance A",
                            "Relais puissance B",
                        ];
                        let name = act_names[current_sub];
                        let gpio = act_gpios[current_sub];
                        let desc = act_descs[current_sub];

                        let (cur_val, is_editing, step_idx) = match self.state {
                            AppState::AjusterSlider { sub_index, value, step_idx, .. } if sub_index == current_sub => (value, true, step_idx),
                            _ => {
                                let acts = actuators.lock().unwrap();
                                let v = match current_sub {
                                    0 => if acts.ina.is_active() { acts.ina.get_speed() as u8 } else { 0 },
                                    1 => if acts.inb.is_active() { acts.inb.get_speed() as u8 } else { 0 },
                                    2 => if acts.relay_a.is_active() { 100 } else { 0 },
                                    3 => if acts.relay_b.is_active() { 100 } else { 0 },
                                    _ => 0,
                                };
                                let actuator_id = match current_sub {
                                    0 => "ina",
                                    1 => "inb",
                                    2 => "rla",
                                    3 => "rlb",
                                    _ => "",
                                };
                                let step_val = {
                                    let reg = crate::dynamic_devices::DeviceRegistry::new(Arc::clone(nvs));
                                    let map = reg.load_registry();
                                    map.get(actuator_id).and_then(|e| e.step)
                                };
                                let step_idx = if let Some(sv) = step_val {
                                    get_step_idx_from_val(sv)
                                } else {
                                    if current_sub == 2 || current_sub == 3 { 8 } else { self.slider_step_idx }
                                };
                                (v, false, step_idx)
                            }
                        };

                        if !val_changed && sub_changed {
                            let _ = Text::new(&format!("{} ({})", name, gpio), Point::new(right_x, 48), font_small_green).draw(display);
                            let _ = Text::new(desc, Point::new(right_x, 62), font_small_white).draw(display);
                        }

                        let (ina_act, inb_act) = {
                            let state = actuators_state.lock().unwrap();
                            (state.ina, state.inb)
                        };

                        let vsense_v = board.lock().unwrap().read_value(ina_act, inb_act).vsense_volts;
                        let has_power = vsense_v.map_or(false, |v| v > 6.0);
                        let is_ina_inb = current_sub == 0 || current_sub == 1;
                        let is_warning = !has_power && is_ina_inb;

                        let bar_x = right_x + 1;
                        let bar_y = 94;
                        let bar_w = 210;
                        let bar_h = 8;
                        let fill_w = ((bar_w - 2) as u32 * cur_val as u32) / 100;

                        let stroke_color = if is_warning { Rgb565::RED } else { Rgb565::WHITE };
                        let _ = Rectangle::new(Point::new(bar_x, bar_y), Size::new(bar_w as u32, bar_h as u32))
                            .into_styled(PrimitiveStyle::with_stroke(stroke_color, 1))
                            .draw(display);

                        if fill_w > 0 {
                            let fill_color = if is_warning {
                                Rgb565::RED
                            } else if is_editing {
                                Rgb565::GREEN
                            } else {
                                Rgb565::new(0, 45, 0)
                            };
                            let _ = Rectangle::new(Point::new(bar_x + 1, bar_y + 1), Size::new(fill_w, (bar_h - 2) as u32))
                                .into_styled(PrimitiveStyle::with_fill(fill_color))
                                .draw(display);
                        }

                        let remaining_w = (bar_w - 2) as u32 - fill_w;
                        if remaining_w > 0 {
                            // Efface la partie droite non remplie du curseur/jauge (slider)
                            let _ = Rectangle::new(Point::new(bar_x + 1 + fill_w as i32, bar_y + 1), Size::new(remaining_w, (bar_h - 2) as u32))
                                .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
                                .draw(display);
                        }

                        let val_label = format!("  {}%   ", cur_val);
                        let val_w = val_label.len() as i32 * 6;
                        let label_x = bar_x + (bar_w as i32 - val_w) / 2;
                        let val_style = if is_warning {
                            font_small_red
                        } else {
                            font_small_green
                        };
                        let _ = Text::new(&val_label, Point::new(label_x, bar_y - 4), val_style).draw(display);

                        if is_warning {
                            let _ = Text::new("Power Supply Missing !", Point::new(right_x, 115), font_small_red).draw(display);
                        } else {
                            let _ = Text::new("                      ", Point::new(right_x, 115), font_small_white).draw(display);
                        }

                        let step_val = SLIDER_STEPS[step_idx];
                        let mode_str = if step_val == 100 { "Mode: Switch" } else { "Mode: PWM      " };
                        let _ = Text::new(mode_str, Point::new(right_x, 145), font_small_gray).draw(display);
                        let _ = Text::new(&format!("Step (BTN3): {}%   ", step_val), Point::new(right_x, 160), font_small_gray).draw(display);
                    }

                    // Affichage dynamique et rafraîchissement ciblé du courant ISENSE pour INA (sub=0) et INB (sub=1)
                    if current_sub == 0 || current_sub == 1 {
                        let (ina_act, inb_act) = {
                            let state = actuators_state.lock().unwrap();
                            (state.ina, state.inb)
                        };
                        let isense_a = board.lock().unwrap().read_value(ina_act, inb_act).isense_amps;
                        let isense_str = isense_a.map_or("--".to_string(), |a| format!("{:.1}", a));
                        let _ = Text::new(&format!("I={}A ", isense_str), Point::new(right_x + 130, 210), font_small_white).draw(display);
                    }
                }
            }
            2 => {
                if sub_changed {
                    let text = "320x186 pixels";
                    let text_w = text.len() as i32 * 10;
                    let x = (320 - text_w) / 2;
                    let y = 33 + (186 / 2);
                    let _ = Text::new(text, Point::new(x, y), font_big_green).draw(display);
                }
            }
            3 => {
                match current_sub {
                    0 => {
                        if !periodic_update {
                            let known_wifis = nvs.lock().unwrap().get_known_networks().unwrap_or_default();
                            
                            if !val_changed {
                                if sub_changed {
                                    let _ = Text::new("WIFI CLIENT", Point::new(right_x, 42), font_small_green).draw(display);
                                    let _ = Text::new("SSID    :", Point::new(right_x, 54), font_small_white).draw(display);
                                    let _ = Text::new("IP      :", Point::new(right_x, 66), font_small_white).draw(display);
                                    let _ = Text::new("GateWay :", Point::new(right_x, 78), font_small_white).draw(display);
                                    let _ = Text::new("Rssi    :", Point::new(right_x, 90), font_small_white).draw(display);
                                    let _ = Text::new("Psk     :", Point::new(right_x, 102), font_small_white).draw(display);
                                    let _ = Text::new("Reseaux connus:", Point::new(right_x, 120), font_small_green).draw(display);
                                }

                                let (ssid, ip, gateway, rssi_val) = {
                                    let net = wifi_manager.lock().unwrap();
                                    let s = crate::wifi::CURRENT_SSID.lock().unwrap().clone();
                                    let i = crate::wifi::CURRENT_IP.lock().unwrap().clone();
                                    let ip_info = net.wifi.wifi().sta_netif().get_ip_info().ok();
                                    let gw = ip_info.as_ref().map(|info| info.subnet.gateway.to_string()).unwrap_or_else(|| "0.0.0.0".to_string());
                                    let rssi_val = net.wifi.wifi().get_ap_info().ok().map(|info| info.signal_strength);
                                    (s, i, gw, rssi_val)
                                };

                                let current_psk = known_wifis.get(&ssid).map(|entry| entry.psk.clone()).unwrap_or_else(|| "--".to_string());

                                let ssid_str = if ssid.is_empty() { "--" } else { &ssid };
                                let _ = Text::new(&format!("{}       ", ssid_str), Point::new(right_x + 56, 54), font_small_white).draw(display);

                                let ip_str = if ip.is_empty() { "--" } else { &ip };
                                let _ = Text::new(&format!("{}       ", ip_str), Point::new(right_x + 56, 66), font_small_white).draw(display);

                                let _ = Text::new(&format!("{}       ", gateway), Point::new(right_x + 56, 78), font_small_white).draw(display);

                                let rssi_str = rssi_val.map(|r| format!("{} dBm", r)).unwrap_or_else(|| "--".to_string());
                                let _ = Text::new(&format!("{}       ", rssi_str), Point::new(right_x + 56, 90), font_small_white).draw(display);

                                let _ = Text::new(&format!("{}       ", current_psk), Point::new(right_x + 56, 102), font_small_white).draw(display);
                            }

                            // Réseaux connus en dessous (toujours redessinés si val_changed ou sub_changed)
                            let scan_results = wifi_manager.lock().unwrap().scan_results.clone();
                            let mut all_known = Vec::new();
                            for (known_ssid, entry) in known_wifis.iter() {
                                let rssi_opt = scan_results.get(known_ssid).copied();
                                all_known.push((known_ssid.clone(), entry.psk.clone(), rssi_opt));
                            }
                            all_known.sort_by(|a, b| a.0.cmp(&b.0));

                            let mut idx = 0;
                            for (i, (known_ssid, _, rssi_opt)) in all_known.iter().enumerate() {
                                if idx >= 4 { break; }
                                let suffix = match rssi_opt {
                                    Some(rssi) => format!(" {}dBm", rssi),
                                    None => " --".to_string(),
                                };
                                let is_selected = i == self.selected_wifi_idx && current_focus == 2;
                                let style = if is_selected { font_small_green } else { font_small_gray };
                                let prefix = if is_selected { ">" } else { " " };
                                let label = format!("{}{}{}", prefix, known_ssid, suffix);
                                let y = 134 + (idx * 12);
                                let _ = Text::new(&label, Point::new(right_x, y), style).draw(display);
                                idx += 1;
                            }
                        }
                    }
                    1 => {
                        let ap_until = wifi_manager.lock().unwrap().pairing_until;
                        let is_ap_active = ap_until.is_some();
                        if self.last_ap_active != Some(is_ap_active) {
                            self.last_ap_active = Some(is_ap_active);
                            self.needs_redraw = true;
                        }

                        if !periodic_update && !val_changed {
                            if sub_changed {
                                let _ = Text::new("WIFI ACCESS POINT", Point::new(right_x, 42), font_small_green).draw(display);
                                let _ = Text::new("SSID    :", Point::new(right_x, 56), font_small_white).draw(display);
                                let _ = Text::new("IP      :", Point::new(right_x, 68), font_small_white).draw(display);
                                let _ = Text::new("Psk     :", Point::new(right_x, 80), font_small_white).draw(display);
                                let _ = Text::new("clients :", Point::new(right_x, 92), font_small_white).draw(display);

                                let ap_ssid = "ESP32-Configuration";
                                let subnet = crate::wifi::AP_IP_B.load(std::sync::atomic::Ordering::Relaxed);
                                let ap_cidr = format!("192.168.{}.1/24", subnet);
                                let num_clients = crate::wifi::get_ap_num_clients();

                                let _ = Text::new(&format!("{}       ", ap_ssid), Point::new(right_x + 57, 56), font_small_white).draw(display);
                                let _ = Text::new(&format!("{}       ", ap_cidr), Point::new(right_x + 57, 68), font_small_white).draw(display);
                                let _ = Text::new(&format!("{}       ", num_clients), Point::new(right_x + 57, 92), font_small_white).draw(display);
                            }
                        }

                        // Dessin dynamique de la PSK et du QR Code (qui change selon l'état actif/inactif)
                        if !periodic_update || val_changed || sub_changed {
                            let ap_psk = if is_ap_active { "None" } else { "--" };
                            let _ = Text::new(&format!("{}      ", ap_psk), Point::new(right_x + 57, 80), font_small_white).draw(display);

                            let start_x = 215;
                            let start_y = 100;
                            if is_ap_active {
                                let qr_str = "WIFI:T:nopass;S:ESP32-Configuration;;";
                                if let Ok(qr) = QrCode::encode_text(qr_str, QrCodeEcc::Medium) {
                                    let qr_size = qr.size(); // 29 pour Version 3
                                    let module_size = 3;
                                    
                                    // Draw white quiet zone (background)
                                    let _ = Rectangle::new(Point::new(start_x - 2, start_y - 2), Size::new((qr_size * module_size + 4) as u32, (qr_size * module_size + 4) as u32))
                                        .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE))
                                        .draw(display);
                                        
                                    for y in 0..qr_size {
                                        for x in 0..qr_size {
                                            if qr.get_module(x, y) {
                                                let px = start_x + (x * module_size);
                                                let py = start_y + (y * module_size);
                                                let _ = Rectangle::new(Point::new(px, py), Size::new(module_size as u32, module_size as u32))
                                                    .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
                                                    .draw(display);
                                            }
                                        }
                                    }
                                }
                            } else {
                                // Efface la zone du QR Code en noir si le mode AP est inactif
                                let _ = Rectangle::new(Point::new(start_x - 2, start_y - 2), Size::new(93, 93))
                                    .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
                                    .draw(display);
                            }
                        }

                        let ap_until = wifi_manager.lock().unwrap().pairing_until;
                        if let Some(until) = ap_until {
                            let rem = until.checked_duration_since(std::time::Instant::now()).map(|d| d.as_secs()).unwrap_or(0);
                            let _ = Text::new(&format!("Reste: {:>3}s      ", rem), Point::new(right_x, 198), font_small_green).draw(display);

                            let bar_x = right_x + 1;
                            let bar_y = 205; // barre tout en bas
                            let bar_w = 210;
                            let bar_h = 8;
                            let fill_w = ((bar_w - 2) as u32 * rem as u32) / 120;

                            let _ = Rectangle::new(Point::new(bar_x, bar_y), Size::new(bar_w as u32, bar_h as u32))
                                .into_styled(PrimitiveStyle::with_stroke(Rgb565::WHITE, 1))
                                .draw(display);

                            if fill_w > 0 {
                                let _ = Rectangle::new(Point::new(bar_x + 1, bar_y + 1), Size::new(fill_w, (bar_h - 2) as u32))
                                    .into_styled(PrimitiveStyle::with_fill(Rgb565::GREEN))
                                    .draw(display);
                            }

                            let remaining_w = (bar_w - 2) as u32 - fill_w;
                            if remaining_w > 0 {
                                // Efface la partie droite non remplie de la jauge Wifi AP
                                let _ = Rectangle::new(Point::new(bar_x + 1 + fill_w as i32, bar_y + 1), Size::new(remaining_w, (bar_h - 2) as u32))
                                    .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
                                    .draw(display);
                            }
                        } else {
                            let _ = Text::new("Activer -> 120s ?", Point::new(right_x, 198), font_small_gray).draw(display);
                            // Efface complètement la jauge de progression Wifi AP lorsqu'il n'y a pas de pairage actif
                            let _ = Rectangle::new(Point::new(right_x + 1, 205), Size::new(210, 8))
                                .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
                                .draw(display);
                        }
                    }
                    2 => {
                        if sub_changed {
                            let _ = Text::new("SECURITE TOTP & NTP", Point::new(right_x, 45), font_small_green).draw(display);
                            let _ = Text::new("NTP Serv: ", Point::new(right_x, 62), font_small_white).draw(display);
                            let _ = Text::new("Cle TOTP en clair:", Point::new(right_x, 110), font_small_gray).draw(display);
                            let _ = Text::new(crate::TOTP_SECRET, Point::new(right_x, 125), font_small_green).draw(display);
                        }

                        let ntp_serv = nvs.lock().unwrap().get_str("ntpServer").ok().flatten().unwrap_or_else(|| "pool.ntp.org".to_string());
                        let _ = Text::new(&format!("{:<20}", ntp_serv), Point::new(right_x + 60, 62), font_small_white).draw(display);

                        let current_time_str = {
                            use std::time::{SystemTime, UNIX_EPOCH};
                            let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                            let total_mins = secs / 60;
                            let hh = (total_mins / 60) % 24;
                            let mm = total_mins % 60;
                            let ss = secs % 60;
                            format!("{:02}:{:02}:{:02} UTC", hh, mm, ss)
                        };
                        let _ = Text::new(&format!("Heure: {:<20}", current_time_str), Point::new(right_x, 80), font_small_white).draw(display);
                    }
                    3 => {
                        if sub_changed {
                            let _ = Text::new("PARAMETRES ECRAN", Point::new(right_x, 42), font_small_green).draw(display);
                            let _ = Text::new("Luminosite", Point::new(right_x, 58), font_small_white).draw(display);
                        }

                        let cur_val = nvs.lock().unwrap().get_i32("scrBrightness").ok().flatten().unwrap_or(20);
                        let timeout = nvs.lock().unwrap().get_i32("scrTimeout").ok().flatten().unwrap_or(5);

                        let bar_x = right_x + 1;
                        let bar_y = 78;
                        let bar_w = 210;
                        let bar_h = 8;
                        let fill_w = ((bar_w - 2) as u32 * cur_val as u32) / 100;

                        let _ = Rectangle::new(Point::new(bar_x, bar_y), Size::new(bar_w as u32, bar_h as u32))
                            .into_styled(PrimitiveStyle::with_stroke(Rgb565::WHITE, 1))
                            .draw(display);

                        if fill_w > 0 {
                            let fill_color = match self.state {
                                AppState::AjusterSlider { main_index: 3, sub_index: 3, .. } => Rgb565::GREEN,
                                _ => Rgb565::new(0, 45, 0),
                            };
                            let _ = Rectangle::new(Point::new(bar_x + 1, bar_y + 1), Size::new(fill_w, (bar_h - 2) as u32))
                                .into_styled(PrimitiveStyle::with_fill(fill_color))
                                .draw(display);
                        }

                        let remaining_w = (bar_w - 2) as u32 - fill_w;
                        if remaining_w > 0 {
                            // Efface la partie droite non remplie du curseur/jauge de luminosité (slider)
                            let _ = Rectangle::new(Point::new(bar_x + 1 + fill_w as i32, bar_y + 1), Size::new(remaining_w, (bar_h - 2) as u32))
                                .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
                                .draw(display);
                        }

                        let val_label = format!("   {}%   ", cur_val);
                        let val_w = val_label.len() as i32 * 6;
                        let label_x = bar_x + (bar_w as i32 - val_w) / 2;
                        let _ = Text::new(&val_label, Point::new(label_x, bar_y - 4), font_small_green).draw(display);

                        let timeout_str = match timeout {
                            0 => "Jamais",
                            1 => "1 min",
                            5 => "5 min",
                            30 => "30 min",
                            _ => "5 min",
                        };
                        let _ = Text::new(&format!("Veille (BTN3): {:<8}", timeout_str), Point::new(right_x, 110), font_small_white).draw(display);
                    }
                    4 => {
                        if sub_changed {
                            let _ = Text::new("MISE A JOUR FIRMWARE", Point::new(right_x, 42), font_small_green).draw(display);
                            let _ = Text::new(&format!("Actuelle: v{}", crate::FW_VERSION), Point::new(right_x, 56), font_small_white).draw(display);
                        }

                        // Déclencher le fetch OTA en arrière-plan si pas encore fait
                        let has_versions = OTA_VERSIONS.lock().map(|g| g.is_some()).unwrap_or(false);
                        if !has_versions && !OTA_FETCH_RUNNING.load(std::sync::atomic::Ordering::Relaxed) {
                            if let Ok(update_url) = nvs.lock().map(|s| s.get_str("updateRepoList").ok().flatten().unwrap_or_default()) {
                                if !update_url.is_empty() {
                                    OTA_FETCH_RUNNING.store(true, std::sync::atomic::Ordering::Relaxed);
                                    let _ = std::thread::Builder::new()
                                        .name("ota_list_fetch".to_string())
                                        .stack_size(8192)
                                        .spawn(move || {
                                            struct FetchGuard;
                                            impl Drop for FetchGuard {
                                                fn drop(&mut self) {
                                                    OTA_FETCH_RUNNING.store(false, std::sync::atomic::Ordering::Relaxed);
                                                }
                                            }
                                            let _guard = FetchGuard;

                                            let result = crate::web_handlers::check_updates_internal(&update_url);
                                            let mut list = Vec::new();
                                            if let Ok(json) = result {
                                                // stable (recommandée)
                                                if let Some(v) = json["stable"]["version"].as_str() {
                                                    let url = json["stable"]["url"].as_str().unwrap_or("").to_string();
                                                    list.push((v.to_string(), true, url));
                                                }
                                                // previous_stable
                                                if let Some(arr) = json["previous_stable"].as_array() {
                                                    for entry in arr {
                                                        if let Some(v) = entry["version"].as_str() {
                                                            let url = entry["url"].as_str().unwrap_or("").to_string();
                                                            list.push((v.to_string(), false, url));
                                                        }
                                                    }
                                                }
                                                // unstable
                                                if let Some(arr) = json["unstable"].as_array() {
                                                    for entry in arr {
                                                        if let Some(v) = entry["version"].as_str() {
                                                            let url = entry["url"].as_str().unwrap_or("").to_string();
                                                            list.push((v.to_string(), false, url));
                                                        }
                                                    }
                                                }
                                                
                                                if let Ok(mut guard) = OTA_VERSIONS.lock() {
                                                    if !list.is_empty() {
                                                        *guard = Some(list);
                                                    } else {
                                                        *guard = None; // Retenter plus tard si vide
                                                    }
                                                }
                                            } else {
                                                if let Ok(mut guard) = OTA_VERSIONS.lock() {
                                                    *guard = None; // Permettre des re-tentatives ultérieures
                                                }
                                            }
                                        });
                                }
                            }
                        }

                        if !periodic_update || val_changed || sub_changed {
                            let versions_snapshot: Vec<(String, bool, String)> = OTA_VERSIONS.lock()
                                .ok()
                                .and_then(|g| g.clone())
                                .unwrap_or_default();

                            if versions_snapshot.is_empty() {
                                let status = if OTA_FETCH_RUNNING.load(std::sync::atomic::Ordering::Relaxed) {
                                    "Chargement..."
                                } else {
                                    "URL non configuree"
                                };
                                let _ = Text::new(status, Point::new(right_x, 74), font_small_gray).draw(display);
                            } else {
                                let _ = Text::new("Versions disponibles:", Point::new(right_x, 74), font_small_green).draw(display);
                                let clamp = versions_snapshot.len().min(4);
                                for (i, (ver, recommended, _)) in versions_snapshot[..clamp].iter().enumerate() {
                                    let y = 88 + (i as i32 * 14);
                                    let is_selected = i == self.selected_ver_idx && current_focus == 2;
                                    let prefix = if is_selected { ">" } else { " " };
                                    let style = if is_selected { font_small_green } else { font_small_white };
                                    let suffix = if *recommended { " (recommandee)" } else { "" };
                                    let label = format!("{:<30}", format!("{}{}{}", prefix, ver, suffix));
                                    let _ = Text::new(&label, Point::new(right_x, y), style).draw(display);
                                }

                                // Ajuster la borne de sélection dans process_inputs
                                if self.selected_ver_idx >= clamp {
                                    self.selected_ver_idx = clamp.saturating_sub(1);
                                }
                            }

                            if current_focus == 2 {
                                let _ = Text::new("[BTN2] Installer [BTN3] Retour", Point::new(right_x, 205), font_small_green).draw(display);
                            } else {
                                let _ = Text::new("[BTN2] Selectionner ver.", Point::new(right_x, 205), font_small_gray).draw(display);
                            }
                        }
                    }
                    5 => {
                        if !periodic_update && !val_changed {
                            if sub_changed {
                                let _ = Text::new("REINITIALISATION USINE", Point::new(right_x, 42), font_small_green).draw(display);
                                let _ = Text::new("Efface toute la config NVS", Point::new(right_x, 56), font_small_white).draw(display);
                                let _ = Text::new("et redemarre l'appareil.", Point::new(right_x, 68), font_small_white).draw(display);
                                let _ = Text::new("Cette action est irreversible !", Point::new(right_x, 90), font_small_red).draw(display);
                            }
                        }

                        if !periodic_update {
                            if current_focus == 2 {
                                let _ = Text::new("[BTN2] Demarrer Reset", Point::new(right_x, 205), font_small_red).draw(display);
                            } else {
                                let _ = Text::new("[BTN2] Selectionner Reset", Point::new(right_x, 205), font_small_gray).draw(display);
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    if let AppState::ConfirmerAction { action_type, choice } = self.state {
            if layout_changed {
                // Cadre extérieur vert
                let _ = Rectangle::new(Point::new(68, 75), Size::new(184, 84))
                    .into_styled(PrimitiveStyle::with_fill(Rgb565::GREEN))
                    .draw(display);
                // Fond noir intérieur
                let _ = Rectangle::new(Point::new(70, 77), Size::new(180, 80))
                    .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
                    .draw(display);

                // Titre & Question selon le type
                let _ = Text::new("CONFIRMATION", Point::new(124, 95), font_small_green).draw(display);
                let question = match action_type {
                    ConfirmActionType::MiseEnVeille => "Eteindre le systeme?",
                    ConfirmActionType::MiseAJour    => "Lancer la mise a jour?",
                    ConfirmActionType::ConnexionWifi => "Se connecter au wifi?",
                    ConfirmActionType::ResetConfiguration => "Effacer et redemarrer?",
                };
                let q_len = question.len() as i32 * 6;
                let q_x = 70 + (180 - q_len) / 2;
                let _ = Text::new(question, Point::new(q_x, 115), font_small_white).draw(display);
            }

            // Boutons de choix
            let (ok_bg, ok_text, ok_stroke) = if choice {
                (Rgb565::GREEN, Rgb565::BLACK, Rgb565::GREEN)
            } else {
                (Rgb565::BLACK, Rgb565::WHITE, Rgb565::new(10, 20, 10))
            };

            let (cancel_bg, cancel_text, cancel_stroke) = if !choice {
                (Rgb565::GREEN, Rgb565::BLACK, Rgb565::GREEN)
            } else {
                (Rgb565::BLACK, Rgb565::WHITE, Rgb565::new(10, 20, 10))
            };

            // Dessin bouton OK
            let _ = Rectangle::new(Point::new(96, 130), Size::new(44, 15))
                .into_styled(PrimitiveStyle::with_fill(ok_bg))
                .draw(display);
            let _ = Rectangle::new(Point::new(96, 130), Size::new(44, 15))
                .into_styled(PrimitiveStyle::with_stroke(ok_stroke, 1))
                .draw(display);
            let ok_style = MonoTextStyleBuilder::new().font(&FONT_6X10).text_color(ok_text).background_color(ok_bg).build();
            let _ = Text::new("  OK  ", Point::new(100, 141), ok_style).draw(display);

            // Dessin bouton CANCEL
            let _ = Rectangle::new(Point::new(176, 130), Size::new(44, 15))
                .into_styled(PrimitiveStyle::with_fill(cancel_bg))
                .draw(display);
            let _ = Rectangle::new(Point::new(176, 130), Size::new(44, 15))
                .into_styled(PrimitiveStyle::with_stroke(cancel_stroke, 1))
                .draw(display);
            let cancel_style = MonoTextStyleBuilder::new().font(&FONT_6X10).text_color(cancel_text).background_color(cancel_bg).build();
            let _ = Text::new("CANCEL", Point::new(180, 141), cancel_style).draw(display);
        }

        self.last_rendered_state = Some(self.state);
        Ok(())
    }
}
