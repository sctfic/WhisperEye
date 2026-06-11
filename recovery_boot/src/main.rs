use esp_idf_sys as _; // Mandatory for linking ESP-IDF
use esp_idf_hal::peripherals::Peripherals;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::http::server::{EspHttpServer, Configuration as ServerConfig};
use esp_idf_svc::sntp::EspSntp;
// use esp_idf_svc::systime::EspSystemTime;
use esp_idf_svc::ota::EspOta;
use anyhow::{Result, Context, anyhow};
use log::{info, error, warn};
use std::time::SystemTime;
use std::thread;
use std::sync::{Arc, Mutex};


mod wifi;
mod ota;
mod web_pages;

use wifi::WifiManager;
use common::nvs_storage::NvsStorage;

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct ConfigPayload {
    wifi_ssid: Option<String>,
    wifi_psk: Option<String>,
    update_url: Option<String>,
    update_interval: Option<String>,
    apply_only: Option<bool>,
    auto_update: Option<bool>,
}

const WHISPEREYE_BOARD: &str = "1.0";
const CHIP_TYPE: &str = "ESP32-S3";
const FW_VERSION: &str = "1.0.0-recovery-0033";

const AUTHOR_EMAIL: &str = "alban.lopez+whisperEye@gmail.com";
const AUTHOR_NAME: &str = "LOPEZ Alban";
const AUTHOR_LINK: &str = "https://github.com/sctfic/WhisperEye/blob/main/README.md";

struct CustomLogger;

impl log::Log for CustomLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            let marker = match record.level() {
                log::Level::Error => "E",
                log::Level::Warn => "W",
                log::Level::Info => "I",
                log::Level::Debug => "D",
                log::Level::Trace => "V",
            };
            let timestamp = unsafe { esp_idf_sys::esp_log_timestamp() };
            // Recovery: first letter in Orange (\x1b[38;5;208m)
            println!("\x1b[38;5;208m{}\x1b[0m ({}) {}: {}", marker, timestamp, record.target(), record.args());
        }
    }

    fn flush(&self) {}
}

static LOGGER: CustomLogger = CustomLogger;

fn main() -> Result<()> {
    log::set_logger(&LOGGER)
        .map(|()| log::set_max_level(log::LevelFilter::Info))
        .expect("Failed to initialize custom logger");
    info!("\x1b[38;5;208mWhisperEye Recovery Boot Firmware Starting Up...\x1b[0m");

    let peripherals = Peripherals::take().context("Failed to take ESP32 Peripherals")?;
    let sys_loop = EspSystemEventLoop::take().context("Failed to take System Event Loop")?;
    let nvs_default = EspDefaultNvsPartition::take().context("Failed to take NVS Partition")?;

    // Initialize NVS Storage helper
    let nvs_storage = Arc::new(Mutex::new(NvsStorage::new(nvs_default.clone())?));
    
    // Dump NVS variables to logs
    {
        let storage = nvs_storage.lock().unwrap();
        let _ = storage.dump_to_log();
    }
    
    // Initialize Wi-Fi
    let mut wifi_manager = WifiManager::new(peripherals, sys_loop.clone(), nvs_default)?;
    
    // Perform initial scan before any connection attempts
    let _ = wifi_manager.perform_initial_scan();
    
    let mut connected = false;
    let mut chosen_ssid = String::new();

    let known_networks = {
        let storage = nvs_storage.lock().unwrap();
        storage.get_known_networks().unwrap_or_default()
    };

    if !known_networks.is_empty() {
        // Find default network first
        let default_net = known_networks.iter()
            .find(|(_, entry)| entry.default.unwrap_or(false))
            .map(|(ssid, entry)| (ssid.clone(), entry.psk.clone()));

        if let Some((ref ssid, ref psk)) = default_net {
            info!("Trying default network: {}", ssid);
            if wifi_manager.start_sta(ssid, psk).unwrap_or(false) {
                connected = true;
                chosen_ssid = ssid.clone();
            }
        }

        if !connected {
            info!("Default network failed or not set. Trying other known networks...");
            for (ssid, entry) in &known_networks {
                if let Some((ref def_ssid, _)) = default_net {
                    if ssid == def_ssid { continue; }
                }
                info!("Trying known network: {}", ssid);
                if wifi_manager.start_sta(ssid, &entry.psk).unwrap_or(false) {
                    connected = true;
                    chosen_ssid = ssid.clone();
                    break;
                }
            }
        }

        if connected {
            if let Ok(mut storage) = nvs_storage.lock() {
                let _ = storage.set_default_network_by_ssid(&chosen_ssid);
            }
        }
    } else {
        info!("No known networks. Staying in AP mode.");
    }
    
    if !connected {
        warn!("All STA Connections failed or no known networks. Falling back to Access Point captive mode...");
        wifi_manager.start_ap()?;
    }

    let wifi_manager = Arc::new(Mutex::new(wifi_manager));

    // Initialize SNTP if connected to STA
    let _sntp = if connected {
        info!("Initializing SNTP default pool...");
        let sntp = EspSntp::new_default();
        if sntp.is_err() {
            warn!("Failed to initialize SNTP service");
        }
        sntp.ok()
    } else {
        None
    };

    // Spawn automatic OTA update thread if connected in STA mode
    if connected {
        let nvs_clone = Arc::clone(&nvs_storage);
        let _ = thread::Builder::new()
            .name("auto_ota_worker".to_string())
            .stack_size(32768)
            .spawn(move || {
                // Wait a few seconds for NTP/Time sync and stable networking
                thread::sleep(std::time::Duration::from_secs(5));
            
            let mut retry_data = None;
            {
                let mut storage = nvs_clone.lock().unwrap();
                if let Ok(Some(retry)) = storage.get_i32("otaRetry") {
                    if retry > 0 {
                        if let Ok(Some(url)) = storage.get_str("updateDlUrl") {
                            if !url.is_empty() {
                                // Decrement otaRetry immediately to prevent infinite bootloop on crash!
                                let new_retry = retry - 1;
                                let _ = storage.set_i32("otaRetry", new_retry);
                                retry_data = Some((new_retry + 1, url));
                            }
                        }
                    }
                }
            }

            if let Some((retries_left, url)) = retry_data {
                info!("Automatic boot update scheduled. Retries left: {}. Fetching URL: {}", retries_left, url);
                match ota::perform_ota(&url) {
                    Ok(_) => {
                        // Update NVS keys
                        if let Ok(mut storage) = nvs_clone.lock() {
                            let now_str = get_formatted_time();
                            let _ = storage.set_str("lastOtaDl", &now_str);
                            let _ = storage.set_str("lastOtaWrite", &now_str);
                            let _ = storage.set_str("fwVersion", "empty");
                        }
                        info!("\x1b[35;1m[ÉTAPE 6] Reboot de l'appareil\x1b[0m");
                        info!("\x1b[36;1m  -> Firmware actif lors du reboot : recovery_boot ({})\x1b[0m", FW_VERSION);
                        info!("\x1b[36;1m  -> Clés NVS configurées pour forcer la validation (fwVersion = empty)\x1b[0m");
                        info!("\x1b[36;1m  -> Démarrage du firmware de production dans 2 secondes...\x1b[0m");
                        thread::sleep(std::time::Duration::from_secs(2));
                        unsafe {
                            esp_idf_sys::esp_restart();
                        }
                    }
                    Err(e) => {
                        error!("Automatic OTA failed: {:?}", e);
                    }
                }
            }
        });
    }

    // Start HTTP Web Server with wildcard URI matching enabled
    let mut server_config = ServerConfig::default();
    server_config.uri_match_wildcard = true;
    let mut server = EspHttpServer::new(&server_config)
        .context("Failed to start HTTP server")?;

    // GET / (Main HTML Dashboard)
    server.fn_handler("/", esp_idf_svc::http::Method::Get, |req| -> Result<(), anyhow::Error> {
        let mut response = req.into_ok_response()?;
        response.write(web_pages::RECOVERY_HTML.as_bytes())?;
        Ok(())
    })?;

    // Captive Portal HTTP Redirects for Mobile Auto-Popup (iOS, Android, Windows)
    server.fn_handler("/generate_204", esp_idf_svc::http::Method::Get, |req| -> Result<(), anyhow::Error> {
        let mut response = req.into_response(302, Some("Found"), &[("Location", "http://192.168.71.1/")])?;
        response.write(b"Redirecting to captive portal...")?;
        Ok(())
    })?;

    server.fn_handler("/hotspot-detect.html", esp_idf_svc::http::Method::Get, |req| -> Result<(), anyhow::Error> {
        let mut response = req.into_response(302, Some("Found"), &[("Location", "http://192.168.71.1/")])?;
        response.write(b"Redirecting to captive portal...")?;
        Ok(())
    })?;

    server.fn_handler("/ncsi.txt", esp_idf_svc::http::Method::Get, |req| -> Result<(), anyhow::Error> {
        let mut response = req.into_response(302, Some("Found"), &[("Location", "http://192.168.71.1/")])?;
        response.write(b"Redirecting to captive portal...")?;
        Ok(())
    })?;

    server.fn_handler("/connecttest.txt", esp_idf_svc::http::Method::Get, |req| -> Result<(), anyhow::Error> {
        let mut response = req.into_response(302, Some("Found"), &[("Location", "http://192.168.71.1/")])?;
        response.write(b"Redirecting to captive portal...")?;
        Ok(())
    })?;

    // GET /api/status
    let nvs_clone = Arc::clone(&nvs_storage);
    let wifi_clone = Arc::clone(&wifi_manager);
    server.fn_handler("/api/status", esp_idf_svc::http::Method::Get, move |req| -> Result<(), anyhow::Error> {
        let storage = nvs_clone.lock().unwrap();
        let wifi = wifi_clone.lock().unwrap();
        
        let (active_mode, wifi_ssid) = match wifi.wifi.get_configuration() {
            Ok(esp_idf_svc::wifi::Configuration::Client(client_cfg)) => {
                ("Station", client_cfg.ssid.as_str().to_string())
            }
            Ok(esp_idf_svc::wifi::Configuration::AccessPoint(ap_cfg)) => {
                ("AccessPoint", ap_cfg.ssid.as_str().to_string())
            }
            Ok(esp_idf_svc::wifi::Configuration::Mixed(client_cfg, _)) => {
                ("Mixed", client_cfg.ssid.as_str().to_string())
            }
            _ => {
                ("None", "".to_string())
            }
        };

        let ip_info = if active_mode == "Station" {
            wifi.wifi.wifi().sta_netif().get_ip_info().ok()
        } else {
            wifi.wifi.wifi().ap_netif().get_ip_info().ok()
        };

        let ip_addr = ip_info.map(|i| i.ip.to_string()).unwrap_or_else(|| "0.0.0.0".to_string());
        let gateway = ip_info.map(|i| i.subnet.gateway.to_string()).unwrap_or_else(|| "0.0.0.0".to_string());
        let rssi = if active_mode == "Station" {
            wifi.wifi.wifi().get_ap_info().ok().map(|i| i.signal_strength)
        } else {
            None
        };

        let now_str = get_formatted_time();

        let ntp_server = storage.get_str("ntpServer")?.unwrap_or_default();
        let fw_version = storage.get_str("fwVersion")?.unwrap_or_default();
        let last_ota_success = storage.get_str("lastOtaSuccess")?.unwrap_or_default();
        let last_ota_dl = storage.get_str("lastOtaDl")?.unwrap_or_default();
        let last_ota_write = storage.get_str("lastOtaWrite")?.unwrap_or_default();
        let update_url = storage.get_str("updateAvailable")?.unwrap_or_default();
        let update_interval = storage.get_str("updateInterval")?.unwrap_or_else(|| "7j".to_string());
        let wifi_known = storage.get_known_networks().unwrap_or_default();
        let auto_update = storage.get_i32("autoUpdate")?.unwrap_or(1) == 1;

        let json = serde_json::json!({
            "network_mode": active_mode,
            "wifi_ssid": wifi_ssid,
            "wifi_rssi": rssi,
            "ip_addr": ip_addr,
            "gateway_addr": gateway,
            "sys_time": now_str,
            "ntp_server": ntp_server,
            "fw_version": fw_version,
            "last_ota_success": last_ota_success,
            "last_ota_dl": last_ota_dl,
            "last_ota_write": last_ota_write,
            "update_url": update_url,
            "update_interval": update_interval,
            "board_type": WHISPEREYE_BOARD,
            "chip_type": CHIP_TYPE,
            "recovery_version": FW_VERSION,
            "wifi_known": wifi_known,
            "auto_update": auto_update,
            "author": {
                "email": AUTHOR_EMAIL,
                "name": AUTHOR_NAME,
                "link": AUTHOR_LINK
            }
        });

        let response_data = serde_json::to_string(&json)?;
        let mut response = req.into_ok_response()?;
        response.write(response_data.as_bytes())?;
        Ok(())
    })?;

    // GET /api/updateStatus
    server.fn_handler("/api/updateStatus", esp_idf_svc::http::Method::Get, move |req| -> Result<(), anyhow::Error> {
        let (pct, size, written, msg) = {
            let status = crate::ota::UPDATE_STATUS.lock().unwrap();
            (status.percentage, status.size, status.written, status.status)
        };
        let json = serde_json::json!({
            "percentage": pct,
            "size": size,
            "written": written,
            "status": msg
        });
        let mut response = req.into_ok_response()?;
        response.write(json.to_string().as_bytes())?;
        Ok(())
    })?;

    // GET /api/check_updates (proxies firmware.json from updateAvailable NVS key to bypass CORS!)
    let nvs_updates_clone = Arc::clone(&nvs_storage);
    server.fn_handler("/api/check_updates", esp_idf_svc::http::Method::Get, move |req| -> Result<(), anyhow::Error> {
        let update_url = {
            let storage = nvs_updates_clone.lock().unwrap();
            storage.get_str("updateAvailable")?.unwrap_or_default()
        };

        info!("\x1b[35;1m[ÉTAPE 1] Vérification des mises à jour à l'URL : {}\x1b[0m", update_url);

        if update_url.is_empty() {
            warn!("Aucune URL de mise à jour configurée dans la NVS.");
            let mut response = req.into_status_response(400)?;
            response.write(b"No update URL configured")?;
            return Ok(());
        }

        info!("Lancement de la requête HTTP GET vers l'URL amont...");
        // Fetch JSON from update_url on ESP32 side to bypass CORS!
        let config = esp_idf_svc::http::client::Configuration {
            buffer_size: Some(2048),
            crt_bundle_attach: Some(esp_idf_sys::esp_crt_bundle_attach),
            ..Default::default()
        };
        let mut connection = esp_idf_svc::http::client::EspHttpConnection::new(&config)?;
        connection.initiate_request(esp_idf_svc::http::Method::Get, &update_url, &[])?;
        connection.initiate_response()?;

        let status = connection.status();
        info!("Réponse reçue de l'URL amont. Statut HTTP : {}", status);
        if status != 200 {
            let mut response = req.into_status_response(502)?;
            response.write(format!("Upstream error: HTTP {}", status).as_bytes())?;
            return Ok(());
        }

        let mut body = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            match connection.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => body.extend_from_slice(&chunk[..n]),
                Err(e) => {
                    let mut response = req.into_status_response(500)?;
                    response.write(format!("Read error: {:?}", e).as_bytes())?;
                    return Ok(());
                }
            }
        }

        if let Ok(body_str) = std::str::from_utf8(&body) {
            info!("Contenu brut de la réponse amont :\n{}", body_str);
        } else {
            info!("Contenu brut de la réponse amont : [Binaire ou UTF-8 invalide]");
        }

        let list: serde_json::Value = serde_json::from_slice(&body)?;
        let mut matched_entry = serde_json::Value::Null;

        let entries = if let Some(arr) = list.as_array() {
            arr.clone()
        } else if list.is_object() {
            vec![list.clone()]
        } else {
            vec![]
        };

        for entry in entries {
            let b_type = entry.get("boardType").and_then(|v| v.as_str()).unwrap_or("");
            let c_type = entry.get("ChipType").and_then(|v| v.as_str()).unwrap_or("");
            if b_type == "v2.0" && c_type == "ESP32-S3" {
                matched_entry = entry.clone();
                break;
            }
        }

        let response_data = serde_json::to_string(&matched_entry)?;
        let mut response = req.into_response(200, Some("OK"), &[
            ("Content-Type", "application/json"),
            ("Access-Control-Allow-Origin", "*")
        ])?;
        response.write(response_data.as_bytes())?;
        Ok(())
    })?;

    // GET /api/ssids (Active hardware Wi-Fi scan)
    let wifi_scan_clone = Arc::clone(&wifi_manager);
    let nvs_ssids_clone = Arc::clone(&nvs_storage);
    server.fn_handler("/api/ssids", esp_idf_svc::http::Method::Get, move |req| -> Result<(), anyhow::Error> {
        let mut wifi = wifi_scan_clone.lock().unwrap();
        let ssids = match wifi.wifi.scan() {
            Ok(ap_list) => {
                let mut list: Vec<String> = ap_list.into_iter()
                    .map(|ap| ap.ssid.to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                list.sort();
                list.dedup();
                wifi.scan_cache = list.clone();
                list
            }
            Err(_) => {
                // In AP mode, active scan fails (ESP_FAIL). Return the boot-time cache quietly.
                let mut fallback = wifi.scan_cache.clone();
                if fallback.is_empty() {
                    fallback = vec!["IoT".to_string(), "Maison_WiFi".to_string(), "WhisperEye-Mesh".to_string(), "Freebox-Private".to_string()];
                }
                fallback
            }
        };
        let wifi_ssid = {
            let storage = nvs_ssids_clone.lock().unwrap();
            let known = storage.get_known_networks().unwrap_or_default();
            known.iter()
                .find(|(_, entry)| entry.default.unwrap_or(false))
                .map(|(ssid, _)| ssid.clone())
                .unwrap_or_default()
        };
        let response_json = serde_json::json!({
            "ssids": ssids,
            "active": wifi_ssid
        });
        let response_data = serde_json::to_string(&response_json)?;
        let mut response = req.into_ok_response()?;
        response.write(response_data.as_bytes())?;
        Ok(())
    })?;

    // GET /* Catch-all wildcard redirect to captive portal for any other GET requests
    server.fn_handler("/*", esp_idf_svc::http::Method::Get, |req| -> Result<(), anyhow::Error> {
        let mut response = req.into_response(302, Some("Found"), &[("Location", "http://192.168.71.1/")])?;
        response.write(b"Redirecting to captive portal...")?;
        Ok(())
    })?;

    // POST /api/config
    let nvs_clone = Arc::clone(&nvs_storage);
    let wifi_clone = Arc::clone(&wifi_manager);
    server.fn_handler("/api/config", esp_idf_svc::http::Method::Post, move |mut req| -> Result<(), anyhow::Error> {
        let mut buf = vec![0u8; 512];
        let bytes_read = req.read(&mut buf)?;
        let payload: ConfigPayload = serde_json::from_slice(&buf[..bytes_read])?;
        
        let mut wifi_success = true;

        if let Some(ref ssid_raw) = payload.wifi_ssid {
            let ssid = ssid_raw.trim();
            if !ssid.is_empty() {
                wifi_success = false;
                let psk = payload.wifi_psk.as_deref().unwrap_or("").trim();
                let mut final_psk = "".to_string();
                let mut wifi = wifi_clone.lock().unwrap();
                let mut storage = nvs_clone.lock().unwrap();
                
                if psk.is_empty() {
                    // Check if it is in known networks
                    let known_networks = storage.get_known_networks().unwrap_or_default();
                    if let Some(entry) = known_networks.get(ssid) {
                        info!("SSID '{}' is known. Testing connection with saved key.", ssid);
                        final_psk = entry.psk.clone();
                        if wifi.start_sta(ssid, &final_psk).unwrap_or(false) {
                            wifi_success = true;
                        }
                    }
                    if !wifi_success {
                        info!("Saved key failed or not found. Testing connection to SSID '{}' without key.", ssid);
                        final_psk = "".to_string();
                        if wifi.start_sta(ssid, "").unwrap_or(false) {
                            wifi_success = true;
                        }
                    }
                } else {
                    // PSK is provided
                    info!("Testing connection to SSID '{}' with provided key.", ssid);
                    final_psk = psk.to_string();
                    if wifi.start_sta(ssid, &final_psk).unwrap_or(false) {
                        wifi_success = true;
                    } else {
                        info!("Provided key failed. Testing connection to SSID '{}' without key.", ssid);
                        final_psk = "".to_string();
                        if wifi.start_sta(ssid, "").unwrap_or(false) {
                            wifi_success = true;
                        }
                    }
                }
                
                if wifi_success {
                    info!("Connection successful to SSID '{}'. Saving to NVS...", ssid);
                    storage.set_default_network(ssid, &final_psk)?;
                } else {
                    warn!("Wi-Fi connection to '{}' failed. Trying known networks from NVS...", ssid);
                    let known_networks = storage.get_known_networks().unwrap_or_default();
                    let mut reconnected = false;
                    let mut chosen_ssid = String::new();

                    // First try default network if any
                    let default_net = known_networks.iter()
                        .find(|(_, entry)| entry.default.unwrap_or(false))
                        .map(|(ssid, entry)| (ssid.clone(), entry.psk.clone()));

                    if let Some((ref d_ssid, ref d_psk)) = default_net {
                        if d_ssid != ssid {
                            info!("Trying default network: {}", d_ssid);
                            if wifi.start_sta(d_ssid, d_psk).unwrap_or(false) {
                                reconnected = true;
                                chosen_ssid = d_ssid.clone();
                            }
                        }
                    }

                    if !reconnected {
                        for (known_ssid, entry) in &known_networks {
                            if known_ssid == ssid { continue; }
                            if let Some((ref d_ssid, _)) = default_net {
                                if known_ssid == d_ssid { continue; }
                            }
                            info!("Trying known network: {}", known_ssid);
                            if wifi.start_sta(known_ssid, &entry.psk).unwrap_or(false) {
                                reconnected = true;
                                chosen_ssid = known_ssid.clone();
                                break;
                            }
                        }
                    }

                    if !reconnected {
                        warn!("All known networks failed. Restarting Access Point...");
                        let _ = wifi.start_ap();
                    } else {
                        info!("Reconnected to known network '{}'.", chosen_ssid);
                        let _ = storage.set_default_network_by_ssid(&chosen_ssid);
                    }
                }
            }
        }
        
        if !wifi_success {
            let mut response = req.into_status_response(400)?;
            response.write(b"WiFi Connection Failed")?;
            return Ok(());
        }

        let mut has_update_url = false;
        if let Some(ref update_url) = payload.update_url {
            if !update_url.is_empty() {
                has_update_url = true;
                let is_bin = update_url.ends_with(".bin");
                let mut storage = nvs_clone.lock().unwrap();
                if is_bin {
                    storage.set_str("updateDlUrl", update_url)?;
                    storage.set_i32("otaRetry", 3)?;
                } else {
                    storage.set_str("updateAvailable", update_url)?;
                }
            }
        }
        
        if let Some(auto_up) = payload.auto_update {
            let mut storage = nvs_clone.lock().unwrap();
            let current_val = storage.get_i32("autoUpdate")?.unwrap_or(1);
            let new_val = if auto_up { 1 } else { 0 };
            storage.set_i32("autoUpdate", new_val)?;
            if new_val == 0 {
                storage.set_str("nextCheck", "4102387200")?;
            } else if current_val == 0 && new_val == 1 {
                let now = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let next_check = if now > 86400 * 365 {
                    ((now / 86400) + 1) * 86400 + 14 * 3600
                } else {
                    0
                };
                storage.set_str("nextCheck", &next_check.to_string())?;
                info!("autoUpdate transitioned from 0 to 1, nextCheck reset to tomorrow 14:00 UTC ({})", next_check);
            }
        }

        let apply_only = payload.apply_only.unwrap_or(false);
        if !apply_only && has_update_url {
            info!("New configuration saved to NVS. Starting flash OTA in background...");
            
            // Spawn OTA in a detached thread with a robust stack size (32KB), wait 1s before starting to send HTTP 200 ok first
            let nvs_thread = Arc::clone(&nvs_clone);
            let _ = thread::Builder::new()
                .name("manual_ota_worker".to_string())
                .stack_size(32768)
                .spawn(move || {
                    thread::sleep(std::time::Duration::from_millis(1000));
                    let update_bin_url = {
                        let storage = nvs_thread.lock().unwrap();
                        storage.get_str("updateDlUrl").unwrap_or(None).unwrap_or_default()
                    };
                    if !update_bin_url.is_empty() {
                        match ota::perform_ota(&update_bin_url) {
                            Ok(_) => {
                                 if let Ok(mut storage) = nvs_thread.lock() {
                                     let now_str = get_formatted_time();
                                     let _ = storage.set_str("lastOtaDl", &now_str);
                                     let _ = storage.set_str("lastOtaWrite", &now_str);
                                     let _ = storage.set_str("fwVersion", "empty");
                                     info!("\x1b[35;1m[ÉTAPE 6] Reboot de l'appareil\x1b[0m");
                                     info!("\x1b[36;1m  -> Firmware actif lors du reboot : recovery_boot ({})\x1b[0m", FW_VERSION);
                                     info!("\x1b[36;1m  -> Clés NVS configurées pour forcer la validation (fwVersion = empty)\x1b[0m");
                                     info!("\x1b[36;1m  -> Redémarrage en cours (esp_restart)...\x1b[0m");
                                     thread::sleep(std::time::Duration::from_secs(1));
                                     unsafe {
                                         esp_idf_sys::esp_restart();
                                     }
                                 }
                            }
                            Err(e) => {
                                error!("OTA failed after config update: {:?}", e);
                            }
                        }
                    } else {
                        error!("No updateDlUrl configured for manual OTA triggering!");
                    }
                });
        } else {
            info!("Configuration saved to NVS. No OTA run in progress.");
        }

        let mut response = req.into_ok_response()?;
        response.write(b"OK")?;
        Ok(())
    })?;

    // POST /api/upload-ota (Direct HTTP partition flashing)
    server.fn_handler("/api/upload-ota", esp_idf_svc::http::Method::Post, |mut req| -> Result<(), anyhow::Error> {
        info!("\x1b[35;1m[ÉTAPE 4] Téléchargement du binaire bloc par bloc\x1b[0m");
        info!("\x1b[36;1m  -> Réception et flashage direct du binaire uploadé par morceaux...\x1b[0m");
        
        {
            let mut status = crate::ota::UPDATE_STATUS.lock().unwrap();
            status.percentage = 0;
            status.size = 0;
            status.written = 0;
            status.status = "Téléchargement et flashage de l'upload direct...";
        }

        let content_len = req.header("Content-Length")
            .and_then(|h| h.parse::<usize>().ok())
            .unwrap_or(0);
        {
            let mut status = crate::ota::UPDATE_STATUS.lock().unwrap();
            status.size = content_len;
        }

        let mut ota = EspOta::new().context("Failed to init ESP OTA")?;
        let mut ota_write = ota.initiate_update().context("Failed to initiate OTA update")?;
        
        let mut buf = [0u8; 2048]; // 2KB Buffer size constraint
        let mut total_read = 0;
        
        loop {
            match req.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    ota_write.write(&buf[..n])?;
                    total_read += n;
                    let progress = if content_len > 0 {
                        ((total_read as f32 / content_len as f32) * 100.0) as u8
                    } else {
                        0
                    };
                    {
                        let mut status = crate::ota::UPDATE_STATUS.lock().unwrap();
                        status.percentage = progress;
                        status.written = total_read;
                    }
                    if content_len > 0 {
                        print!("\r\x1b[36;1m  -> Progression : {}% ({} / {} octets)\x1b[0m", progress, total_read, content_len);
                    } else {
                        print!("\r\x1b[36;1m  -> Progression : {} octets\x1b[0m", total_read);
                    }
                    use std::io::Write;
                    let _ = std::io::stdout().flush();
                }
                Err(e) => {
                    if let Ok(mut status) = crate::ota::UPDATE_STATUS.lock() {
                        status.percentage = 0;
                        status.status = "Erreur lors de l'upload direct";
                    }
                    return Err(anyhow!("Failed reading raw post body: {:?}", e));
                }
            }
        }
        println!();
        
        info!("\x1b[35;1m[ÉTAPE 5] Écriture en mémoire flash\x1b[0m");
        info!("\x1b[36;1m  -> Finalisation de la partition (Total écrit : {} octets)...\x1b[0m", total_read);
        {
            let mut status = crate::ota::UPDATE_STATUS.lock().unwrap();
            status.percentage = 100;
            status.written = total_read;
            status.status = "Écriture en mémoire flash...";
        }
        ota_write.complete().context("Failed to complete OTA")?;
        info!("\x1b[35;1m[ÉTAPE 5] Flashage de l'upload direct terminé avec succès !\x1b[0m");
        
        info!("\x1b[35;1m[ÉTAPE 6] Reboot de l'appareil\x1b[0m");
        info!("\x1b[36;1m  -> Firmware actif lors du reboot : recovery_boot ({})\x1b[0m", FW_VERSION);
        info!("\x1b[36;1m  -> Programmation du redémarrage dans 2 secondes...\x1b[0m");
        {
            let mut status = crate::ota::UPDATE_STATUS.lock().unwrap();
            status.percentage = 100;
            status.status = "Mise à jour terminée. Redémarrage...";
        }
        thread::spawn(|| {
            thread::sleep(std::time::Duration::from_secs(2));
            unsafe {
                esp_idf_sys::esp_restart();
            }
        });

        let mut response = req.into_ok_response()?;
        response.write(b"OK")?;
        Ok(())
    })?;

    // Prevent main thread from exiting
    loop {
        thread::sleep(std::time::Duration::from_secs(60));
    }
}

fn get_formatted_time() -> String {
    let now = SystemTime::now();
    let duration = now.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
    let total_secs = duration.as_secs();
    
    if total_secs < 86400 {
        return "1970-01-01T00:00:00Z".to_string();
    }
    
    // Formatting a simple RFC 3339 style timestamp without extra chrono crate dependencies
    // to save precious binary size (opt-level: size)
    let secs = total_secs % 60;
    let mins = (total_secs / 60) % 60;
    let hours = (total_secs / 3600) % 24;
    // let days = total_secs / 86400;
    
    format!("2026-05-27T{:02}:{:02}:{:02}Z", hours, mins, secs)
}

































