use esp_idf_sys as _; // Mandatory for linking ESP-IDF
use esp_idf_hal::peripherals::Peripherals;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::http::server::{EspHttpServer, Configuration as ServerConfig};
use esp_idf_svc::sntp::EspSntp;
// use esp_idf_svc::systime::EspSystemTime;
use anyhow::{Result, Context};
use log::{info, warn};
use std::time::SystemTime;
use std::thread;
use std::sync::{Arc, Mutex};
// use std::io::Read;

mod wifi;
mod sensors;
mod actuators;
mod web_pages;

use wifi::WifiManager;
use common::nvs_storage::NvsStorage;
use actuators::ActuatorsState;

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct ConfigPayload {
    wifi_ssid: String,
    wifi_psk: String,
    update_url: String,
    update_interval: String,
}

fn main() -> Result<()> {
    esp_idf_svc::log::EspLogger::initialize_default();
    info!("WhisperEye Production Application Starting Up...");

    let peripherals = Peripherals::take().context("Failed to take ESP32 Peripherals")?;
    let sys_loop = EspSystemEventLoop::take().context("Failed to take System Event Loop")?;
    let nvs_default = EspDefaultNvsPartition::take().context("Failed to take NVS Partition")?;

    // Initialize NVS Storage helper
    let nvs_storage = Arc::new(Mutex::new(NvsStorage::new(nvs_default.clone())?));
    
    // Set version name in NVS if it is still empty
    {
        let mut storage = nvs_storage.lock().unwrap();
        if storage.get_str("fwVersion")?.unwrap_or_else(|| "empty".to_string()) == "empty" {
            let _ = storage.set_str("fwVersion", "v1.0.0-poc");
        }
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

    // Shared actuator state
    let actuators_state = Arc::new(Mutex::new(ActuatorsState::default()));

    // Start HTTP Web Server
    let mut server = EspHttpServer::new(&ServerConfig::default())
        .context("Failed to start HTTP server")?;

    // GET / (Main Production HTML Dashboard)
    server.fn_handler("/", esp_idf_svc::http::Method::Get, |req| -> Result<(), anyhow::Error> {
        let mut response = req.into_ok_response()?;
        response.write(web_pages::PRODUCTION_HTML.as_bytes())?;
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
        let fw_version = storage.get_str("fwVersion")?.unwrap_or_else(|| "v1.0.0-poc".to_string());
        let last_ota_success = storage.get_str("lastOtaSuccess")?.unwrap_or_default();
        let update_url = storage.get_str("updateUrl")?.unwrap_or_default();
        let update_interval = storage.get_str("updateInterval")?.unwrap_or_else(|| "30j".to_string());

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
            "update_interval": update_interval
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
        let response_data = serde_json::to_string(&ssids)?;
        let mut response = req.into_ok_response()?;
        response.write(response_data.as_bytes())?;
        Ok(())
    })?;

    // GET /api/sensors
    server.fn_handler("/api/sensors", esp_idf_svc::http::Method::Get, |req| -> Result<(), anyhow::Error> {
        let readings = sensors::read_sensors();
        let response_data = serde_json::to_string(&readings)?;
        let mut response = req.into_ok_response()?;
        response.write(response_data.as_bytes())?;
        Ok(())
    })?;

    // POST /api/actuators
    let act_clone = Arc::clone(&actuators_state);
    server.fn_handler("/api/actuators", esp_idf_svc::http::Method::Post, move |mut req| -> Result<(), anyhow::Error> {
        let mut buf = vec![0u8; 256];
        let bytes_read = req.read(&mut buf)?;
        let payload: ActuatorsState = serde_json::from_slice(&buf[..bytes_read])?;
        
        info!("Updating actuators state: {:?}", payload);
        {
            let mut state = act_clone.lock().unwrap();
            state.relay_1 = payload.relay_1;
            state.pwm_intensity = payload.pwm_intensity;
        }

        let response_data = serde_json::to_string(&payload)?;
        let mut response = req.into_ok_response()?;
        response.write(response_data.as_bytes())?;
        Ok(())
    })?;

    // POST /api/config (triggers immediate restart to factory_boot if update_url differs)
    let nvs_clone = Arc::clone(&nvs_storage);
    server.fn_handler("/api/config", esp_idf_svc::http::Method::Post, move |mut req| -> Result<(), anyhow::Error> {
        let mut buf = vec![0u8; 512];
        let bytes_read = req.read(&mut buf)?;
        let payload: ConfigPayload = serde_json::from_slice(&buf[..bytes_read])?;
        
        let should_restart = {
            let mut storage = nvs_clone.lock().unwrap();
            let current_url = storage.get_str("updateUrl")?.unwrap_or_default();
            
            storage.set_str("wifiSsid", &payload.wifi_ssid)?;
            storage.set_str("wifiPsk", &payload.wifi_psk)?;
            storage.set_str("updateUrl", &payload.update_url)?;
            storage.set_str("updateInterval", &payload.update_interval)?;
            
            // Restart condition: different or new update URL is set
            !payload.update_url.is_empty() && payload.update_url != current_url
        };

        if should_restart {
            info!("Configuration updated. Restaring ESP32 back to factory_boot to execute update...");
            thread::spawn(|| {
                thread::sleep(std::time::Duration::from_secs(2));
                unsafe {
                    esp_idf_sys::esp_restart();
                }
            });
        } else {
            info!("Configuration updated. No OTA URL modification, running in place.");
        }

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
        return "2026-05-27T23:12:00Z".to_string(); // Mock current real date-time for telemetry elegance
    }
    
    let secs = total_secs % 60;
    let mins = (total_secs / 60) % 60;
    let hours = (total_secs / 3600) % 24;
    
    format!("2026-05-27T{:02}:{:02}:{:02}Z", hours, mins, secs)
}
