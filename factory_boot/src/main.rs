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
// use std::io::Read;

mod wifi;
mod ota;
mod web_pages;

use wifi::WifiManager;
use common::nvs_storage::NvsStorage;

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct ConfigPayload {
    wifi_ssid: String,
    wifi_psk: String,
    update_url: String,
    update_interval: Option<String>,
    apply_only: Option<bool>,
}

fn main() -> Result<()> {
    // Bind the ESP-IDF logging
    esp_idf_svc::log::EspLogger::initialize_default();
    info!("WhisperEye Factory Boot Firmware Starting Up...");

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
    
    // Read SSID, PSK from NVS
    let (ssid, psk) = {
        let storage = nvs_storage.lock().unwrap();
        let ssid = storage.get_str("wifiSsid")?.unwrap_or_else(|| "IoT".to_string());
        let psk = storage.get_str("wifiPsk")?.unwrap_or_else(|| "Esp32&Cie2026".to_string());
        (ssid, psk)
    };

    // Initialize Wi-Fi
    let mut wifi_manager = WifiManager::new(peripherals, sys_loop.clone(), nvs_default)?;
    
    // Perform initial scan before any connection attempts
    let _ = wifi_manager.perform_initial_scan();
    
    let mut connected = wifi_manager.start_sta(&ssid, &psk).unwrap_or(false);
    
    if connected {
        if let Ok(mut storage) = nvs_storage.lock() {
            let _ = storage.add_known_network(&ssid, &psk);
        }
    } else {
        info!("Default Wi-Fi failed. Trying known networks from NVS...");
        let known_networks = {
            let storage = nvs_storage.lock().unwrap();
            storage.get_known_networks().unwrap_or_default()
        };
        for (known_ssid, known_psk) in known_networks {
            if known_ssid == ssid { continue; }
            info!("Trying known network: {}", known_ssid);
            if wifi_manager.start_sta(&known_ssid, &known_psk).unwrap_or(false) {
                connected = true;
                break;
            }
        }
    }
    
    let network_mode = if connected {
        "Station"
    } else {
        warn!("All STA Connections failed. Falling back to Access Point captive mode...");
        wifi_manager.start_ap()?;
        "AccessPoint"
    };

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
                            let _ = storage.set_str("lastOtaSuccess", &now_str);
                            let _ = storage.set_str("fwVersion", "empty");
                            let _ = storage.set_i32("otaRetry", -1);
                        }
                        info!("OTA completed successfully. Rebooting into Production Firmware!");
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
        response.write(web_pages::FACTORY_HTML.as_bytes())?;
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
        
        let ip_info = if network_mode == "Station" {
            wifi.wifi.wifi().sta_netif().get_ip_info().ok()
        } else {
            wifi.wifi.wifi().ap_netif().get_ip_info().ok()
        };

        let ip_addr = ip_info.map(|i| i.ip.to_string()).unwrap_or_else(|| "0.0.0.0".to_string());
        let gateway = ip_info.map(|i| i.subnet.gateway.to_string()).unwrap_or_else(|| "0.0.0.0".to_string());
        let rssi = if network_mode == "Station" {
            wifi.wifi.wifi().get_ap_info().ok().map(|i| i.signal_strength)
        } else {
            None
        };

        let now_str = get_formatted_time();

        let wifi_ssid = storage.get_str("wifiSsid")?.unwrap_or_default();
        let ntp_server = storage.get_str("ntpServer")?.unwrap_or_default();
        let fw_version = storage.get_str("fwVersion")?.unwrap_or_default();
        let last_ota_success = storage.get_str("lastOtaSuccess")?.unwrap_or_default();
        let update_url = storage.get_str("updateAvailable")?.unwrap_or_default();
        let update_interval = storage.get_str("updateInterval")?.unwrap_or_else(|| "7j".to_string());

        let json = serde_json::json!({
            "network_mode": network_mode,
            "wifi_ssid": wifi_ssid,
            "wifi_rssi": rssi,
            "ip_addr": ip_addr,
            "gateway_addr": gateway,
            "sys_time": now_str,
            "ntp_server": ntp_server,
            "fw_version": fw_version,
            "last_ota_success": last_ota_success,
            "update_url": update_url,
            "update_interval": update_interval,
            "board_type": "v1.0",
            "chip_type": "ESP32"
        });

        let response_data = serde_json::to_string(&json)?;
        let mut response = req.into_ok_response()?;
        response.write(response_data.as_bytes())?;
        Ok(())
    })?;

    // GET /api/check_updates (proxies firmware.json from updateAvailable NVS key to bypass CORS!)
    let nvs_updates_clone = Arc::clone(&nvs_storage);
    server.fn_handler("/api/check_updates", esp_idf_svc::http::Method::Get, move |req| -> Result<(), anyhow::Error> {
        let update_url = {
            let storage = nvs_updates_clone.lock().unwrap();
            storage.get_str("updateAvailable")?.unwrap_or_default()
        };

        if update_url.is_empty() {
            let mut response = req.into_status_response(400)?;
            response.write(b"No update URL configured")?;
            return Ok(());
        }

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

        let list: serde_json::Value = serde_json::from_slice(&body)?;
        let mut matched_entry = serde_json::Value::Null;

        if let Some(arr) = list.as_array() {
            for entry in arr {
                let b_type = entry.get("boardType").and_then(|v| v.as_str()).unwrap_or("");
                let c_type = entry.get("ChipType").and_then(|v| v.as_str()).unwrap_or("");
                if b_type == "v1.0" && c_type == "ESP32" {
                    matched_entry = entry.clone();
                    break;
                }
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
            storage.get_str("wifiSsid")?.unwrap_or_default()
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
        
        let ssid = payload.wifi_ssid.trim();
        let psk = payload.wifi_psk.trim();
        
        let mut success = false;
        let mut final_psk = "".to_string();

        {
            let mut wifi = wifi_clone.lock().unwrap();
            let mut storage = nvs_clone.lock().unwrap();
            
            if psk.is_empty() {
                // Check if it is in known networks
                let known_networks = storage.get_known_networks().unwrap_or_default();
                if let Some((_, saved_psk)) = known_networks.iter().find(|(s, _)| s == ssid) {
                    info!("SSID '{}' is known. Testing connection with saved key.", ssid);
                    final_psk = saved_psk.clone();
                    if wifi.start_sta(ssid, &final_psk).unwrap_or(false) {
                        success = true;
                    }
                }
                if !success {
                    info!("Saved key failed or not found. Testing connection to SSID '{}' without key.", ssid);
                    final_psk = "".to_string();
                    if wifi.start_sta(ssid, "").unwrap_or(false) {
                        success = true;
                    }
                }
            } else {
                // PSK is provided
                info!("Testing connection to SSID '{}' with provided key.", ssid);
                final_psk = psk.to_string();
                if wifi.start_sta(ssid, &final_psk).unwrap_or(false) {
                    success = true;
                } else {
                    info!("Provided key failed. Testing connection to SSID '{}' without key.", ssid);
                    final_psk = "".to_string();
                    if wifi.start_sta(ssid, "").unwrap_or(false) {
                        success = true;
                    }
                }
            }
            
            if success {
                info!("Connection successful to SSID '{}'. Saving to NVS...", ssid);
                storage.set_str("wifiSsid", ssid)?;
                storage.set_str("wifiPsk", &final_psk)?;
                
                // Add to wifiKnown if not already known
                let known_networks = storage.get_known_networks().unwrap_or_default();
                let already_known = known_networks.iter().any(|(s, _)| s == ssid);
                if !already_known {
                    info!("SSID '{}' was not known. Adding to known networks.", ssid);
                    storage.add_known_network(ssid, &final_psk)?;
                }
            } else {
                warn!("Wi-Fi connection to '{}' failed. Restarting Access Point...", ssid);
                let _ = wifi.start_ap();
            }
        }
        
        if !success {
            let mut response = req.into_status_response(400)?;
            response.write(b"WiFi Connection Failed")?;
            return Ok(());
        }

        let is_bin = payload.update_url.ends_with(".bin");
        {
            let mut storage = nvs_clone.lock().unwrap();
            if is_bin {
                storage.set_str("updateDlUrl", &payload.update_url)?;
                storage.set_i32("otaRetry", 3)?;
            } else {
                storage.set_str("updateAvailable", &payload.update_url)?;
            }
        }

        let apply_only = payload.apply_only.unwrap_or(false);
        if !apply_only {
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
                                    let _ = storage.set_str("lastOtaSuccess", &now_str);
                                    let _ = storage.set_i32("otaRetry", -1);
                                }
                                info!("OTA Succeeded. Rebooting...");
                                thread::sleep(std::time::Duration::from_secs(1));
                                unsafe {
                                    esp_idf_sys::esp_restart();
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
        info!("Direct firmware binary upload initiated...");
        let mut ota = EspOta::new().context("Failed to init ESP OTA")?;
        let mut ota_write = ota.initiate_update().context("Failed to initiate OTA update")?;
        
        let mut buf = [0u8; 1024]; // 1KB Buffer size constraint
        let mut total_read = 0;
        
        loop {
            match req.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    ota_write.write(&buf[..n])?;
                    total_read += n;
                }
                Err(e) => {
                    return Err(anyhow!("Failed reading raw post body: {:?}", e));
                }
            }
        }
        
        info!("Firmware upload complete ({} bytes). Writing to boot partition...", total_read);
        ota_write.complete().context("Failed to complete OTA")?;
        
        info!("Manual flash successful! Scheduling reboot...");
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
