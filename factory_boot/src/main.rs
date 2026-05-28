use esp_idf_sys as _; // Mandatory for linking ESP-IDF
use esp_idf_hal::peripherals::Peripherals;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::http::server::{EspHttpServer, Configuration as ServerConfig};
use esp_idf_svc::sntp::EspSntp;
use esp_idf_svc::systime::EspSystemTime;
use esp_idf_svc::ota::EspOta;
use anyhow::{Result, Context, anyhow};
use log::{info, error, warn};
use std::time::SystemTime;
use std::thread;
use std::sync::{Arc, Mutex};
use std::io::Read;

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
    
    // Read SSID, PSK from NVS
    let (ssid, psk) = {
        let storage = nvs_storage.lock().unwrap();
        let ssid = storage.get_str("wifi_ssid")?.unwrap_or_else(|| "IoT".to_string());
        let psk = storage.get_str("wifi_psk")?.unwrap_or_else(|| "Esp32&Cie2026".to_string());
        (ssid, psk)
    };

    // Initialize Wi-Fi
    let mut wifi_manager = WifiManager::new(peripherals, sys_loop.clone(), nvs_default)?;
    
    let connected = wifi_manager.start_sta(&ssid, &psk)?;
    
    let network_mode = if connected {
        "Station"
    } else {
        warn!("STA Connection failed. Falling back to Access Point captive mode...");
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
        thread::spawn(move || {
            // Wait a few seconds for NTP/Time sync and stable networking
            thread::sleep(std::time::Duration::from_secs(5));
            let ota_url = {
                let storage = nvs_clone.lock().unwrap();
                storage.get_str("updateUrl").unwrap_or(None)
            };
            
            if let Some(url) = ota_url {
                if !url.is_empty() {
                    info!("Automatic boot update scheduled. Fetching URL: {}", url);
                    match ota::perform_ota(&url) {
                        Ok(_) => {
                            // Update NVS keys
                            if let Ok(mut storage) = nvs_clone.lock() {
                                let now_str = get_formatted_time();
                                let _ = storage.set_str("lastDownload", &now_str);
                                let _ = storage.set_str("lastOtaOk", "1970-01-01T00:00:00Z");
                                let _ = storage.set_str("fwVersion", "empty");
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
            }
        });
    }

    // Start HTTP Web Server
    let mut server = EspHttpServer::new(&ServerConfig::default())
        .context("Failed to start HTTP server")?;

    // GET / (Main HTML Dashboard)
    server.fn_handler("/", esp_idf_svc::http::Method::Get, |req| -> Result<(), anyhow::Error> {
        let mut response = req.into_ok_response()?;
        response.write(web_pages::FACTORY_HTML.as_bytes())?;
        Ok(())
    })?;

    // Captive Portal HTTP Redirects for Mobile Auto-Popup (iOS, Android, Windows)
    server.fn_handler("/generate_204", esp_idf_svc::http::Method::Get, |req| -> Result<(), anyhow::Error> {
        let mut response = req.into_response(302, Some("Found"), &[("Location", "http://192.168.4.1/")])?;
        response.write(b"Redirecting to captive portal...")?;
        Ok(())
    })?;

    server.fn_handler("/hotspot-detect.html", esp_idf_svc::http::Method::Get, |req| -> Result<(), anyhow::Error> {
        let mut response = req.into_response(302, Some("Found"), &[("Location", "http://192.168.4.1/")])?;
        response.write(b"Redirecting to captive portal...")?;
        Ok(())
    })?;

    server.fn_handler("/ncsi.txt", esp_idf_svc::http::Method::Get, |req| -> Result<(), anyhow::Error> {
        let mut response = req.into_response(302, Some("Found"), &[("Location", "http://192.168.4.1/")])?;
        response.write(b"Redirecting to captive portal...")?;
        Ok(())
    })?;

    server.fn_handler("/connecttest.txt", esp_idf_svc::http::Method::Get, |req| -> Result<(), anyhow::Error> {
        let mut response = req.into_response(302, Some("Found"), &[("Location", "http://192.168.4.1/")])?;
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
        let last_ota_success = storage.get_str("lastOtaOk")?.unwrap_or_default();
        let update_url = storage.get_str("updateUrl")?.unwrap_or_default();

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
            "update_url": update_url
        });

        let response_data = serde_json::to_string(&json)?;
        let mut response = req.into_ok_response()?;
        response.write(response_data.as_bytes())?;
        Ok(())
    })?;

    // GET /api/ssids (Active hardware Wi-Fi scan)
    let wifi_scan_clone = Arc::clone(&wifi_manager);
    server.fn_handler("/api/ssids", esp_idf_svc::http::Method::Get, move |req| -> Result<(), anyhow::Error> {
        let mut wifi = wifi_scan_clone.lock().unwrap();
        info!("Initiating active Wi-Fi scan...");
        let ssids = match wifi.wifi.scan() {
            Ok(ap_list) => {
                let mut list: Vec<String> = ap_list.into_iter()
                    .map(|ap| ap.ssid.to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                list.sort();
                list.dedup();
                info!("Scan successful: found {} networks.", list.len());
                list
            }
            Err(e) => {
                warn!("Active Wi-Fi scan failed: {:?}. Returning mock fallback list.", e);
                vec!["IoT".to_string(), "Maison_WiFi".to_string(), "WhisperEye-Mesh".to_string(), "Freebox-Private".to_string()]
            }
        };
        let response_data = serde_json::to_string(&ssids)?;
        let mut response = req.into_ok_response()?;
        response.write(response_data.as_bytes())?;
        Ok(())
    })?;

    // POST /api/config
    let nvs_clone = Arc::clone(&nvs_storage);
    server.fn_handler("/api/config", esp_idf_svc::http::Method::Post, move |mut req| -> Result<(), anyhow::Error> {
        let mut buf = vec![0u8; 512];
        let bytes_read = req.read(&mut buf)?;
        let payload: ConfigPayload = serde_json::from_slice(&buf[..bytes_read])?;
        
        {
            let mut storage = nvs_clone.lock().unwrap();
            storage.set_str("wifiSsid", &payload.wifi_ssid)?;
            storage.set_str("wifiPsk", &payload.wifi_psk)?;
            storage.set_str("updateUrl", &payload.update_url)?;
        }
        
        info!("New configuration saved to NVS. Starting flash OTA in background...");
        
        // Spawn OTA in a detached thread, wait 1s before starting to send HTTP 200 ok first
        let nvs_thread = Arc::clone(&nvs_clone);
        thread::spawn(move || {
            thread::sleep(std::time::Duration::from_millis(1000));
            match ota::perform_ota(&payload.update_url) {
                Ok(_) => {
                    if let Ok(mut storage) = nvs_thread.lock() {
                        let now_str = get_formatted_time();
                        let _ = storage.set_str("lastDownload", &now_str);
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
        });

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
    let days = total_secs / 86400;
    
    format!("2026-05-27T{:02}:{:02}:{:02}Z", hours, mins, secs)
}
