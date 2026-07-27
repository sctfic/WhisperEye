use crate::sensors::{read_sensors, SensorReadings};
use anyhow::{Context, Result};
use crate::wifi::{NetManager, NetState};
use common::nvs_storage::NvsStorage;
use log::{debug, info, warn};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime};

/// `MetricEntry` (structure) : Représente une entrée de mesure archivée dans l'historique du Cron.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MetricEntry {
    /// `timestamp` (type: u64) : Horodatage epoch de la mesure.
    pub timestamp: u64,
    /// `readings` (type: SensorReadings) : Mesures des capteurs fixes (SHT45 principal, SCD41, sondes 1-Wire).
    pub readings: SensorReadings,
    /// `i2c_readings` (type: std::collections::HashMap<String, f32>) : Snapshot de toutes les mesures I2C dynamiques à cet instant précis.
    pub i2c_readings: std::collections::HashMap<String, f32>,
}

#[allow(dead_code)]
pub enum CronMessage {
    Tick,
    ForceCheckUpdate,
    GetHistory(Sender<Vec<MetricEntry>>),
}

pub struct CronWorker {
    rx: Receiver<CronMessage>,
    history: Vec<MetricEntry>,
    nvs: Arc<Mutex<NvsStorage>>,
    wifi: Arc<Mutex<NetManager>>,
    actuators_state: Arc<Mutex<crate::actuators::ActuatorsState>>,
    static_devs: Arc<Mutex<crate::actuators::Actuators>>,
    scheduled_actions: Arc<Mutex<crate::actuators::ScheduledActions>>,
    onewire_bus: Option<Arc<Mutex<crate::one_wire::OneWire<'static>>>>,
    i2c: Arc<Mutex<crate::i2c::I2c>>,
    board: Arc<Mutex<crate::board::Board>>,
    last_metrics_run: Option<std::time::Instant>,
    last_telemetry_run: Option<std::time::Instant>,
    last_update_check_run: Option<std::time::Instant>,
}

impl CronWorker {
    pub fn new(
        rx: Receiver<CronMessage>,
        nvs: Arc<Mutex<NvsStorage>>,
        wifi: Arc<Mutex<NetManager>>,
        actuators_state: Arc<Mutex<crate::actuators::ActuatorsState>>,
        static_devs: Arc<Mutex<crate::actuators::Actuators>>,
        scheduled_actions: Arc<Mutex<crate::actuators::ScheduledActions>>,
        onewire_bus: Option<Arc<Mutex<crate::one_wire::OneWire<'static>>>>,
        i2c: Arc<Mutex<crate::i2c::I2c>>,
        board: Arc<Mutex<crate::board::Board>>,
    ) -> Self {
        Self {
            rx,
            history: Vec::with_capacity(10),
            nvs,
            wifi,
            actuators_state,
            static_devs,
            scheduled_actions,
            onewire_bus,
            i2c,
            board,
            last_metrics_run: None,
            last_telemetry_run: None,
            last_update_check_run: None,
        }
    }

    pub fn run(mut self) {
        info!("Starting Periodic Task Scheduler Worker Thread...");

        while let Ok(msg) = self.rx.recv() {
            match msg {
                CronMessage::Tick => {
                    let now_instant = std::time::Instant::now();

                    // Mettre à jour la LED en fonction de l'état du réseau
                    let current_state = {
                        let wifi = self.wifi.lock().unwrap();
                        wifi.state
                    };

                    // STA status (pulse 1)
                    let sta = match current_state {
                        NetState::WifiOk => common::led::LedStaStatus::WifiOk,
                        NetState::WifiPreferred => common::led::LedStaStatus::WifiAttempting,
                        NetState::ProvisioningScan => {
                            common::led::LedStaStatus::ProvisioningAttempting
                        }
                        NetState::ProvisioningOk => common::led::LedStaStatus::ProvisioningOk,
                        NetState::ProvisioningAp => common::led::LedStaStatus::None,
                        NetState::ApPairing => common::led::LedStaStatus::None,
                    };
                    common::led::set_sta_status(sta);

                    // AP status (pulse 2)
                    let ap = match current_state {
                        NetState::ApPairing => common::led::LedApStatus::ApPairing,
                        NetState::ProvisioningAp => common::led::LedApStatus::ProvisioningSsid,
                        _ => common::led::LedApStatus::Off,
                    };
                    common::led::set_ap_status(ap);

                    // Task 1: Collect sensor metrics every 30 seconds
                    let elapsed_metrics = self
                        .last_metrics_run
                        .map(|t| now_instant.duration_since(t))
                        .unwrap_or(Duration::from_secs(999));
                    if elapsed_metrics >= Duration::from_secs(30) {
                        self.last_metrics_run = Some(now_instant);
                        self.collect_sensor_metrics();
                        self.evaluate_event_schedulers();
                    }

                    // Task 2: Trigger simulated HTTP API every 300 seconds (5 minutes)
                    let elapsed_telemetry = self
                        .last_telemetry_run
                        .map(|t| now_instant.duration_since(t))
                        .unwrap_or(Duration::from_secs(999));
                    if elapsed_telemetry >= Duration::from_secs(300) {
                        self.last_telemetry_run = Some(now_instant);
                        self.trigger_simulated_http_api();
                    }

                    // Task 3: Check NVS target nextCheck timestamp to prevent drifts every 60 seconds
                    let elapsed_update = self
                        .last_update_check_run
                        .map(|t| now_instant.duration_since(t))
                        .unwrap_or(Duration::from_secs(999));
                    if elapsed_update >= Duration::from_secs(60) {
                        self.last_update_check_run = Some(now_instant);
                        let _ = self.evaluate_need_update_check(false);
                    }

                    // Task 4: Check and execute scheduled actions
                    let now_str = crate::web_handlers::get_formatted_time();
                    if now_str != "1970-01-01T00:00:00Z" { // Only check if NTP is synchronized
                        let mut scheds = self.scheduled_actions.lock().unwrap();
                        let mut acts = self.actuators_state.lock().unwrap();
                        let mut devs = self.static_devs.lock().unwrap();
                        let mut changed = false;

                        for (id, list) in scheds.schedules.iter_mut() {
                            while !list.is_empty() && list[0].datetime_utc <= now_str {
                                let action = list.remove(0);
                                info!("\x1b[35mExecuting scheduled action for actuator {}: setting state to {}\x1b[0m", id, action.state);
                                match id.as_str() {
                                    "rla" => { acts.rla = action.state; }
                                    "rlb" => { acts.rlb = action.state; }
                                    "swpwr" => { acts.swpwr = action.state; }
                                    "ina" => {
                                        if acts.H0.inverseur == 2 {
                                            acts.H0.speed_a = if action.state { 30 } else { 0 };
                                        } else {
                                            if action.state {
                                                acts.H0.inverseur = -1;
                                                acts.H0.speed_a = 30;
                                            } else if acts.H0.inverseur == -1 {
                                                acts.H0.inverseur = 0;
                                            }
                                        }
                                    }
                                    "inb" => {
                                        if acts.H0.inverseur == 2 {
                                            acts.H0.speed_b = if action.state { 30 } else { 0 };
                                        } else {
                                            if action.state {
                                                acts.H0.inverseur = 1;
                                                acts.H0.speed_b = 30;
                                            } else if acts.H0.inverseur == 1 {
                                                acts.H0.inverseur = 0;
                                            }
                                        }
                                    }
                                    _ => {
                                        warn!("Unknown actuator id in schedule: {}", id);
                                    }
                                }
                                if id == "ina" || id == "inb" {
                                    let _ = devs.write_h0(&acts.H0);
                                } else {
                                    let _ = devs.write(id.as_str(), action.state);
                                }
                                changed = true;
                            }
                        }
                        if changed {
                            info!("\x1b[35mActuators state updated from schedules: {:?}\x1b[0m", *acts);
                        }
                    }
                }
                CronMessage::ForceCheckUpdate => {
                    info!("Manual trigger: Forcing update check now...");
                    let _ = self.evaluate_need_update_check(true);
                }
                CronMessage::GetHistory(tx) => {
                    let _ = tx.send(self.history.clone());
                }
            }
        }
    }

    fn collect_sensor_metrics(&mut self) {
        debug!("\x1b[36m[CRON] collect_sensor_metrics() starting...\x1b[0m");
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let (onewr_probes, onewr_names) = {
            let storage = self.nvs.lock().unwrap();
            let mut list = Vec::new();
            let mut names = std::collections::HashMap::new();
            if let Ok(Some(json_str)) = storage.get_str("devicesKnow") {
                if let Ok(map) = serde_json::from_str::<
                    std::collections::HashMap<String, serde_json::Value>,
                >(&json_str)
                {
                    for (id, val) in map {
                        if id.starts_with("onewr:") {
                            let present = val
                                .get("present")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            let addr = id[6..].to_string();
                            if let Some(name_val) = val.get("name").and_then(|n| n.as_str()) {
                                names.insert(addr.clone(), name_val.to_string());
                            }
                            if present {
                                list.push(addr);
                            }
                        }
                    }
                }
            }
            if list.is_empty() {
                // Fallbacks
                list.push("28ff641e8315029c".to_string());
                list.push("28aa412e831501fa".to_string());
            }
            (list, names)
        };

        // Lire les valeurs réelles du capteur matériel I2C
        let (bme_opt, scd_opt, _sht3_opt, sht4_opt) = {
            let mut i2c_lock = self.i2c.lock().unwrap();
            i2c_lock.read_value()
        };
        if let Some(ref bme) = bme_opt {
            if let Ok(mut g_t) = crate::i2c::i2c_bme280::BME280_TEMP.lock() { *g_t = bme.temperature; }
            if let Ok(mut g_h) = crate::i2c::i2c_bme280::BME280_HUM.lock() { *g_h = bme.humidity; }
            if let Ok(mut g_p) = crate::i2c::i2c_bme280::BME280_PRESS.lock() { *g_p = bme.pressure; }
        }

        let mut readings = read_sensors(self.onewire_bus.as_ref().map(|b| b.as_ref()), &onewr_probes, &onewr_names);
        if let Ok(mut opt_temps) = crate::one_wire::ONEWIRE_TEMPERATURES.lock() {
            if opt_temps.is_none() {
                *opt_temps = Some(std::collections::HashMap::new());
            }
            if let Some(ref mut global_temps) = *opt_temps {
                *global_temps = readings.ds18b20_temperatures.clone();
            }
        }
        if let Some(ref sht) = sht4_opt {
            if let Ok(mut g_t) = crate::i2c::i2c_sht3x_4x::SHT4X_TEMP.lock() { *g_t = sht.temperature; }
            if let Ok(mut g_h) = crate::i2c::i2c_sht3x_4x::SHT4X_HUM.lock() { *g_h = sht.humidity; }
            readings.temperature_sht45 = sht.temperature;
            readings.humidity_sht45 = sht.humidity;
        }
        if let Some(ref sht) = _sht3_opt {
            if let Ok(mut g_t) = crate::i2c::i2c_sht3x_4x::SHT3X_TEMP.lock() { *g_t = sht.temperature; }
            if let Ok(mut g_h) = crate::i2c::i2c_sht3x_4x::SHT3X_HUM.lock() { *g_h = sht.humidity; }
            if sht4_opt.is_none() {
                readings.temperature_sht45 = sht.temperature;
                readings.humidity_sht45 = sht.humidity;
            }
        }
        if let Some(ref scd) = scd_opt {
            readings.co2_scd41 = scd.co2;
            readings.temp_scd41 = scd.temperature;
            readings.hum_scd41 = scd.humidity;
        }
        
        let (ina_on, inb_on) = {
            let state = self.actuators_state.lock().unwrap();
            match state.H0.inverseur {
                -1 => (true, false),
                1 => (false, true),
                2 => (state.H0.speed_a > 0, state.H0.speed_b > 0),
                _ => (false, false),
            }
        };
        let board_readings = {
            let mut b = self.board.lock().unwrap();
            b.read_value(ina_on, inb_on)
        };
        readings.vsense = board_readings.vsense_volts;
        readings.isense = board_readings.isense_amps;
        readings.touch = Some(board_readings.touch);

        crate::dynamic_devices::apply_sensor_corrections(&self.nvs, &mut readings);
        let entry = MetricEntry {
            timestamp: now,
            readings: readings.clone(),
            i2c_readings: {
                let map = crate::i2c::I2C_READINGS.lock().unwrap();
                map.clone()
            },
        };

        if self.history.len() >= 10 {
            self.history.remove(0);
        }
        self.history.push(entry);

        let registry = crate::dynamic_devices::DeviceRegistry::new(Arc::clone(&self.nvs));
        let devices = registry.load_registry();

        let mut lines: Vec<String> = vec!["\x1b[36m[CRON] Task 30s: Collected sensor metrics:".to_string()];

        // [Junior Dev Note] : e.name contient soit "SHT45", soit "SHT30" selon le modèle I2C détecté.
        // On vérifie la présence de l'un ou l'autre dans les périphériques actifs.
        let sht_present: bool = devices.values().any(|e| (e.name.contains("SHT45") || e.name.contains("SHT30")) && e.present);
        if sht_present {
            // [Junior Dev Note] : On boucle sur toutes les lectures enregistrées pour les adresses de capteurs SHT (0x44 ou 0x45)
            // afin d'afficher la valeur de chaque capteur détecté sur tous les canaux du multiplexeur.
            let map = crate::i2c::I2C_READINGS.lock().unwrap();
            for (k, &v) in map.iter() {
                if k.contains("0x44") || k.contains("0x45") {
                    let unit: &str = if k.ends_with("_T") { "°C" } else { "%RH" };
                    lines.push(format!("  SHT ({}) : {:.1}{}", k, v, unit));
                }
            }
        }

        let scd_present = devices.values().any(|e| e.name.contains("SCD41") && e.present);
        if scd_present {
            lines.push(format!("  SCD41 : CO2={} ppm", readings.co2_scd41));
        }

        let bme_present = devices.values().any(|e| e.name.contains("BME280") && e.present);
        if bme_present {
            let t = *crate::i2c::i2c_bme280::BME280_TEMP.lock().unwrap();
            let h = *crate::i2c::i2c_bme280::BME280_HUM.lock().unwrap();
            let p = *crate::i2c::i2c_bme280::BME280_PRESS.lock().unwrap();
            lines.push(format!("  BME280: Temp={:.1}C  Hum={:.1}%  Pres={:.1} hPa", t, h, p));
        }

        let valid_probes: Vec<_> = readings.ds18b20_temperatures.iter()
            .filter(|(_, &t)| t != -255.0)
            .collect();
        if !valid_probes.is_empty() {
            lines.push(format!("  DS18B20 ({} probe(s)):", valid_probes.len()));
            for (addr, temp) in &valid_probes {
                lines.push(format!("    [0x{}]: {:.2}C", addr.to_uppercase(), temp));
            }
        }

        lines.push(format!("  History: {}/{} entries\x1b[0m", self.history.len(), 10));

        info!("{}", lines.join("\n"));

        // Calcul et affichage de l'historique (bleu clair) et de la moyenne (bleu foncé)
        if !self.history.is_empty() {
            // [Junior Dev Note] : Les moyennes sont calculées à partir de self.history.

            // 1. Historique (Bleu clair \x1b[96m)
            // `hist_lines` (type: Vec<String>) : Liste des lignes de log formatées pour l'affichage de l'historique.
            let mut hist_lines: Vec<String> = vec!["\x1b[96m[CRON] Historique des mesures (10 dernières) :".to_string()];
            for (idx, entry) in self.history.iter().enumerate() {
                // `sensors_states` (type: Vec<String>) : Liste des mesures formatées pour cette entrée d'historique.
                let mut sensors_states: Vec<String> = Vec::new();

                // Regrouper toutes les mesures I2C par identifiant de périphérique (canal et adresse)
                // `i2c_by_device` (type: HashMap<String, (Option<f32>, Option<f32>)>) : Regroupe les valeurs (Température, Humidité) par périphérique I2C.
                let mut i2c_by_device: std::collections::HashMap<String, (Option<f32>, Option<f32>)> = std::collections::HashMap::new();
                for (k, &v) in &entry.i2c_readings {
                    if k.starts_with("i2c:") {
                        let dev_key: String = if k.ends_with("_T") || k.ends_with("_H") {
                            k[..k.len()-2].to_string()
                        } else {
                            k.clone()
                        };
                        let record = i2c_by_device.entry(dev_key).or_insert((None, None));
                        if k.ends_with("_T") {
                            record.0 = Some(v);
                        } else if k.ends_with("_H") {
                            record.1 = Some(v);
                        }
                    }
                }

                // `sorted_i2c_keys` (type: Vec<&String>) : Liste ordonnée des périphériques I2C pour un affichage stable.
                let mut sorted_i2c_keys: Vec<&String> = i2c_by_device.keys().collect();
                sorted_i2c_keys.sort();

                for k in sorted_i2c_keys {
                    let vals: &(Option<f32>, Option<f32>) = &i2c_by_device[k];
                    // `dev_name` (type: &str) : Nom générique du capteur déduit de son adresse.
                    let dev_name: &str = if k.contains("0x44") || k.contains("0x45") {
                        if k.contains("i2c:7:") { "SHT4x" } else { "SHT3x" }
                    } else if k.contains("0x62") {
                        "SCD41"
                    } else {
                        "I2C"
                    };
                    match (vals.0, vals.1) {
                        (Some(t), Some(h)) => {
                            sensors_states.push(format!("{}:{}: {:.1}C/{:.1}%", dev_name, &k[4..], t, h));
                        }
                        (Some(t), None) => {
                            sensors_states.push(format!("{}:{}: {:.1}C", dev_name, &k[4..], t));
                        }
                        (None, Some(h)) => {
                            sensors_states.push(format!("{}:{}: {:.1}%", dev_name, &k[4..], h));
                        }
                        _ => {}
                    }
                }

                // Formater les sondes 1-Wire
                // `ds_states` (type: Vec<String>) : Liste des températures mesurées par les sondes DS18B20 actives.
                let ds_states: Vec<String> = entry.readings.ds18b20_temperatures.iter()
                    .filter(|(_, &t)| t != -255.0)
                    .map(|(addr, t)| format!("DS[{:.6}]: {:.1}C", addr, t))
                    .collect();
                if !ds_states.is_empty() {
                    sensors_states.push(format!("1Wire: {}", ds_states.join(", ")));
                }

                hist_lines.push(format!("  [{}] {}", idx + 1, sensors_states.join(" , ")));
            }
            hist_lines.push("\x1b[0m".to_string());
            info!("{}", hist_lines.join("\n"));

            // 2. Moyennes (Bleu foncé \x1b[34m)
            // `avg_lines` (type: Vec<String>) : Liste des lignes de log formatées pour la moyenne des mesures.
            let mut avg_lines: Vec<String> = vec!["\x1b[34m[CRON] Moyenne des mesures historiques :".to_string()];
            
            // Regrouper par périphérique I2C et calculer la somme des mesures
            // `i2c_sums` (type: HashMap<String, (f32, u32, f32, u32)>) : Accumule (SommeTemp, NbrTemp, SommeHum, NbrHum) pour la moyenne.
            let mut i2c_sums: std::collections::HashMap<String, (f32, u32, f32, u32)> = std::collections::HashMap::new();
            for ent in &self.history {
                for (k, &v) in &ent.i2c_readings {
                    if k.starts_with("i2c:") {
                        let dev_key: String = if k.ends_with("_T") || k.ends_with("_H") {
                            k[..k.len()-2].to_string()
                        } else {
                            k.clone()
                        };
                        let record = i2c_sums.entry(dev_key).or_insert((0.0, 0, 0.0, 0));
                        if k.ends_with("_T") {
                            record.0 += v;
                            record.1 += 1;
                        } else if k.ends_with("_H") {
                            record.2 += v;
                            record.3 += 1;
                        }
                    }
                }
            }

            // `sorted_avg_keys` (type: Vec<&String>) : Liste ordonnée des clés de capteurs I2C pour calcul de moyenne.
            let mut sorted_avg_keys: Vec<&String> = i2c_sums.keys().collect();
            sorted_avg_keys.sort();

            for k in sorted_avg_keys {
                let sums = &i2c_sums[k];
                // `dev_name` (type: &str) : Nom générique du capteur I2C.
                let dev_name: &str = if k.contains("0x44") || k.contains("0x45") {
                    if k.contains("i2c:7:") { "SHT4x" } else { "SHT3x" }
                } else if k.contains("0x62") {
                    "SCD41"
                } else {
                    "I2C"
                };

                let t_avg: Option<f32> = if sums.1 > 0 { Some(sums.0 / sums.1 as f32) } else { None };
                let h_avg: Option<f32> = if sums.3 > 0 { Some(sums.2 / sums.3 as f32) } else { None };

                match (t_avg, h_avg) {
                    (Some(t), Some(h)) => {
                        avg_lines.push(format!("  {}:{} : Temp={:.2}C  Humi={:.2}%", dev_name, &k[4..], t, h));
                    }
                    (Some(t), None) => {
                        avg_lines.push(format!("  {}:{} : Temp={:.2}C", dev_name, &k[4..], t));
                    }
                    (None, Some(h)) => {
                        avg_lines.push(format!("  {}:{} : Humi={:.2}%", dev_name, &k[4..], h));
                    }
                    _ => {}
                }
            }

            // Pour DS18B20, on regroupe par adresse de sonde
            // `ds_sums` (type: HashMap<String, f32>) : Somme des températures par sonde DS18B20.
            let mut ds_sums: std::collections::HashMap<String, f32> = std::collections::HashMap::new();
            // `ds_counts` (type: HashMap<String, u32>) : Nombre de mesures par sonde DS18B20.
            let mut ds_counts: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
            for ent in &self.history {
                for (addr, &t) in &ent.readings.ds18b20_temperatures {
                    if t != -255.0 {
                        *ds_sums.entry(addr.clone()).or_insert(0.0) += t;
                        *ds_counts.entry(addr.clone()).or_insert(0) += 1;
                    }
                }
            }
            if !ds_sums.is_empty() {
                avg_lines.push("  DS18B20 :".to_string());
                // `sorted_keys` (type: Vec<&String>) : Liste des adresses ordonnées pour les moyennes DS18B20.
                let mut sorted_keys: Vec<&String> = ds_sums.keys().collect();
                sorted_keys.sort();
                for addr in sorted_keys {
                    let sum: f32 = ds_sums[addr];
                    let c: u32 = ds_counts[addr];
                    avg_lines.push(format!("    [0x{}]: {:.2}C (sur {} entrées)", addr.to_uppercase(), sum / c as f32, c));
                }
            }
            avg_lines.push("\x1b[0m".to_string());
            info!("{}", avg_lines.join("\n"));
        }

        // Reconnection handled asynchronously by the net_controller thread
        let mut net = self.wifi.lock().unwrap();
        if net.state == NetState::ProvisioningAp {
            info!("\x1b[36m[CRON] WhisperEye in captive portal (ProvisioningAp). Requesting periodic retry of known Wi-Fi networks (30s interval).\x1b[0m");
            net.state = NetState::WifiPreferred;
            net.last_state_change = std::time::Instant::now();
        }
    }

    fn trigger_simulated_http_api(&self) {
        let metrics_url = {
            let storage = self.nvs.lock().unwrap();
            storage
                .get_str("metricsUrl")
                .unwrap_or(None)
                .unwrap_or_default()
        };

        if metrics_url.is_empty() || metrics_url == "empty" {
            info!("\x1b[36mTelemetry skipped: metricsUrl is empty or not defined\x1b[0m");
            return;
        }

        info!(
            "\x1b[36mTask 300s: Sending HTTP PUT telemetry to {}...\x1b[0m",
            metrics_url
        );

        let payload = if let Some(last_entry) = self.history.last() {
            serde_json::to_string(last_entry).unwrap_or_default()
        } else {
            "".to_string()
        };

        if payload.is_empty() {
            warn!("Telemetry payload is empty, skipping upload");
            return;
        }

        // Perform HTTP PUT
        let config = esp_idf_svc::http::client::Configuration {
            buffer_size: Some(2048),
            crt_bundle_attach: Some(esp_idf_sys::esp_crt_bundle_attach),
            ..Default::default()
        };

        match esp_idf_svc::http::client::EspHttpConnection::new(&config) {
            Ok(mut connection) => {
                let payload_bytes = payload.as_bytes();
                let len_str = payload_bytes.len().to_string();
                let headers = [
                    ("Content-Type", "application/json"),
                    ("Content-Length", &len_str),
                ];
                if let Err(e) = connection.initiate_request(
                    esp_idf_svc::http::Method::Put,
                    &metrics_url,
                    &headers,
                ) {
                    warn!("Failed to initiate PUT request to telemetry: {:?}", e);
                    return;
                }

                if let Err(e) = connection.write_all(payload_bytes) {
                    warn!("Failed to write telemetry payload: {:?}", e);
                    return;
                }
                match connection.initiate_response() {
                    Ok(_) => {
                        info!(
                            "Telemetry HTTP PUT successfully completed to {} (Status: {})",
                            metrics_url,
                            connection.status()
                        );
                    }
                    Err(e) => {
                        warn!(
                            "Failed to receive response from telemetry endpoint: {:?}",
                            e
                        );
                    }
                }
            }
            Err(e) => {
                warn!("Failed to create EspHttpConnection for telemetry: {:?}", e);
            }
        }
    }

    fn evaluate_need_update_check(&self, force: bool) -> Result<()> {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // If system time is uninitialized or un-synchronized (mock NTP has not succeeded yet), skip check
        if now < 86400 * 365 {
            return Ok(());
        }

        let (auto_update, next_check_str, url, fw) = {
            let storage = self.nvs.lock().unwrap();
            let auto = storage.get_i32("autoUpdate")?.unwrap_or(1);
            let next = storage.get_str("nextCheck")?.unwrap_or_default();
            let repo_url = storage.get_str("updateRepoList")?.unwrap_or_default();
            let fw_version = storage.get_str("fwVersion")?.unwrap_or_else(|| "v1.0.0-poc".to_string());
            (auto, next, repo_url, fw_version)
        };

        if auto_update == 0 {
            if next_check_str != "4102387200" {
                let mut storage = self.nvs.lock().unwrap();
                storage.set_str("nextCheck", "4102387200")?;
                info!("autoUpdate is false, nextCheck set to 2099-12-31 (4102387200)");
            }
            return Ok(());
        }

        let mut next_check: u64 = next_check_str.parse().unwrap_or(0);

        if next_check == 0 || next_check_str == "4102387200" {
            // First run or transitioning from disabled: initialize target date to tomorrow at 14:00 UTC
            next_check = ((now / 86400) + 1) * 86400 + 14 * 3600;
            let mut storage = self.nvs.lock().unwrap();
            storage.set_str("nextCheck", &next_check.to_string())?;
            info!("NVS target 'nextCheck' initialized to tomorrow 14:00 UTC: {} (after transition or first run)", next_check);
            return Ok(());
        }

        if force || now >= next_check {
            info!(
                "Task 7 Days: Running check_update() check (target nextCheck: {}, current: {})",
                next_check, now
            );
            self.perform_check_update(&url, &fw)?;

            // Set new target check date to exactly 7 days from now
            let new_next_check = now + 7 * 86400;
            let mut storage = self.nvs.lock().unwrap();
            storage.set_str("nextCheck", &new_next_check.to_string())?;
            info!(
                "NVS target 'nextCheck' updated to: {} (Next 7-day target)",
                new_next_check
            );
        }

        Ok(())
    }

    fn perform_check_update(&self, url: &str, fw: &str) -> Result<()> {
        if url.is_empty() {
            warn!("check_update skipped: no updateRepoList URL configured");
            return Ok(());
        }

        info!("Sending update request to catalogue URL: {}", url);

        let config = esp_idf_svc::http::client::Configuration {
            buffer_size: Some(2048),
            crt_bundle_attach: Some(esp_idf_sys::esp_crt_bundle_attach),
            ..Default::default()
        };
        let mut connection = esp_idf_svc::http::client::EspHttpConnection::new(&config)?;
        connection.initiate_request(esp_idf_svc::http::Method::Get, url, &[])?;
        connection.initiate_response()?;

        if connection.status() != 200 {
            warn!(
                "Upstream catalog server returned HTTP status {}",
                connection.status()
            );
            return Ok(());
        }

        let mut body = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            match connection.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => body.extend_from_slice(&chunk[..n]),
                Err(e) => return Err(anyhow::anyhow!("Read error in update check: {:?}", e)),
            }
        }

        let list: serde_json::Value = serde_json::from_slice(&body)?;
        let mut new_stable_url = None;
        let mut new_version = None;

        if let Some(arr) = list.as_array() {
            for entry in arr {
                let c_type = entry.get("ChipType").and_then(|v| v.as_str()).unwrap_or("");
                if c_type == "ESP32-S3" {
                    if let Some(stable_val) = entry.get("stable") {
                        for v_obj in version_entries(stable_val) {
                            if let Some(ver_str) = v_obj.get("version").and_then(|v| v.as_str()) {
                                if let Some(url_str) = v_obj.get("url").and_then(|v| v.as_str()) {
                                    if parse_version(ver_str) > parse_version(fw) {
                                        let current_best =
                                            new_version.as_deref().unwrap_or(fw);
                                        if parse_version(ver_str) > parse_version(current_best) {
                                            new_stable_url = Some(url_str.to_string());
                                            new_version = Some(ver_str.to_string());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        if let (Some(dl_url), Some(ver)) = (new_stable_url, new_version) {
            info!(
                "Periodic update found version: {}. Arming OTA and rebooting to recovery...",
                ver
            );
            {
                let mut storage = self.nvs.lock().unwrap();
                storage.set_str("updateDlUrl", &dl_url)?;
                storage.set_i32("otaRetry", 3)?;
            }

            thread::sleep(Duration::from_secs(2));
            crate::web_handlers::set_boot_to_recovery();
            unsafe {
                esp_idf_sys::esp_restart();
            }
        } else {
            info!(
                "Periodic update check: firmware is up-to-date (Version: {})",
                fw
            );
        }

        Ok(())
    }

    /// `evaluate_event_schedulers` (fonction) : Analyse et évalue les règles événementielles
    /// stockées en NVS (clé Rule) pour chaque actionneur et met à jour leurs états physiques.
    fn evaluate_event_schedulers(&mut self) {
        debug!("[CRON] Évaluation des planificateurs événementiels...");
        
        // 1. Récupérer les dernières mesures de capteurs
        // `last_entry` (type: &MetricEntry) : Dernière entrée de l'historique des capteurs.
        let last_entry = match self.history.last() {
            Some(entry) => entry,
            None => return, // Pas de mesures encore disponibles, on attend
        };

        // Obtenir toutes les valeurs de capteurs corrigées
        // `sensor_values` (type: HashMap<String, f64>) : Map de toutes les mesures physiques corrigées.
        let sensor_values: std::collections::HashMap<String, f64> = 
            crate::dynamic_devices::get_corrected_sensor_values(&self.nvs, &last_entry.readings);

        // 2. Charger le registre des périphériques (devicesKnow) de la NVS
        // `registry` (type: HashMap<String, PersistEntry>) : Registre des périphériques stockés dans la NVS.
        let registry: std::collections::HashMap<String, crate::dynamic_devices::PersistEntry> = {
            let storage = self.nvs.lock().unwrap();
            if let Ok(Some(json_str)) = storage.get_str("devicesKnow") {
                serde_json::from_str(&json_str).unwrap_or_default()
            } else {
                std::collections::HashMap::new()
            }
        };

        // Heure UTC actuelle
        // `now_str` (type: String) : Date/heure UTC actuelle formatée au format ISO 8601 ("YYYY-MM-DDTHH:MM:SSZ").
        let now_str = crate::web_handlers::get_formatted_time();

        let mut acts = self.actuators_state.lock().unwrap();
        let mut devs = self.static_devs.lock().unwrap();
        let mut changed = false;

        // Pour chaque actionneur configuré, nous vérifions s'il a des règles
        for (id, pe) in registry.iter() {
            if let Some(ref rules) = pe.rules {
                if rules.is_empty() {
                    continue;
                }

                // Parcourir les règles. La première règle qui est vraie (short-circuit) ou qui a un ELSE défini s'applique.
                for rule in rules {
                    let mut condition_met = false;
                    let rule_name = rule.name.as_deref().unwrap_or(id);
                    
                    // Vérifier la plage UTC si elle est définie
                    let mut utc_ok = true;
                    if let Some(ref range) = rule.utc {
                        if range.len() == 2 {
                            let start = &range[0];
                            let end = &range[1];
                            // Comparaison lexicographique des chaînes de date ISO 8601
                            if now_str < *start || now_str > *end {
                                utc_ok = false;
                                info!("[EVENT SCHEDULER] Regle '{}' : Plage UTC [{}, {}] REJETEE (Heure actuelle : {})", rule_name, start, end, now_str);
                            } else {
                                info!("[EVENT SCHEDULER] Regle '{}' : Plage UTC [{}, {}] ACCEPTEE", rule_name, start, end);
                            }
                        }
                    }

                    if utc_ok {
                        // Évaluer la condition logique ("if")
                        let is_if_true = crate::dynamic_devices::evaluate_logic_condition(&rule.if_expr, &sensor_values);
                        if is_if_true {
                            info!("[EVENT SCHEDULER] Regle '{}' : Condition IF '{}' ACCEPTEE", rule_name, rule.if_expr);
                            condition_met = true;
                        } else {
                            info!("[EVENT SCHEDULER] Regle '{}' : Condition IF '{}' REJETEE", rule_name, rule.if_expr);
                        }
                    }

                    if condition_met {
                        info!("\x1b[35m[EVENT SCHEDULER] Regle '{}' vraie -> execution du THEN: {}\x1b[0m", rule_name, rule.then_expr);
                        apply_action_assignments(&rule.then_expr, id, &sensor_values, &mut acts, &mut devs, &self.nvs, &mut changed);
                        break; // Première règle active trouvée, fin de la boucle pour cet actionneur
                    } else {
                        // Si la condition est fausse, le ELSE s'applique s'il est défini
                        if let Some(ref else_expr) = rule.else_expr {
                            info!("\x1b[35m[EVENT SCHEDULER] Regle '{}' fausse -> execution du ELSE: {}\x1b[0m", rule_name, else_expr);
                            apply_action_assignments(else_expr, id, &sensor_values, &mut acts, &mut devs, &self.nvs, &mut changed);
                            break; // Le ELSE produit son effet, arrêt de l'évaluation
                        }
                    }
                }
            }
        }

        if changed {
            // Sauvegarder les nouveaux états dans devicesKnow
            let registry = crate::dynamic_devices::DeviceRegistry::new(Arc::clone(&self.nvs));
            let mut map = registry.load_registry();
            if let Some(entry) = map.get_mut("rla") {
                entry.pwm_val = Some(acts.rla_speed.unwrap_or(0));
            }
            if let Some(entry) = map.get_mut("rlb") {
                entry.pwm_val = Some(acts.rlb_speed.unwrap_or(0));
            }
            if let Some(entry) = map.get_mut("swpwr") {
                entry.pwm_val = Some(if acts.swpwr { 100 } else { 0 });
            }
            if let Some(entry) = map.get_mut("H0") {
                entry.inverseur = Some(acts.H0.inverseur);
                if let Some(ref mut ina) = entry.ina { ina.pwm_val = acts.H0.speed_a; }
                if let Some(ref mut inb) = entry.inb { inb.pwm_val = acts.H0.speed_b; }
            }
            registry.save_registry(&map);
            info!("\x1b[35m[EVENT SCHEDULER] Actuators updated physically and persisted.\x1b[0m");
        }
    }
}

/// `apply_action_assignments` (fonction) : Parse et applique les affectations de variables d'une chaîne d'action (ex: "ina=30, inb=45").
/// - `action_str` (type: &str) : La chaîne de règles ou d'expressions (ex: "ina=30, inb=i2c:7:0x44_H" ou "100").
/// - `default_device_id` (type: &str) : ID de périphérique par défaut si aucune variable n'est ciblée.
/// - `sensor_values` (type: &HashMap<String, f64>) : Map de toutes les mesures corrigées des capteurs.
/// - `acts` (type: &mut ActuatorsState) : Structure des états de la RAM.
/// - `devs` (type: &mut Actuators) : Pilotes physiques des relais/pont H.
/// - `nvs` (type: &Arc<Mutex<NvsStorage>>) : Stockage NVS.
/// - `changed` (type: &mut bool) : Indicateur de modification d'état.
fn apply_action_assignments(
    action_str: &str,
    default_device_id: &str,
    sensor_values: &std::collections::HashMap<String, f64>,
    acts: &mut crate::actuators::ActuatorsState,
    devs: &mut crate::actuators::Actuators,
    nvs: &Arc<Mutex<NvsStorage>>,
    changed: &mut bool,
) {
    if action_str.contains('=') {
        let parts: Vec<&str> = action_str.split(',').collect();
        for part in parts {
            let part_trimmed = part.trim();
            if part_trimmed.is_empty() {
                continue;
            }
            let sub_parts: Vec<&str> = part_trimmed.split('=').collect();
            if sub_parts.len() == 2 {
                let target = sub_parts[0].trim();
                let expr = sub_parts[1].trim();
                if let Ok(output_val) = crate::dynamic_devices::evaluate_arithmetic_expr(expr, sensor_values) {
                    apply_single_actuator(target, output_val, acts, devs, nvs, changed);
                } else {
                    warn!("[EVENT SCHEDULER] Impossible d'evaluer l'expression arithmetique : {}", expr);
                }
            } else {
                warn!("[EVENT SCHEDULER] Format d'affectation invalide : {}", part_trimmed);
            }
        }
    } else {
        // C'est une affectation directe simple sur le périphérique par défaut
        if let Ok(output_val) = crate::dynamic_devices::evaluate_arithmetic_expr(action_str, sensor_values) {
            apply_single_actuator(default_device_id, output_val, acts, devs, nvs, changed);
        } else {
            warn!("[EVENT SCHEDULER] Impossible d'evaluer l'expression simple : {}", action_str);
        }
    }
}

/// `apply_single_actuator` (fonction) : Applique une valeur numérique sur un unique actionneur ciblé.
/// - `target` (type: &str) : Identifiant de l'actionneur (ex: "ina", "rla", "screen").
/// - `output_val` (type: f64) : Valeur évaluée.
/// - `acts` (type: &mut ActuatorsState) : Structure des états de la RAM.
/// - `devs` (type: &mut Actuators) : Pilotes physiques.
/// - `nvs` (type: &Arc<Mutex<NvsStorage>>) : Stockage NVS.
/// - `changed` (type: &mut bool) : Indicateur de modification d'état.
fn apply_single_actuator(
    target: &str,
    output_val: f64,
    acts: &mut crate::actuators::ActuatorsState,
    devs: &mut crate::actuators::Actuators,
    nvs: &Arc<Mutex<NvsStorage>>,
    changed: &mut bool,
) {
    match target {
        "rla" => {
            let state = output_val > 0.0;
            let speed = if state { output_val.min(100.0) as u8 } else { 0 };
            info!("[EVENT SCHEDULER] Actionneur 'rla' -> application valeur: {} (Etat: {}, Vitesse: {})", output_val, state, speed);
            if acts.rla != state || acts.rla_speed != Some(speed) {
                acts.rla = state;
                acts.rla_speed = Some(speed);
                let _ = devs.relay_a.set_speed(speed as i32);
                let _ = devs.write("rla", state);
                *changed = true;
            }
        }
        "rlb" => {
            let state = output_val > 0.0;
            let speed = if state { output_val.min(100.0) as u8 } else { 0 };
            info!("[EVENT SCHEDULER] Actionneur 'rlb' -> application valeur: {} (Etat: {}, Vitesse: {})", output_val, state, speed);
            if acts.rlb != state || acts.rlb_speed != Some(speed) {
                acts.rlb = state;
                acts.rlb_speed = Some(speed);
                let _ = devs.relay_b.set_speed(speed as i32);
                let _ = devs.write("rlb", state);
                *changed = true;
            }
        }
        "swpwr" => {
            let state = output_val > 0.0;
            info!("[EVENT SCHEDULER] Actionneur 'swpwr' -> application valeur: {} (Etat: {})", output_val, state);
            if acts.swpwr != state {
                acts.swpwr = state;
                let _ = devs.write("swpwr", state);
                *changed = true;
            }
        }
        "screen" => {
            let brightness = output_val.max(0.0).min(100.0) as i32;
            info!("[EVENT SCHEDULER] Actionneur 'screen' -> application valeur: {} (Luminosite: {}%)", output_val, brightness);
            if acts.screen_brightness != Some(brightness as u8) {
                acts.screen_brightness = Some(brightness as u8);
                // Sauvegarder la nouvelle luminosité de l'écran en NVS
                let mut storage = nvs.lock().unwrap();
                let _ = storage.set_i32("scrBrightness", brightness);
                *changed = true;
            }
        }
        "ina" => {
            let speed = output_val.max(0.0).min(100.0) as u8;
            info!("[EVENT SCHEDULER] Actionneur 'ina' (mode independant) -> application valeur: {} (Vitesse: {})", output_val, speed);
            if acts.H0.inverseur != 2 || acts.H0.speed_a != speed {
                acts.H0.inverseur = 2; // Forcer le mode indépendant
                acts.H0.speed_a = speed;
                let _ = devs.write_h0(&acts.H0);
                *changed = true;
            }
        }
        "inb" => {
            let speed = output_val.max(0.0).min(100.0) as u8;
            info!("[EVENT SCHEDULER] Actionneur 'inb' (mode independant) -> application valeur: {} (Vitesse: {})", output_val, speed);
            if acts.H0.inverseur != 2 || acts.H0.speed_b != speed {
                acts.H0.inverseur = 2; // Forcer le mode indépendant
                acts.H0.speed_b = speed;
                let _ = devs.write_h0(&acts.H0);
                *changed = true;
            }
        }
        "H0" => {
            let speed_val = output_val.clamp(-100.0, 100.0) as i8;
            let (new_inv, new_speed_a, new_speed_b) = if speed_val < 0 {
                (-1, (-speed_val) as u8, 0u8)
            } else if speed_val > 0 {
                (1, 0u8, speed_val as u8)
            } else {
                (0, 0u8, 0u8)
            };
            info!("[EVENT SCHEDULER] Actionneur 'H0' (mode bipolaire) -> application valeur: {} (Mode: {}, Vitesse A: {}, Vitesse B: {})", output_val, new_inv, new_speed_a, new_speed_b);

            if acts.H0.inverseur != new_inv || acts.H0.speed_a != new_speed_a || acts.H0.speed_b != new_speed_b {
                acts.H0.inverseur = new_inv;
                acts.H0.speed_a = new_speed_a;
                acts.H0.speed_b = new_speed_b;
                let _ = devs.write_h0(&acts.H0);
                *changed = true;
            }
        }
        _ => {
            warn!("[EVENT SCHEDULER] Actionneur cible inconnu : {}", target);
        }
    }
}

fn parse_version(v: &str) -> (u32, u32, u32, u32) {
    let clean = v.trim().trim_start_matches('v');
    let (base, build_str) = if let Some(dash) = clean.find('-') {
        (&clean[..dash], &clean[dash + 1..])
    } else {
        (clean, "0")
    };
    let parts: Vec<&str> = base.split('.').collect();
    let major = parts.get(0).and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let patch = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    let build: u32 = build_str.parse().unwrap_or(0);
    (major, minor, patch, build)
}

/// Accept both a JSON array `[{...}, ...]` and a bare object `{...}` as a list of version entries.
fn version_entries(val: &serde_json::Value) -> Vec<&serde_json::Value> {
    if let Some(arr) = val.as_array() {
        arr.iter().collect()
    } else if val.is_object() {
        vec![val]
    } else {
        vec![]
    }
}

#[allow(dead_code)]
fn parse_url_host_port(url: &str) -> Option<(String, u16)> {
    let without_scheme = if let Some(stripped) = url.strip_prefix("http://") {
        (stripped, 80)
    } else if let Some(stripped) = url.strip_prefix("https://") {
        (stripped, 443)
    } else {
        (url, 80)
    };

    let host_part = without_scheme.0.split('/').next()?;
    if host_part.is_empty() {
        return None;
    }

    if let Some(colon_idx) = host_part.find(':') {
        let (host, port_str) = host_part.split_at(colon_idx);
        let port = port_str.strip_prefix(':')?.parse::<u16>().ok()?;
        Some((host.to_string(), port))
    } else {
        Some((host_part.to_string(), without_scheme.1))
    }
}

#[allow(dead_code)]
fn check_tcp_reachable(host: &str, port: u16) -> bool {
    use std::net::{TcpStream, ToSocketAddrs};
    let addr_str = format!("{}:{}", host, port);
    if let Ok(addrs) = addr_str.to_socket_addrs() {
        for addr in addrs {
            if let Ok(_stream) = TcpStream::connect_timeout(&addr, Duration::from_secs(3)) {
                return true;
            }
        }
    }
    false
}

#[allow(dead_code)]
fn check_dns_resolvable(host: &str) -> bool {
    use std::net::ToSocketAddrs;
    let addr_str = format!("{}:123", host);
    if let Ok(addrs) = addr_str.to_socket_addrs() {
        return addrs.count() > 0;
    }
    false
}

#[derive(Clone)]
pub struct CronHandle {
    sender: Sender<CronMessage>,
}

impl CronHandle {
    pub fn get_sensor_history(&self) -> Vec<MetricEntry> {
        let (tx, rx) = channel();
        if self.sender.send(CronMessage::GetHistory(tx)).is_ok() {
            rx.recv().unwrap_or_default()
        } else {
            Vec::new()
        }
    }

    #[allow(dead_code)]
    pub fn force_check_update(&self) {
        let _ = self.sender.send(CronMessage::ForceCheckUpdate);
    }
}

pub fn spawn_cron_scheduler(
    nvs: Arc<Mutex<NvsStorage>>,
    wifi: Arc<Mutex<NetManager>>,
    actuators_state: Arc<Mutex<crate::actuators::ActuatorsState>>,
    static_devs: Arc<Mutex<crate::actuators::Actuators>>,
    scheduled_actions: Arc<Mutex<crate::actuators::ScheduledActions>>,
    onewire_bus: Option<Arc<Mutex<crate::one_wire::OneWire<'static>>>>,
    i2c: Arc<Mutex<crate::i2c::I2c>>,
    board: Arc<Mutex<crate::board::Board>>,
) -> Result<CronHandle> {
    let (tx, rx) = channel();

    // 1. Spawn Worker Thread with a larger stack size (32KB) to prevent stack overflow
    let worker_nvs = Arc::clone(&nvs);
    let worker_wifi = Arc::clone(&wifi);
    let worker_act = Arc::clone(&actuators_state);
    let worker_devs = Arc::clone(&static_devs);
    let worker_sched = Arc::clone(&scheduled_actions);
    let worker_bus = onewire_bus.clone();
    let worker_i2c = Arc::clone(&i2c);
    let worker_board = Arc::clone(&board);
    thread::Builder::new()
        .name("cron_worker".to_string())
        .stack_size(65536)
        .spawn(move || {
            let worker = CronWorker::new(
                rx,
                worker_nvs,
                worker_wifi,
                worker_act,
                worker_devs,
                worker_sched,
                worker_bus,
                worker_i2c,
                worker_board,
            );
            worker.run();
        })
        .context("Failed to spawn cron worker thread")?;

    // 2. Spawn Tick generator thread (sends a Tick message every second)
    let tick_tx = tx.clone();
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_secs(1));
            if tick_tx.send(CronMessage::Tick).is_err() {
                break; // Receiver hung up, exit thread
            }
        }
    });

    Ok(CronHandle { sender: tx })
}
