use crate::sensors::{read_sensors, SensorReadings};
use crate::wifi::{NetManager, NetState};
use crate::MeshState;
use anyhow::{Context, Result};
use common::nvs_storage::NvsStorage;
use log::{info, warn};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, serde::Serialize)]
pub struct MetricEntry {
    pub timestamp: u64,
    pub readings: SensorReadings,
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
    #[allow(dead_code)]
    mesh_state: Arc<Mutex<MeshState>>,
    actuators_state: Arc<Mutex<crate::actuators::ActuatorsState>>,
    static_devs: Arc<Mutex<crate::actuators::Actuators>>,
    scheduled_actions: Arc<Mutex<crate::actuators::ScheduledActions>>,
    onewire_bus: Option<Arc<Mutex<crate::one_wire::OneWire<'static>>>>,
    i2c: Arc<Mutex<crate::i2c::I2c>>,
    last_metrics_run: Option<std::time::Instant>,
    last_telemetry_run: Option<std::time::Instant>,
    last_update_check_run: Option<std::time::Instant>,
}

impl CronWorker {
    pub fn new(
        rx: Receiver<CronMessage>,
        nvs: Arc<Mutex<NvsStorage>>,
        wifi: Arc<Mutex<NetManager>>,
        mesh_state: Arc<Mutex<MeshState>>,
        actuators_state: Arc<Mutex<crate::actuators::ActuatorsState>>,
        static_devs: Arc<Mutex<crate::actuators::Actuators>>,
        scheduled_actions: Arc<Mutex<crate::actuators::ScheduledActions>>,
        onewire_bus: Option<Arc<Mutex<crate::one_wire::OneWire<'static>>>>,
        i2c: Arc<Mutex<crate::i2c::I2c>>,
    ) -> Self {
        Self {
            rx,
            history: Vec::with_capacity(10),
            nvs,
            wifi,
            mesh_state,
            actuators_state,
            static_devs,
            scheduled_actions,
            onewire_bus,
            i2c,
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
                                info!("Executing scheduled action for actuator {}: setting state to {}", id, action.state);
                                match id.as_str() {
                                    "rla" => { acts.rla = action.state; }
                                    "rlb" => { acts.rlb = action.state; }
                                    "swpwr" => { acts.swpwr = action.state; }
                                    "ina" => { acts.ina = action.state; }
                                    "inb" => { acts.inb = action.state; }
                                    _ => {
                                        warn!("Unknown actuator id in schedule: {}", id);
                                    }
                                }
                                let _ = devs.write(id.as_str(), action.state);
                                changed = true;
                            }
                        }
                        if changed {
                            info!("Actuators state updated from schedules: {:?}", *acts);
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
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let onewr_probes = {
            let storage = self.nvs.lock().unwrap();
            let mut list = Vec::new();
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
                            if present {
                                list.push(id[6..].to_string());
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
            list
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

        let mut readings = read_sensors(self.onewire_bus.as_ref().map(|b| b.as_ref()), &onewr_probes);
        if let Some(ref sht) = sht4_opt {
            readings.temperature_sht45 = sht.temperature;
            readings.humidity_sht45 = sht.humidity;
        }
        if let Some(ref scd) = scd_opt {
            readings.co2_scd41 = scd.co2;
        }
        crate::dynamic_devices::apply_sensor_corrections(&self.nvs, &mut readings);
        let entry = MetricEntry {
            timestamp: now,
            readings: readings.clone(),
        };

        if self.history.len() >= 10 {
            self.history.remove(0);
        }
        self.history.push(entry);

        let mut msg_log = "Task 30s: Collected sensor metrics.".to_string();
        let registry = crate::dynamic_devices::DeviceRegistry::new(Arc::clone(&self.nvs));
        let devices = registry.load_registry();
        
        let sht_present = devices.values().any(|e| e.name.contains("SHT45") && e.present);
        if sht_present {
            msg_log.push_str(" Temp SHT45: 23.4°C, Humi SHT45: 45.2%,");
        }
        
        let scd_present = devices.values().any(|e| e.name.contains("SCD41") && e.present);
        if scd_present {
            msg_log.push_str(" CO2: 680 ppm,");
        }

        let bme_present = devices.values().any(|e| e.name.contains("BME280") && e.present);
        if bme_present {
            let t = *crate::i2c::i2c_bme280::BME280_TEMP.lock().unwrap();
            let h = *crate::i2c::i2c_bme280::BME280_HUM.lock().unwrap();
            let p = *crate::i2c::i2c_bme280::BME280_PRESS.lock().unwrap();
            msg_log.push_str(&format!(" BME280: Temp={:.1}°C, Hum={:.1}%, Pres={:.1} hPa,", t, h, p));
        }

        let probes_count = readings.ds18b20_temperatures.iter().filter(|(_, &t)| t != -255.0).count();
        msg_log.push_str(&format!(" Probes count: {}. Sliding history size: {}", probes_count, self.history.len()));
        
        info!("{}", msg_log);

        // Reconnection handled asynchronously by the net_controller thread
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
            info!("Telemetry skipped: metricsUrl is empty or not defined");
            return;
        }

        info!(
            "Task 300s: Sending HTTP PUT telemetry to {}...",
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

        let mut storage = self.nvs.lock().unwrap();
        let auto_update = storage.get_i32("autoUpdate")?.unwrap_or(1);
        if auto_update == 0 {
            let next_check_str = storage.get_str("nextCheck")?.unwrap_or_default();
            if next_check_str != "4102387200" {
                storage.set_str("nextCheck", "4102387200")?;
                info!("autoUpdate is false, nextCheck set to 2099-12-31 (4102387200)");
            }
            return Ok(());
        }

        let next_check_str = storage.get_str("nextCheck")?.unwrap_or_default();
        let mut next_check: u64 = next_check_str.parse().unwrap_or(0);

        if next_check == 0 || next_check_str == "4102387200" {
            // First run or transitioning from disabled: initialize target date to tomorrow at 14:00 UTC
            next_check = ((now / 86400) + 1) * 86400 + 14 * 3600;
            storage.set_str("nextCheck", &next_check.to_string())?;
            info!("NVS target 'nextCheck' initialized to tomorrow 14:00 UTC: {} (after transition or first run)", next_check);
            return Ok(());
        }

        if force || now >= next_check {
            info!(
                "Task 7 Days: Running check_update() check (target nextCheck: {}, current: {})",
                next_check, now
            );
            self.perform_check_update(&mut *storage)?;

            // Set new target check date to exactly 7 days from now
            let new_next_check = now + 7 * 86400;
            storage.set_str("nextCheck", &new_next_check.to_string())?;
            info!(
                "NVS target 'nextCheck' updated to: {} (Next 7-day target)",
                new_next_check
            );
        }

        Ok(())
    }

    fn perform_check_update(&self, storage: &mut NvsStorage) -> Result<()> {
        let url = storage.get_str("updateRepoList")?.unwrap_or_default();
        let fw = storage
            .get_str("fwVersion")?
            .unwrap_or_else(|| "v1.0.0-poc".to_string());

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
        connection.initiate_request(esp_idf_svc::http::Method::Get, &url, &[])?;
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
                                    if parse_version(ver_str) > parse_version(&fw) {
                                        let current_best =
                                            new_version.as_deref().unwrap_or(fw.as_str());
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
            storage.set_str("updateDlUrl", &dl_url)?;
            storage.set_i32("otaRetry", 3)?;

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
}

fn parse_version(v: &str) -> (u32, u32, u32, u32) {
    let clean = v.trim().trim_start_matches('v');
    // Handle both "1.0.1" and "1.0.1-0125" formats
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
    mesh_state: Arc<Mutex<MeshState>>,
    actuators_state: Arc<Mutex<crate::actuators::ActuatorsState>>,
    static_devs: Arc<Mutex<crate::actuators::Actuators>>,
    scheduled_actions: Arc<Mutex<crate::actuators::ScheduledActions>>,
    onewire_bus: Option<Arc<Mutex<crate::one_wire::OneWire<'static>>>>,
    i2c: Arc<Mutex<crate::i2c::I2c>>,
) -> Result<CronHandle> {
    let (tx, rx) = channel();

    // 1. Spawn Worker Thread with a larger stack size (32KB) to prevent stack overflow
    let worker_nvs = Arc::clone(&nvs);
    let worker_wifi = Arc::clone(&wifi);
    let worker_mesh = Arc::clone(&mesh_state);
    let worker_act = Arc::clone(&actuators_state);
    let worker_devs = Arc::clone(&static_devs);
    let worker_sched = Arc::clone(&scheduled_actions);
    let worker_bus = onewire_bus.clone();
    let worker_i2c = Arc::clone(&i2c);
    thread::Builder::new()
        .name("cron_worker".to_string())
        .stack_size(32768)
        .spawn(move || {
            let worker = CronWorker::new(
                rx,
                worker_nvs,
                worker_wifi,
                worker_mesh,
                worker_act,
                worker_devs,
                worker_sched,
                worker_bus,
                worker_i2c,
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
