use std::sync::{Arc, Mutex};
use std::sync::mpsc::{channel, Sender, Receiver};
use std::thread;
use std::time::{SystemTime, Duration};
use log::{info, warn};
use anyhow::Result;
use common::nvs_storage::NvsStorage;
use crate::sensors::{read_sensors, SensorReadings};

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
}

impl CronWorker {
    pub fn new(rx: Receiver<CronMessage>, nvs: Arc<Mutex<NvsStorage>>) -> Self {
        Self {
            rx,
            history: Vec::with_capacity(10),
            nvs,
        }
    }

    pub fn run(mut self) {
        info!("Starting Periodic Task Scheduler Worker Thread...");
        let mut sec_counter: u64 = 0;
        
        while let Ok(msg) = self.rx.recv() {
            match msg {
                CronMessage::Tick => {
                    sec_counter += 1;
                    
                    // Task 1: Collect sensor metrics every 30 seconds
                    if sec_counter % 30 == 0 {
                        self.collect_sensor_metrics();
                    }
                    
                    // Task 2: Trigger Simulated HTTP API report every 300 seconds
                    if sec_counter % 300 == 0 {
                        self.trigger_simulated_http_api();
                    }
                    
                    // Task 3: Check NVS target nextCheck timestamp to prevent drifts
                    if sec_counter % 60 == 0 { // Check NVS date target every 60 seconds
                        let _ = self.evaluate_7day_update_check(false);
                    }
                }
                CronMessage::ForceCheckUpdate => {
                    info!("Manual trigger: Forcing 7-day update check now...");
                    let _ = self.evaluate_7day_update_check(true);
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
        
        let readings = read_sensors();
        let entry = MetricEntry { timestamp: now, readings: readings.clone() };
        
        if self.history.len() >= 10 {
            self.history.remove(0);
        }
        self.history.push(entry);
        
        info!(
            "Task 30s: Collected sensor metrics. Temp SHT45: {:.1}°C, CO2: {} ppm. Sliding history size: {}", 
            readings.temperature_sht45, readings.co2_scd41, self.history.len()
        );
    }

    fn trigger_simulated_http_api(&self) {
        info!("Task 300s: Simulating HTTP Telemetry API report sending to cloud...");
        // Simulator placeholder
        thread::sleep(Duration::from_millis(200)); // Simulate networking delay
        info!("Telemetry HTTP POST successfully completed to https://api.whispereye.lan/v1/metrics [Payload: SHT45 Temp/Hum & CO2 SCD41]");
    }

    fn evaluate_7day_update_check(&self, force: bool) -> Result<()> {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // If system time is uninitialized or un-synchronized (mock NTP has not succeeded yet), skip check
        if now < 86400 * 365 {
            return Ok(());
        }

        let mut storage = self.nvs.lock().unwrap();
        let next_check_str = storage.get_str("nextCheck")?.unwrap_or_default();
        let mut next_check: u64 = next_check_str.parse().unwrap_or(0);

        if next_check == 0 {
            // First run: initialize target date to 7 days from now
            next_check = now + 7 * 86400;
            storage.set_str("nextCheck", &next_check.to_string())?;
            info!("NVS target 'nextCheck' initialized to: {} (7 days from now)", next_check);
            return Ok(());
        }

        if force || now >= next_check {
            info!("Task 7 Days: Running check_update() check (target nextCheck: {}, current: {})", next_check, now);
            self.perform_check_update(&mut *storage)?;
            
            // Set new target target target check date to exactly 7 days from now
            let new_next_check = now + 7 * 86400;
            storage.set_str("nextCheck", &new_next_check.to_string())?;
            info!("NVS target 'nextCheck' updated to: {} (Next 7-day target)", new_next_check);
        }

        Ok(())
    }

    fn perform_check_update(&self, storage: &mut NvsStorage) -> Result<()> {
        let url = storage.get_str("updateAvailable")?.unwrap_or_default();
        let fw = storage.get_str("fwVersion")?.unwrap_or_else(|| "v1.0.0-poc".to_string());
        
        if url.is_empty() {
            warn!("check_update skipped: no updateAvailable URL configured");
            return Ok(());
        }

        info!("Sending update request to catalogue URL: {}", url);
        
        let config = esp_idf_svc::http::client::Configuration {
            buffer_size: Some(2048),
            ..Default::default()
        };
        let mut connection = esp_idf_svc::http::client::EspHttpConnection::new(&config)?;
        connection.initiate_request(esp_idf_svc::http::Method::Get, &url, &[])?;
        connection.initiate_response()?;

        if connection.status() != 200 {
            warn!("Upstream catalog server returned HTTP status {}", connection.status());
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
                let b_type = entry.get("boardType").and_then(|v| v.as_str()).unwrap_or("");
                let c_type = entry.get("ChipType").and_then(|v| v.as_str()).unwrap_or("");
                if b_type == "v1.0" && c_type == "ESP32" {
                    if let Some(obj) = entry.as_object() {
                        for (key, val) in obj {
                            if key != "boardType" && key != "ChipType" && key != "peripheriques" {
                                if let Some(stable) = val.get("stable").and_then(|v| v.as_bool()) {
                                    if stable {
                                        if let Some(ver_str) = val.get("version").and_then(|v| v.as_str()) {
                                            if let Some(url_str) = val.get("url").and_then(|v| v.as_str()) {
                                                if parse_version(ver_str) > parse_version(&fw) {
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
            }
        }

        if let (Some(dl_url), Some(ver)) = (new_stable_url, new_version) {
            info!("Periodic update found version: {}. Arming OTA and rebooting to factory...", ver);
            storage.set_str("updateDlUrl", &dl_url)?;
            storage.set_i32("otaRetry", 3)?;
            
            thread::sleep(Duration::from_secs(2));
            unsafe {
                esp_idf_sys::esp_restart();
            }
        } else {
            info!("Periodic update check: firmware is up-to-date (Version: {})", fw);
        }

        Ok(())
    }
}

fn parse_version(v: &str) -> (u32, u32, u32) {
    let clean = v.trim().trim_start_matches('v');
    let parts: Vec<&str> = clean.split(|c| c == '.' || c == '-').collect();
    let major = parts.get(0).and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let patch = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    (major, minor, patch)
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

pub fn spawn_cron_scheduler(nvs: Arc<Mutex<NvsStorage>>) -> Result<CronHandle> {
    let (tx, rx) = channel();
    
    // 1. Spawn Worker Thread
    let worker_nvs = Arc::clone(&nvs);
    thread::spawn(move || {
        let worker = CronWorker::new(rx, worker_nvs);
        worker.run();
    });
    
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
