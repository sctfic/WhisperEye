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

const WHISPEREYE_BOARD:  &str = "1.0";
const CHIP_TYPE:  &str = "ESP32-S3";
const FW_VERSION: &str = "1.0.19-0011";
#[allow(dead_code)]
const TOTP_SECRET: &str = "Salt-4-Hash-Between-Probe-&-WhisperEye";

const AUTHOR_EMAIL: &str = "alban.lopez+whisperEye@gmail.com";
const AUTHOR_NAME: &str = "LOPEZ Alban";
const AUTHOR_LINK: &str = "https://github.com/sctfic/WhisperEye/blob/main/README.md";

mod wifi;
mod sensors;
mod actuators;
mod web_pages;
mod cron;
mod static_devices;
mod dynamic_devices;
mod ds18b20;
mod i2c_bus;
mod screen;
mod radio;

use wifi::WifiManager;
use common::nvs_storage::NvsStorage;
use actuators::ActuatorsState;

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct ConfigPayload {
    wifi_ssid: Option<String>,
    wifi_psk: Option<String>,
    update_url: Option<String>,
    update_interval: Option<String>,
    apply_only: Option<bool>,
    auto_update: Option<bool>,
    totp_secret: Option<String>,
    current_totp_secret: Option<String>,
    ext_name: Option<String>,
    ext_desc: Option<String>,
    ntp_server: Option<String>,
    metrics_url: Option<String>,
    rename_enabled: Option<bool>,
    mesh_enabled: Option<bool>,
    mesh_channel: Option<i32>,
    mesh_id: Option<String>,
    mesh_pmk: Option<String>,
}

fn get_mac_address() -> String {
    let mut mac = [0u8; 6];
    unsafe {
        esp_idf_sys::esp_read_mac(mac.as_mut_ptr(), esp_idf_sys::esp_mac_type_t_ESP_MAC_WIFI_STA);
    }
    format!("{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}", mac[0], mac[1], mac[2], mac[3], mac[4], mac[5])
}

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
            println!("\x1b[35;1m{}\x1b[0m ({}) {}: {}", marker, timestamp, record.target(), record.args());
        }
    }

    fn flush(&self) {}
}

static LOGGER: CustomLogger = CustomLogger;

struct MeshState {
    pub enabled: bool,
    pub is_root: bool,
    pub distance: i32,
    pub nodes: std::collections::HashMap<String, std::time::SystemTime>,
}

fn perform_mesh_sync(nvs: &Arc<Mutex<NvsStorage>>) -> Result<i32> {
    info!("Syncing with Mesh parent at http://192.168.71.1/api/mesh/sync...");
    let config = esp_idf_svc::http::client::Configuration {
        buffer_size: Some(1024),
        crt_bundle_attach: None,
        ..Default::default()
    };
    let mut connection = esp_idf_svc::http::client::EspHttpConnection::new(&config)?;
    let url = format!("http://192.168.71.1/api/mesh/sync?mac={}", get_mac_address());
    connection.initiate_request(esp_idf_svc::http::Method::Get, &url, &[])?;
    connection.initiate_response()?;
    
    let status = connection.status();
    if status != 200 {
        anyhow::bail!("Parent sync returned HTTP status {}", status);
    }
    
    let mut body = Vec::new();
    let mut chunk = [0u8; 512];
    loop {
        match connection.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => body.extend_from_slice(&chunk[..n]),
            Err(e) => anyhow::bail!("Failed to read response: {:?}", e),
        }
    }
    
    #[derive(serde::Deserialize)]
    struct SyncResponse {
        wifi_ssid: String,
        wifi_psk: String,
        ntp_server: String,
        distance: i32,
    }
    
    let res: SyncResponse = serde_json::from_slice(&body)?;
    
    // Save to NVS
    {
        let mut storage = nvs.lock().unwrap();
        if !res.wifi_ssid.is_empty() {
            let known = storage.get_known_networks().unwrap_or_default();
            if !known.contains_key(&res.wifi_ssid) {
                info!("Sync: Saving wifi SSID '{}' to NVS (newly discovered)", res.wifi_ssid);
                storage.set_default_network(&res.wifi_ssid, &res.wifi_psk)?;
            } else {
                info!("Sync: Wifi SSID '{}' is already known, skipping save", res.wifi_ssid);
            }
        }
        if !res.ntp_server.is_empty() {
            info!("Sync: Saving ntpServer '{}' to NVS", res.ntp_server);
            storage.set_str("ntpServer", &res.ntp_server)?;
        }
    }
    
    Ok(res.distance)
}

fn main() -> Result<()> {
    log::set_logger(&LOGGER)
        .map(|()| log::set_max_level(log::LevelFilter::Info))
        .expect("Failed to initialize custom logger");
    
    // Allumer la LED en Jaune à 10% (R=25, G=25, B=0) à la mise sous tension
    common::led::set_led_color(25, 25, 0);

    info!("\x1b[35mWhisperEye Production Application Starting Up (Version {})...\x1b[0m", FW_VERSION);


    let peripherals = Peripherals::take().context("Failed to take ESP32 Peripherals")?;
    let sys_loop = EspSystemEventLoop::take().context("Failed to take System Event Loop")?;
    let nvs_default = EspDefaultNvsPartition::take().context("Failed to take NVS Partition")?;

    // Initialize NVS Storage helper
    let nvs_storage = Arc::new(Mutex::new(NvsStorage::new(nvs_default.clone())?));
    
    // Set version name in NVS, update otaRetry and lastOtaSuccess on successful boot
    {
        let mut storage = nvs_storage.lock().unwrap();
        let saved_version = storage.get_str("fwVersion")?.unwrap_or_else(|| "empty".to_string());
        
        // If we are booting a new firmware version (either empty or upgraded)
        if saved_version != FW_VERSION {
            info!("New firmware version detected! Upgrading NVS from '{}' to '{}'...", saved_version, FW_VERSION);
            let _ = storage.set_str("fwVersion", FW_VERSION);
            
            // Set lastOtaSuccess only upon successful boot of the production firmware
            let now_str = get_formatted_time();
            let _ = storage.set_str("lastOtaSuccess", &now_str);
        }
        
        // Always reset otaRetry to -1 when booting successfully on production
        let _ = storage.set_i32("otaRetry", -1);
        
        let _ = storage.dump_to_log();
    }

    // Unpack peripherals components to prevent borrow-checker moves
    let pins = peripherals.pins;
    let modem = peripherals.modem;

    // Initialize Static Devices from pins
    let static_devs = Arc::new(std::sync::Mutex::new(static_devices::StaticDevices::init(
        pins.gpio9,
        pins.gpio47,
        pins.gpio21,
        pins.gpio14,
        pins.gpio36,
        pins.gpio35,
    )?));


    // Scan 1-Wire bus dynamically at boot
    let onewr_pin = pins.gpio39;
    let discovered_probes = if let Ok(mut ow) = ds18b20::OneWire::new(onewr_pin) {
        ow.search_roms()
    } else {
        Vec::new()
    };
    info!("Dynamic 1-Wire scan found {} DS18B20 probes.", discovered_probes.len());
    let discovered_probes = Arc::new(discovered_probes);

    // Register all static and dynamic devices in NVS registry
    {
        let mut registry = dynamic_devices::DeviceRegistry::new(Arc::clone(&nvs_storage));
        let _ = registry.scan_and_register((*discovered_probes).clone());
    }

    // Initialize Wi-Fi (consuming only the modem, leaving other pins untouched)
    let mut wifi_manager = WifiManager::new(modem, sys_loop.clone(), nvs_default)?;
    
    // Perform initial scan before any connection attempts
    let _ = wifi_manager.perform_initial_scan();
    
    let (mesh_enabled, mesh_channel, mesh_id, mesh_pmk) = {
        let storage = nvs_storage.lock().unwrap();
        let enabled = storage.get_i32("meshEnabled").unwrap_or(Some(1)).unwrap_or(1) == 1;
        let channel = storage.get_i32("meshChannel").unwrap_or(Some(11)).unwrap_or(11) as u8;
        let id = storage.get_str("meshId").unwrap_or(Some("Whiper".to_string())).unwrap_or_else(|| "Whiper".to_string());
        let pmk = storage.get_str("meshPmk").unwrap_or(Some("WhiperEyes@Mesh!".to_string())).unwrap_or_else(|| "WhiperEyes@Mesh!".to_string());
        (enabled, channel, id, pmk)
    };

    let mut connected = false;
    let mut chosen_ssid = String::new();
    let mut is_root = false;
    let mut distance = -1;

    let known_networks = {
        let storage = nvs_storage.lock().unwrap();
        storage.get_known_networks().unwrap_or_default()
    };

    if mesh_enabled {
        info!("Mesh is enabled. Starting Mesh AP first, then trying STA connections...");

        // 1. Démarrer l'AP Mesh immédiatement → les clients Mesh/frontend peuvent se connecter dès maintenant
        wifi_manager.start_mesh_ap_only(&mesh_id, &mesh_pmk, mesh_channel)?;

        // 2. Tenter la connexion STA vers les réseaux Wi-Fi connus (sans couper l'AP)
        if !known_networks.is_empty() {
            let default_net = known_networks.iter()
                .find(|(_, entry)| entry.default.unwrap_or(false))
                .map(|(ssid, entry)| (ssid.clone(), entry.psk.clone()));

            // Essayer le réseau par défaut en premier
            if let Some((ref ssid, ref psk)) = default_net {
                info!("Trying default network '{}' with Mesh AP active...", ssid);
                if wifi_manager.try_sta_on_mesh(ssid, psk, &mesh_id, &mesh_pmk, mesh_channel).unwrap_or(false) {
                    connected = true;
                    chosen_ssid = ssid.clone();
                    is_root = true;
                    distance = 0;
                }
            }

            // Si le défaut échoue, essayer les autres réseaux connus
            if !connected {
                for (ssid, entry) in &known_networks {
                    if let Some((ref def_ssid, _)) = default_net {
                        if ssid == def_ssid { continue; }
                    }
                    info!("Trying known network '{}' with Mesh AP active...", ssid);
                    if wifi_manager.try_sta_on_mesh(ssid, &entry.psk, &mesh_id, &mesh_pmk, mesh_channel).unwrap_or(false) {
                        connected = true;
                        chosen_ssid = ssid.clone();
                        is_root = true;
                        distance = 0;
                        break;
                    }
                }
            }
        }

        // 3. Si toujours pas connecté au Wi-Fi local, chercher un parent Mesh
        if !connected {
            info!("No Wi-Fi router available. Scanning for parent Mesh SSID '{}'...", mesh_id);
            let found_mesh_parent = match wifi_manager.wifi.scan() {
                Ok(ap_list) => {
                    ap_list.into_iter().any(|ap| ap.ssid.as_str() == mesh_id)
                }
                Err(_) => {
                    wifi_manager.scan_cache.iter().any(|s| s == &mesh_id)
                }
            };

            if found_mesh_parent {
                info!("Found parent Mesh network '{}'. Connecting and syncing...", mesh_id);
                if wifi_manager.try_sta_on_mesh(&mesh_id, &mesh_pmk, &mesh_id, &mesh_pmk, mesh_channel).unwrap_or(false) {
                    info!("Connected to Mesh parent. Synchronizing configuration...");
                    match perform_mesh_sync(&nvs_storage) {
                        Ok(parent_distance) => {
                            connected = true;
                            chosen_ssid = mesh_id.clone();
                            is_root = false;
                            distance = parent_distance + 1;
                            info!("Mesh sync successful! Distance to root: {}", distance);
                        }
                        Err(e) => {
                            warn!("Mesh synchronization failed: {:?}", e);
                        }
                    }
                }
            } else {
                info!("No parent Mesh network '{}' detected.", mesh_id);
            }
        }

        // 4. Résultat du boot
        if !connected {
            warn!("No STA connection established. Mesh AP '{}' remains active for direct client access.", mesh_id);
            is_root = false;
            distance = -1;
            common::led::set_led_color(25, 25, 0); // Jaune : AP seul, pas de connexion routeur
        } else {
            if is_root {
                if let Ok(mut storage) = nvs_storage.lock() {
                    let _ = storage.set_default_network_by_ssid(&chosen_ssid);
                }
            }
            common::led::set_led_color(0, 25, 0); // Vert : connecté au routeur Wi-Fi
        }

    } else {
        info!("Mesh is disabled. Running standard Wi-Fi boot sequence...");
        if !known_networks.is_empty() {
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
                common::led::set_led_color(0, 25, 0);
            }
        } else {
            info!("No known networks. Staying in AP mode.");
        }

        if !connected {
            warn!("All STA Connections failed or no known networks. Falling back to Access Point captive mode...");
            wifi_manager.start_ap()?;
            common::led::set_led_color(25, 25, 0); // Jaune en AP
        }
    }

    let wifi_manager = Arc::new(Mutex::new(wifi_manager));

    let mesh_state = Arc::new(Mutex::new(MeshState {
        enabled: mesh_enabled,
        is_root,
        distance,
        nodes: std::collections::HashMap::new(),
    }));

    // Initialize SNTP if connected to STA
    let _sntp = if connected {
        let ntp_server = {
            let storage = nvs_storage.lock().unwrap();
            storage.get_str("ntpServer").ok().flatten().unwrap_or_default()
        };

        let sntp = if !ntp_server.is_empty() && ntp_server != "empty" {
            info!("Initializing SNTP with custom server: {} and fallback pool.ntp.org", ntp_server);
            let mut conf = esp_idf_svc::sntp::SntpConf::default();
            conf.servers[0] = &ntp_server;
            let s = EspSntp::new(&conf);
            if s.is_ok() {
                unsafe {
                    if let Ok(fallback) = std::ffi::CString::new("pool.ntp.org") {
                        esp_idf_sys::esp_sntp_setservername(1, fallback.as_ptr());
                    }
                }
            }
            s
        } else {
            info!("Initializing SNTP default pool...");
            EspSntp::new_default()
        };

        if sntp.is_err() {
            warn!("Failed to initialize SNTP service");
        }
        sntp.ok()
    } else {
        None
    };

    // Spawn robust periodic task scheduler
    let cron_handle = cron::spawn_cron_scheduler(Arc::clone(&nvs_storage))
        .context("Failed to spawn cron periodic task scheduler")?;

    // Shared actuator state
    let actuators_state = Arc::new(Mutex::new(ActuatorsState::default()));

    // Start HTTP Web Server
    let mut server = EspHttpServer::new(&ServerConfig::default())
        .context("Failed to start HTTP server")?;

    // GET /favicon.ico (Favicon file)
    server.fn_handler("/favicon.ico", esp_idf_svc::http::Method::Get, |req| -> Result<(), anyhow::Error> {
        let mut response = req.into_response(200, Some("OK"), &[("Content-Type", "image/x-icon")])?;
        response.write(include_bytes!("../../common/src/favicon.ico"))?;
        Ok(())
    })?;

    // GET / (Main Production HTML Dashboard)
    server.fn_handler("/", esp_idf_svc::http::Method::Get, |req| -> Result<(), anyhow::Error> {
        let mut response = req.into_ok_response()?;
        response.write(web_pages::PRODUCTION_HTML.as_bytes())?;
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

    // GET /api/mesh/sync
    let nvs_sync = Arc::clone(&nvs_storage);
    let mesh_state_sync = Arc::clone(&mesh_state);
    server.fn_handler("/api/mesh/sync", esp_idf_svc::http::Method::Get, move |req| -> Result<(), anyhow::Error> {
        let uri = req.uri();
        let mut mac = if let Some(pos) = uri.find("mac=") {
            let raw_mac = &uri[pos + 4..];
            raw_mac.split('&').next().unwrap_or("").to_string()
        } else {
            "unknown".to_string()
        };
        mac = mac.replace("%3A", ":").replace("%3a", ":");
        
        info!("Received Mesh sync request from MAC: {}", mac);
        
        if mac != "unknown" {
            let mut state = mesh_state_sync.lock().unwrap();
            state.nodes.insert(mac, std::time::SystemTime::now());
        }
        
        let (wifi_ssid, wifi_psk) = {
            let storage = nvs_sync.lock().unwrap();
            let known = storage.get_known_networks().unwrap_or_default();
            known.iter()
                .find(|(_, entry)| entry.default.unwrap_or(false))
                .map(|(ssid, entry)| (ssid.clone(), entry.psk.clone()))
                .unwrap_or_default()
        };
        let ntp_server = {
            let storage = nvs_sync.lock().unwrap();
            storage.get_str("ntpServer").ok().flatten().unwrap_or_default()
        };
        let distance = {
            let state = mesh_state_sync.lock().unwrap();
            state.distance
        };
        
        let json = serde_json::json!({
            "wifi_ssid": wifi_ssid,
            "wifi_psk": wifi_psk,
            "ntp_server": ntp_server,
            "distance": distance,
        });
        
        let response_data = serde_json::to_string(&json)?;
        let mut response = req.into_response(200, Some("OK"), &[
            ("Content-Type", "application/json"),
            ("Access-Control-Allow-Origin", "*")
        ])?;
        response.write(response_data.as_bytes())?;
        Ok(())
    })?;

    // GET /api/status
    let nvs_clone = Arc::clone(&nvs_storage);
    let wifi_clone = Arc::clone(&wifi_manager);
    let mesh_state_clone = Arc::clone(&mesh_state);
    server.fn_handler("/api/status", esp_idf_svc::http::Method::Get, move |req| -> Result<(), anyhow::Error> {
        let storage = nvs_clone.lock().unwrap();
        let wifi = wifi_clone.lock().unwrap();
        
        let (m_enabled, m_root, m_distance, m_nodes_count) = {
            let state = mesh_state_clone.lock().unwrap();
            (state.enabled, state.is_root, state.distance, state.nodes.len())
        };
        let (m_channel, m_id, m_pmk) = {
            let channel = storage.get_i32("meshChannel").unwrap_or(Some(11)).unwrap_or(11) as u8;
            let id = storage.get_str("meshId").unwrap_or(Some("Whiper".to_string())).unwrap_or_else(|| "Whiper".to_string());
            let pmk = storage.get_str("meshPmk").unwrap_or(Some("WhiperEyes@Mesh!".to_string())).unwrap_or_else(|| "WhiperEyes@Mesh!".to_string());
            (channel, id, pmk)
        };

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

        let ip_info = if active_mode == "Station" || active_mode == "Mixed" {
            wifi.wifi.wifi().sta_netif().get_ip_info().ok()
        } else {
            wifi.wifi.wifi().ap_netif().get_ip_info().ok()
        };

        let ip_addr = ip_info.map(|i| i.ip.to_string()).unwrap_or_else(|| "0.0.0.0".to_string());
        let gateway = ip_info.map(|i| i.subnet.gateway.to_string()).unwrap_or_else(|| "0.0.0.0".to_string());
        let rssi = if active_mode == "Station" || active_mode == "Mixed" {
            wifi.wifi.wifi().get_ap_info().ok().map(|i| i.signal_strength)
        } else {
            None
        };

        let now_str = get_formatted_time();

        let ntp_server = storage.get_str("ntpServer")?.unwrap_or_default();
        let metrics_url_raw = storage.get_str("metricsUrl")?.unwrap_or_default();
        let metrics_url = if metrics_url_raw == "empty" { "".to_string() } else { metrics_url_raw };
        let last_ota_success = storage.get_str("lastOtaSuccess")?.unwrap_or_default();
        let last_ota_dl = storage.get_str("lastOtaDl")?.unwrap_or_default();
        let last_ota_write = storage.get_str("lastOtaWrite")?.unwrap_or_default();
        let update_url = storage.get_str("updateAvailable")?.unwrap_or_default();
        let update_interval = storage.get_str("updateInterval")?.unwrap_or_else(|| "7j".to_string());
        let wifi_known = storage.get_known_networks().unwrap_or_default();
        let auto_update = storage.get_i32("autoUpdate")?.unwrap_or(1) == 1;
        let rename_enabled = storage.get_i32("renameEnabled")?.unwrap_or(1) == 1;
        let totp_secret = storage.get_str("totpSecret")?.unwrap_or_default();
        let has_totp = !totp_secret.is_empty();
        let partial_totp = if totp_secret.len() >= 12 {
            format!("{}......{}", &totp_secret[0..6], &totp_secret[totp_secret.len()-6..])
        } else if !totp_secret.is_empty() {
            "......".to_string()
        } else {
            "".to_string()
        };
        let ext_name = storage.get_str("extName")?.unwrap_or_else(|| {
            let m = get_mac_address();
            let clean = m.replace(":", "");
            format!("WE-{}", &clean[8..12])
        });
        let ext_desc = storage.get_str("extDesc")?.unwrap_or_else(|| "WhisperEye Extender".to_string());

        let json = serde_json::json!({
            "network_mode": active_mode,
            "wifi_ssid": wifi_ssid,
            "wifi_rssi": rssi,
            "ip_addr": ip_addr,
            "gateway_addr": gateway,
            "sys_time": now_str,
            "ntp_server": ntp_server,
            "metrics_url": metrics_url,
            "fw_version": FW_VERSION,
            "last_ota_success": last_ota_success,
            "last_ota_dl": last_ota_dl,
            "last_ota_write": last_ota_write,
            "update_url": update_url,
            "update_interval": update_interval,
            "whispereye_board": WHISPEREYE_BOARD,
            "board_type": WHISPEREYE_BOARD,
            "chip_type": CHIP_TYPE,
            "wifi_known": wifi_known,
            "auto_update": auto_update,
            "rename_enabled": rename_enabled,
            "has_totp": has_totp,
            "partial_totp": partial_totp,
            "ext_name": ext_name,
            "ext_desc": ext_desc,
            "mesh_enabled": m_enabled,
            "mesh_root": m_root,
            "mesh_distance": m_distance,
            "mesh_nodes_count": m_nodes_count,
            "mesh_channel": m_channel,
            "mesh_id": m_id,
            "mesh_pmk": m_pmk,
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
    // GET /api/capacity
    let cap_nvs = Arc::clone(&nvs_storage);
    let cap_probes = Arc::clone(&discovered_probes);
    let cap_act = Arc::clone(&actuators_state);
    server.fn_handler("/api/capacity", esp_idf_svc::http::Method::Get, move |req| -> Result<(), anyhow::Error> {
        let (rla, rlb, swpwr, ina, inb) = {
            let act = cap_act.lock().unwrap();
            (act.rla, act.rlb, act.swpwr, act.ina, act.inb)
        };
        let touch_state = false;
        
        let readings = sensors::read_sensors(&cap_probes);
        let mut ds_readings = std::collections::HashMap::new();
        for (addr, temp) in &readings.ds18b20_temperatures {
            ds_readings.insert(addr.clone(), *temp);
        }

        let registry = dynamic_devices::DeviceRegistry::new(Arc::clone(&cap_nvs));
        let list = registry.get_devices_display(
            rla,
            rlb,
            swpwr,
            ina,
            inb,
            readings.temperature_sht45,
            readings.humidity_sht45,
            readings.co2_scd41,
            &ds_readings,
            touch_state,
        );

        let mut sensors = Vec::new();
        let mut actuators = Vec::new();

        for dev in &list {
            if !dev.present {
                continue;
            }

            match dev.id.as_str() {
                // Return only: rla, rlb, ina, inb, swpwr
                "rla" | "rlb" | "ina" | "inb" | "swpwr" => {
                    actuators.push(serde_json::json!({
                        "Name": dev.id,
                        "description": dev.name,
                        "Type": "tout ou rien",
                        "range": "bool:0 1",
                    }));
                }
                // Allowed Sensors
                "touch" | "vsense" | "isense" => {
                    let (s_type, unit) = match dev.id.as_str() {
                        "touch" => ("Touch", "-"),
                        "vsense" => ("Voltage", "V"),
                        "isense" => ("Current", "A"),
                        _ => ("Generic", "-"),
                    };
                    sensors.push(serde_json::json!({
                        "Name": dev.id,
                        "description": dev.name,
                        "Type": s_type,
                        "Unit": unit,
                    }));
                }
                id if id.starts_with("onewr:") || id.contains("0x44") => {
                    sensors.push(serde_json::json!({
                        "Name": dev.id,
                        "description": dev.name,
                        "Type": "Temperature",
                        "Unit": "°C",
                    }));
                }
                id if id.contains("0x62") => {
                    sensors.push(serde_json::json!({
                        "Name": dev.id,
                        "description": dev.name,
                        "Type": "CO2",
                        "Unit": "ppm",
                    }));
                }
                _ => {}
            }
        }

        let (mac, name, desc, rename_enabled) = {
            let storage = cap_nvs.lock().unwrap();
            let m = get_mac_address();
            let name = storage.get_str("extName")?.unwrap_or_else(|| {
                let clean = m.replace(":", "");
                format!("WE-{}", &clean[8..12])
            });
            let desc = storage.get_str("extDesc")?.unwrap_or_else(|| "WhisperEye Extender".to_string());
            let rename_enabled = storage.get_i32("renameEnabled")?.unwrap_or(1) == 1;
            (m, name, desc, rename_enabled)
        };

        let cap_json = serde_json::json!({
            "mac": mac,
            "name": name,
            "description": desc,
            "version": FW_VERSION,
            "rename_enabled": rename_enabled,
            "sensors": sensors,
            "actuators": actuators,
        });

        let response_data = serde_json::to_string(&cap_json)?;
        let mut response = req.into_response(200, Some("OK"), &[
            ("Content-Type", "application/json"),
            ("Access-Control-Allow-Origin", "*")
        ])?;
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
            warn!("Aucune URL de mise à jour configurée dans la NVS.");
            let mut response = req.into_status_response(400)?;
            response.write(b"No update URL configured")?;
            return Ok(());
        }

        // Add a random cache-buster to prevent GitHub raw/CDN caching
        let rand_val = unsafe { esp_idf_sys::esp_random() };
        let mut cache_busted_url = update_url.clone();
        if cache_busted_url.contains('?') {
            cache_busted_url.push_str(&format!("&nocache={}", rand_val));
        } else {
            cache_busted_url.push_str(&format!("?nocache={}", rand_val));
        }

        info!("\x1b[35;1m[ÉTAPE 1] Vérification des mises à jour à l'URL : {} (original: {})\x1b[0m", cache_busted_url, update_url);

        info!("Lancement de la requête HTTP GET vers l'URL amont...");
        // Fetch JSON from update_url on ESP32 side to bypass CORS!
        let config = esp_idf_svc::http::client::Configuration {
            buffer_size: Some(2048),
            crt_bundle_attach: Some(esp_idf_sys::esp_crt_bundle_attach),
            ..Default::default()
        };
        let mut connection = esp_idf_svc::http::client::EspHttpConnection::new(&config)?;
        connection.initiate_request(esp_idf_svc::http::Method::Get, &cache_busted_url, &[])?;
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
            let c_type = entry.get("ChipType").and_then(|v| v.as_str()).unwrap_or("");
            if c_type == "ESP32-S3" {
                matched_entry = entry.clone();
                break;
            }
        }

        let response_data = serde_json::to_string(&matched_entry)?;
        let mut response = req.into_response(200, Some("OK"), &[
            ("Content-Type", "application/json"),
            ("Access-Control-Allow-Origin", "*"),
            ("Cache-Control", "no-store, no-cache, must-revalidate, max-age=0")
        ])?;
        response.write(response_data.as_bytes())?;
        Ok(())
    })?;

    // GET /api/history (returns the sliding metrics history from cron scheduler)
    let cron_history_clone = cron_handle.clone();
    server.fn_handler("/api/history", esp_idf_svc::http::Method::Get, move |req| -> Result<(), anyhow::Error> {
        let history = cron_history_clone.get_sensor_history();
        let response_data = serde_json::to_string(&history)?;
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

    // GET /api/sensors
    let sensors_probes_clone = Arc::clone(&discovered_probes);
    server.fn_handler("/api/sensors", esp_idf_svc::http::Method::Get, move |req| -> Result<(), anyhow::Error> {
        let readings = sensors::read_sensors(&sensors_probes_clone);
        let response_data = serde_json::to_string(&readings)?;
        let mut response = req.into_ok_response()?;
        response.write(response_data.as_bytes())?;
        Ok(())
    })?;

    // GET /api/peripherals (lists devices with custom names and current values)
    let periphs_nvs = Arc::clone(&nvs_storage);
    let periphs_act = Arc::clone(&actuators_state);
    let periphs_probes = Arc::clone(&discovered_probes);
    server.fn_handler("/api/peripherals", esp_idf_svc::http::Method::Get, move |req| -> Result<(), anyhow::Error> {
        let (rla, rlb, swpwr, ina, inb) = {
            let act = periphs_act.lock().unwrap();
            (act.rla, act.rlb, act.swpwr, act.ina, act.inb)
        };
        let touch_state = false; // default touch status
        
        let readings = sensors::read_sensors(&periphs_probes);
        let mut ds_readings = std::collections::HashMap::new();
        for (addr, temp) in &readings.ds18b20_temperatures {
            ds_readings.insert(addr.clone(), *temp);
        }

        let registry = dynamic_devices::DeviceRegistry::new(Arc::clone(&periphs_nvs));
        let list = registry.get_devices_display(
            rla,
            rlb,
            swpwr,
            ina,
            inb,
            readings.temperature_sht45,
            readings.humidity_sht45,
            readings.co2_scd41,
            &ds_readings,
            touch_state,
        );

        let response_data = serde_json::to_string(&list)?;
        let mut response = req.into_response(200, Some("OK"), &[
            ("Content-Type", "application/json"),
            ("Access-Control-Allow-Origin", "*")
        ])?;
        response.write(response_data.as_bytes())?;
        Ok(())
    })?;

    // POST /api/peripherals (renames a device in NVS, limited to 64 chars)
    #[derive(serde::Deserialize)]
    struct RenamePayload {
        id: String,
        name: String,
    }
    let rename_nvs = Arc::clone(&nvs_storage);
    server.fn_handler("/api/peripherals", esp_idf_svc::http::Method::Post, move |mut req| -> Result<(), anyhow::Error> {
        let mut buf = vec![0u8; 256];
        let bytes_read = req.read(&mut buf)?;
        let payload: RenamePayload = serde_json::from_slice(&buf[..bytes_read])?;

        let mut registry = dynamic_devices::DeviceRegistry::new(Arc::clone(&rename_nvs));
        if let Err(e) = registry.rename_device(&payload.id, &payload.name) {
            let mut response = req.into_status_response(400)?;
            response.write(format!("Error: {}", e).as_bytes())?;
            return Ok(());
        }

        let mut response = req.into_ok_response()?;
        response.write(b"Rename Successful")?;
        Ok(())
    })?;

    // POST /api/actuators
    let act_clone = Arc::clone(&actuators_state);
    let static_devs_clone = Arc::clone(&static_devs);
    server.fn_handler("/api/actuators", esp_idf_svc::http::Method::Post, move |mut req| -> Result<(), anyhow::Error> {
        let mut buf = vec![0u8; 256];
        let bytes_read = req.read(&mut buf)?;
        let payload: ActuatorsState = serde_json::from_slice(&buf[..bytes_read])?;
        
        info!("Updating actuators state: {:?}", payload);
        {
            let mut state = act_clone.lock().unwrap();
            *state = payload.clone();
        }

        // Apply physical states to static device GPIO pins
        {
            let mut devs = static_devs_clone.lock().unwrap();
            let _ = devs.relay_a.set_level(payload.rla.into());
            let _ = devs.relay_b.set_level(payload.rlb.into());
            let _ = devs.sw_pwr.set_level(payload.swpwr.into());
            let _ = devs.ina.set_level(payload.ina.into());
            let _ = devs.inb.set_level(payload.inb.into());
        }

        let response_data = serde_json::to_string(&payload)?;
        let mut response = req.into_ok_response()?;
        response.write(response_data.as_bytes())?;
        Ok(())
    })?;

    // POST /api/clear-totp
    let nvs_clear_totp = Arc::clone(&nvs_storage);
    server.fn_handler("/api/clear-totp", esp_idf_svc::http::Method::Post, move |mut req| -> Result<(), anyhow::Error> {
        let mut buf = vec![0u8; 256];
        let bytes_read = req.read(&mut buf)?;
        
        #[derive(serde::Deserialize)]
        struct ClearTotpPayload {
            token: String,
        }
        
        let payload: ClearTotpPayload = match serde_json::from_slice(&buf[..bytes_read]) {
            Ok(p) => p,
            Err(_) => {
                let mut response = req.into_status_response(400)?;
                response.write(b"Format JSON invalide")?;
                return Ok(());
            }
        };
        
        let current_secret = {
            let storage = nvs_clear_totp.lock().unwrap();
            storage.get_str("totpSecret")?.unwrap_or_default()
        };
        
        if current_secret.is_empty() {
            info!("clear-totp: NVS has empty totpSecret, responding success");
            let mut response = req.into_ok_response()?;
            response.write(b"OK")?;
            return Ok(());
        }
        
        // Verify TOTP token by direct secret comparison
        if !current_secret.eq_ignore_ascii_case(&payload.token) {
            warn!("Rejected clear-totp: Invalid validation token supplied. Got: '{}'", payload.token);
            let mut response = req.into_status_response(403)?;
            let err_msg = format!("Non autorise : token TOTP incorrect. Token fourni : '{}'.", payload.token);
            response.write(err_msg.as_bytes())?;
            return Ok(());
        }
        
        {
            let mut storage = nvs_clear_totp.lock().unwrap();
            info!("TOTP reset authorized via second-level TOTP: removing totpSecret and metricsUrl");
            storage.remove_key("totpSecret")?;
            storage.remove_key("metricsUrl")?;
        }
        
        let mut response = req.into_ok_response()?;
        response.write(b"OK")?;
        Ok(())
    })?;

    // POST /api/reset
    let nvs_reset = Arc::clone(&nvs_storage);
    server.fn_handler("/api/reset", esp_idf_svc::http::Method::Post, move |mut req| -> Result<(), anyhow::Error> {
        let mut buf = vec![0u8; 128];
        let bytes_read = req.read(&mut buf)?;
        
        #[derive(serde::Deserialize)]
        struct ResetPayload {
            confirm: String,
        }
        
        let payload: ResetPayload = match serde_json::from_slice(&buf[..bytes_read]) {
            Ok(p) => p,
            Err(_) => {
                let mut response = req.into_status_response(400)?;
                response.write(b"Format JSON invalide")?;
                return Ok(());
            }
        };
        
        if payload.confirm != "RESET" {
            let mut response = req.into_status_response(400)?;
            response.write(b"Confirmation de reset incorrecte")?;
            return Ok(());
        }
        
        {
            let mut storage = nvs_reset.lock().unwrap();
            info!("Factory reset triggered: removing TOTP secret and clearing Wi-Fi config");
            storage.remove_key("totpSecret")?;
            storage.set_str("wifiKnown", "{}")?;
        }
        
        let mut response = req.into_ok_response()?;
        response.write(b"OK")?;
        
        let _ = thread::Builder::new()
            .name("reset_restart_worker".to_string())
            .stack_size(4096)
            .spawn(|| {
                thread::sleep(std::time::Duration::from_secs(2));
                // Do not boot to recovery, reboot normally to production instead
                unsafe {
                    esp_idf_sys::esp_restart();
                }
            });
            
        Ok(())
    })?;

    // POST /api/config (triggers immediate restart to recovery_boot if update_url differs)
    let nvs_clone = Arc::clone(&nvs_storage);
    let wifi_clone = Arc::clone(&wifi_manager);
    server.fn_handler("/api/config", esp_idf_svc::http::Method::Post, move |mut req| -> Result<(), anyhow::Error> {
        let mut buf = vec![0u8; 512];
        let bytes_read = req.read(&mut buf)?;
        let payload: ConfigPayload = match serde_json::from_slice(&buf[..bytes_read]) {
            Ok(p) => p,
            Err(e) => {
                let err_msg = format!("Format JSON invalide : {:?}", e);
                let mut response = req.into_status_response(400)?;
                response.write(err_msg.as_bytes())?;
                return Ok(());
            }
        };

        // apply_only=true : sauvegarde sans redémarrer (utilisé par le toggle seul)
        let apply_only = payload.apply_only.unwrap_or(false);
        let mut wifi_success = true;
        let mut totp_success = true;
        let mut totp_err_msg = "";
        let mut should_reboot_production = false;

        {
            let mut storage = nvs_clone.lock().unwrap();
            if let Some(ref totp_secret) = payload.totp_secret {
                let current = storage.get_str("totpSecret")?.unwrap_or_default();
                if current.is_empty() {
                    let masked = if totp_secret.len() > 12 {
                        format!("{}...{}", &totp_secret[..6], &totp_secret[totp_secret.len() - 6..])
                    } else {
                        totp_secret.clone()
                    };
                    info!("Saving totpSecret to NVS (was empty): {}", masked);
                    storage.set_str("totpSecret", totp_secret)?;
                } else {
                    let current_supplied = payload.current_totp_secret.as_deref().unwrap_or("");
                    if current_supplied == current || *totp_secret == current {
                        let masked = if totp_secret.len() > 12 {
                            format!("{}...{}", &totp_secret[..6], &totp_secret[totp_secret.len() - 6..])
                        } else {
                            totp_secret.clone()
                        };
                        info!("Updating/confirming totpSecret in NVS: {}", masked);
                        storage.set_str("totpSecret", totp_secret)?;
                    } else {
                        warn!("Rejected totpSecret update: NVS has existing secret and correct current secret not supplied.");
                        totp_success = false;
                        totp_err_msg = "Non autorisé : le secret TOTP actuel est incorrect ou manquant.";
                    }
                }
            }
            if totp_success {
                if let Some(ref ext_name) = payload.ext_name {
                    let trimmed = ext_name.trim();
                    let final_name = if trimmed.len() > 16 {
                        &trimmed[..16]
                    } else {
                        trimmed
                    };
                    info!("Saving extName to NVS: {}", final_name);
                    storage.set_str("extName", final_name)?;
                }
                if let Some(ref ext_desc) = payload.ext_desc {
                    let trimmed = ext_desc.trim();
                    info!("Saving extDesc to NVS: {}", trimmed);
                    storage.set_str("extDesc", trimmed)?;
                }
                if let Some(ref ntp_server) = payload.ntp_server {
                    let trimmed = ntp_server.trim();
                    info!("Saving ntpServer to NVS: {}", trimmed);
                    storage.set_str("ntpServer", trimmed)?;
                }
                if let Some(ref metrics_url) = payload.metrics_url {
                    let trimmed = metrics_url.trim();
                    let formatted = if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
                         format!("http://{}", trimmed)
                    } else {
                         trimmed.to_string()
                    };
                    info!("Saving metricsUrl to NVS: {}", formatted);
                    storage.set_str("metricsUrl", &formatted)?;
                }
                if let Some(rename_en) = payload.rename_enabled {
                    let new_val = if rename_en { 1 } else { 0 };
                    info!("Saving renameEnabled to NVS: {}", new_val);
                    storage.set_i32("renameEnabled", new_val)?;
                }
                if let Some(mesh_en) = payload.mesh_enabled {
                    let new_val = if mesh_en { 1 } else { 0 };
                    let current_val = storage.get_i32("meshEnabled")?.unwrap_or(1);
                    if new_val != current_val {
                        info!("Saving meshEnabled to NVS: {} (apply_only={})", new_val, apply_only);
                        storage.set_i32("meshEnabled", new_val)?;
                        // Redémarrage uniquement si apply_only=false (bouton complet)
                        if !apply_only {
                            should_reboot_production = true;
                        }
                    }
                }
                if let Some(mesh_chan) = payload.mesh_channel {
                    let current_chan = storage.get_i32("meshChannel")?.unwrap_or(11);
                    if mesh_chan != current_chan {
                        info!("Saving meshChannel to NVS: {}", mesh_chan);
                        storage.set_i32("meshChannel", mesh_chan)?;
                        if !apply_only {
                            should_reboot_production = true;
                        }
                    }
                }
                if let Some(ref mesh_id) = payload.mesh_id {
                    let current_id = storage.get_str("meshId")?.unwrap_or_else(|| "Whiper".to_string());
                    if mesh_id != &current_id {
                        info!("Saving meshId to NVS: {}", mesh_id);
                        storage.set_str("meshId", mesh_id)?;
                        if !apply_only {
                            should_reboot_production = true;
                        }
                    }
                }
                if let Some(ref mesh_pmk) = payload.mesh_pmk {
                    let current_pmk = storage.get_str("meshPmk")?.unwrap_or_else(|| "WhiperEyes@Mesh!".to_string());
                    if mesh_pmk != &current_pmk {
                        info!("Saving meshPmk to NVS");
                        storage.set_str("meshPmk", mesh_pmk)?;
                        if !apply_only {
                            should_reboot_production = true;
                        }
                    }
                }
            }
        }

        if !totp_success {
            let mut response = req.into_status_response(403)?;
            response.write(totp_err_msg.as_bytes())?;
            return Ok(());
        }

        if let Some(ref ssid_raw) = payload.wifi_ssid {
            let ssid = ssid_raw.trim();
            if !ssid.is_empty() {
                wifi_success = false;
                let psk = payload.wifi_psk.as_deref().unwrap_or("").trim();
                let mut final_psk = "".to_string();
                let mut wifi = wifi_clone.lock().unwrap();
                let mut storage = nvs_clone.lock().unwrap();
                
                // Mémoriser la configuration Wi-Fi cliente précédente
                let prev_client_config = match wifi.wifi.get_configuration() {
                    Ok(esp_idf_svc::wifi::Configuration::Client(client_cfg)) => {
                        Some((client_cfg.ssid.as_str().to_string(), client_cfg.password.as_str().to_string()))
                    }
                    Ok(esp_idf_svc::wifi::Configuration::Mixed(client_cfg, _)) => {
                        Some((client_cfg.ssid.as_str().to_string(), client_cfg.password.as_str().to_string()))
                    }
                    _ => None,
                };
                
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
                    common::led::set_led_color(0, 25, 0); // Vert car connecté
                } else {
                    warn!("Wi-Fi connection to '{}' failed. Reverting to previous configuration...", ssid);
                    let mut reconnected = false;
                    let mut chosen_ssid = String::new();

                    if let Some((ref prev_ssid, ref prev_psk)) = prev_client_config {
                        info!("Trying to reconnect to previous network: {}", prev_ssid);
                        if wifi.start_sta(prev_ssid, prev_psk).unwrap_or(false) {
                            reconnected = true;
                            chosen_ssid = prev_ssid.clone();
                            info!("Successfully reverted to previous Wi-Fi network '{}'", prev_ssid);
                        } else {
                            warn!("Revert to previous Wi-Fi network '{}' failed.", prev_ssid);
                        }
                    }

                    if !reconnected {
                        info!("Revert failed. Trying known networks from NVS...");
                        let known_networks = storage.get_known_networks().unwrap_or_default();

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
                    }

                    if !reconnected {
                        warn!("All connection attempts failed. Restarting Access Point...");
                        let _ = wifi.start_ap();
                        common::led::set_led_color(25, 25, 0); // Jaune car mode AP
                    } else {
                        info!("Reconnected to network '{}'.", chosen_ssid);
                        let _ = storage.set_default_network_by_ssid(&chosen_ssid);
                        common::led::set_led_color(0, 25, 0); // Vert car connecté
                    }
                }
            }
        }
        
        if !wifi_success {
            let mut response = req.into_status_response(400)?;
            response.write(b"Echec de la connexion Wi-Fi : SSID introuvable ou mot de passe incorrect.")?;
            return Ok(());
        }

        let should_restart = {
            let mut storage = nvs_clone.lock().unwrap();
            
            if let Some(ref update_url) = payload.update_url {
                if !update_url.is_empty() {
                    info!("\x1b[35;1m[ÉTAPE 2] Choix d'installation / mise à jour\x1b[0m");
                    info!("\x1b[36;1m  -> URL cible : {}\x1b[0m", update_url);
                    if let Some(filename) = update_url.split('/').last() {
                        info!("\x1b[36;1m  -> Fichier binaire : {}\x1b[0m", filename);
                    }
                }
                let is_bin = update_url.ends_with(".bin");
                if is_bin {
                    storage.set_str("updateDlUrl", update_url)?;
                    storage.set_i32("otaRetry", 3)?;
                } else {
                    storage.set_str("updateAvailable", update_url)?;
                }
                storage.set_str("updateInterval", "7j")?;
            }
            
            if let Some(auto_up) = payload.auto_update {
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
            
            // Restart condition: different or new update URL is set, and NOT apply_only
            if apply_only {
                false
            } else if let Some(ref update_url) = payload.update_url {
                if update_url.is_empty() {
                    false
                } else {
                    let current_url = storage.get_str("updateAvailable")?.unwrap_or_default();
                    let is_bin = update_url.ends_with(".bin");
                    is_bin || update_url != &current_url
                }
            } else {
                false
            }
        };

        if should_restart {
            info!("\x1b[35;1m[ÉTAPE 3] Redémarrage sur Recovery\x1b[0m");
            info!("\x1b[36;1m  -> Firmware actif lors de la demande : production_app ({})\x1b[0m", FW_VERSION);
            info!("\x1b[36;1m  -> Redémarrage programmé sur la partition de secours dans 2 secondes...\x1b[0m");
            let _ = thread::Builder::new()
                .name("restart_worker".to_string())
                .stack_size(4096)
                .spawn(|| {
                    thread::sleep(std::time::Duration::from_secs(2));
                    set_boot_to_recovery();
                    unsafe {
                        esp_idf_sys::esp_restart();
                    }
                });
        } else if should_reboot_production {
            info!("Configuration du Mesh modifiée. Redémarrage sur Production dans 2 secondes...");
            let _ = thread::Builder::new()
                .name("production_restart_worker".to_string())
                .stack_size(4096)
                .spawn(|| {
                    thread::sleep(std::time::Duration::from_secs(2));
                    unsafe {
                        esp_idf_sys::esp_restart();
                    }
                });
        } else {
            info!("Configuration updated. No OTA URL modification, running in place.");
        }

        let (mac, name, desc) = {
            let storage = nvs_clone.lock().unwrap();
            let m = get_mac_address();
            let name = storage.get_str("extName")?.unwrap_or_else(|| {
                let clean = m.replace(":", "");
                format!("WE-{}", &clean[8..12])
            });
            let desc = storage.get_str("extDesc")?.unwrap_or_else(|| "WhisperEye Extender".to_string());
            (m, name, desc)
        };
        let response_json = serde_json::json!({
            "status": "OK",
            "mac": mac,
            "name": name,
            "description": desc
        });
        let response_data = serde_json::to_string(&response_json)?;
        let mut response = req.into_response(200, Some("OK"), &[
            ("Content-Type", "application/json"),
            ("Access-Control-Allow-Origin", "*")
        ])?;
        response.write(response_data.as_bytes())?;
        Ok(())
    })?;

    // Prevent main thread from exiting
    loop {
        thread::sleep(std::time::Duration::from_secs(60));
    }
}

#[allow(dead_code)]
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
#[allow(dead_code)]
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
fn is_web_accessible() -> bool {
    use std::net::ToSocketAddrs;
    
    // Hôtes de test légers
    let probes = [
        "probe.lpz.ovh:80",
        "google.com:80",
        "github.com:80",
    ];

    info!("Checking web accessibility before update check...");
    for probe in &probes {
        // Tente une résolution DNS et vérification d'adresse de manière légère
        if let Ok(mut addrs) = probe.to_socket_addrs() {
            if addrs.next().is_some() {
                info!("Web accessibility check passed using probe: {}", probe);
                return true;
            }
        }
    }
    warn!("Web accessibility check failed. No connection to update servers.");
    false
}

fn get_formatted_time() -> String {
    let now = SystemTime::now();
    let duration = now.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
    let total_secs = duration.as_secs();

    if total_secs < 86400 {
        // NTP pas encore synchronisé : retourner epoch pour que l'UI signale "non synchronisé"
        return "1970-01-01T00:00:00Z".to_string();
    }

    // Calcul de l'heure UTC
    let secs   = total_secs % 60;
    let mins   = (total_secs / 60) % 60;
    let hours  = (total_secs / 3600) % 24;

    // Calcul de la date (algorithme civil de Howard Hinnant)
    let days = (total_secs / 86400) as i64; // jours depuis 1970-01-01
    let z  = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y   = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp  = (5 * doy + 2) / 153;
    let d   = doy - (153 * mp + 2) / 5 + 1;
    let m   = if mp < 10 { mp + 3 } else { mp - 9 };
    let y   = if m <= 2 { y + 1 } else { y };

    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, m, d, hours, mins, secs)
}

pub fn set_boot_to_recovery() {
    unsafe {
        let partition = esp_idf_sys::esp_partition_find_first(
            esp_idf_sys::esp_partition_type_t_ESP_PARTITION_TYPE_APP,
            esp_idf_sys::esp_partition_subtype_t_ESP_PARTITION_SUBTYPE_APP_FACTORY,
            std::ptr::null(),
        );
        if !partition.is_null() {
            let err = esp_idf_sys::esp_ota_set_boot_partition(partition);
            if err != 0 {
                log::error!("Failed to set boot partition to factory/recovery: {}", err);
            } else {
                log::info!("Successfully set boot partition to factory/recovery!");
            }
        } else {
            log::error!("Factory/recovery partition not found!");
        }
    }
}

















































