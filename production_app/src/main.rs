#![recursion_limit = "256"]

use esp_idf_sys as _; // Mandatory for linking ESP-IDF
use esp_idf_hal::peripherals::Peripherals;
use esp_idf_hal::gpio::{PinDriver, Pull};
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
const FW_VERSION: &str = "1.0.55-0002";
#[allow(dead_code)]
const TOTP_SECRET: &str = "Salt-4-Hash-Between-Probe-&-WhisperEye";

macro_rules! extend_pairing {
    ($mesh_state:expr) => {
        {
            let mut state = $mesh_state.lock().unwrap();
            if state.pairing_until.is_some() {
                state.pairing_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(120));
                info!("Pairing mode extended by 120 seconds.");
            }
        }
    };
}

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

use wifi::{NetManager, NetState};
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

fn get_ext_name(nvs: &Arc<Mutex<NvsStorage>>) -> String {
    let storage = nvs.lock().unwrap();
    storage.get_str("extName").ok().flatten().unwrap_or_default()
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

pub struct MeshState {
    pub is_root: bool,
    pub distance: i32,
    pub nodes: std::collections::HashMap<String, std::time::SystemTime>,
    pub ip_addresses: std::collections::HashMap<String, String>,
    pub node_names: std::collections::HashMap<String, String>,
    pub pairing_until: Option<std::time::Instant>,
}

pub(crate) fn perform_mesh_sync(nvs: &Arc<Mutex<NvsStorage>>, gateway_ip: std::net::Ipv4Addr) -> Result<(i32, bool)> {
    let my_ip = unsafe {
        let netif = esp_idf_sys::esp_netif_get_handle_from_ifkey(b"WIFI_STA_DEF\0".as_ptr() as *const _);
        if !netif.is_null() {
            let mut ip_info = esp_idf_sys::esp_netif_ip_info_t::default();
            if esp_idf_sys::esp_netif_get_ip_info(netif, &mut ip_info) == 0 {
                // esp_ip4_addr_t.addr stocke le 1er octet dans le LSB (convention ESP-IDF/lwIP)
                let ip = ip_info.ip.addr;
                format!("{}.{}.{}.{}", ip & 0xff, (ip >> 8) & 0xff, (ip >> 16) & 0xff, (ip >> 24) & 0xff)
            } else {
                "0.0.0.0".to_string()
            }
        } else {
            "0.0.0.0".to_string()
        }
    };

    info!("Syncing Wi-Fi credentials from provisioning peer at http://{}/api/mesh/sync?mac={}&ip={} (My IP: {})", gateway_ip, get_mac_address(), my_ip, my_ip);
    // Court délai de stabilisation réseau après connexion Wi‑Fi
    thread::sleep(std::time::Duration::from_millis(500));

    let ext_name = get_ext_name(nvs);
    let mut last_err = anyhow::anyhow!("no attempt");
    for attempt in 0..3 {
        if attempt > 0 {
            info!("Mesh sync retry {}/3...", attempt + 1);
            thread::sleep(std::time::Duration::from_millis(1500));
        }
        match try_mesh_sync_request(gateway_ip, my_ip.as_str(), &ext_name) {
            Ok(res) => {
                // Valider avant d'enregistrer : si le credential est différent, comparer les dates.
                let should_save_wifi = if !res.wifi_ssid.is_empty() {
                    let (known_psk, my_last_seen) = {
                        let storage = nvs.lock().unwrap();
                        let known = storage.get_known_networks().unwrap_or_default();
                        known.get(&res.wifi_ssid)
                            .map(|e| (e.psk.clone(), e.last_seen.unwrap_or(0)))
                            .unwrap_or_default()
                    };
                    if known_psk.is_empty() {
                        // Nouveau réseau : toujours enregistrer
                            info!("Sync: New wifi '{}' discovered, saving", res.wifi_ssid);
                        true
                    } else if known_psk != res.wifi_psk {
                        // Credential différent : comparer les last_seen
                        let parent_last_seen = res.last_seen.unwrap_or(0);
                        if parent_last_seen > my_last_seen {
                            info!("Sync: Wifi '{}' PSK differs + peer last_seen={} > local={}, updating",
                                res.wifi_ssid, parent_last_seen, my_last_seen);
                            true
                        } else {
                            info!("Sync: Wifi '{}' PSK differs but peer last_seen={} <= local={}, keeping local",
                                res.wifi_ssid, parent_last_seen, my_last_seen);
                            false
                        }
                    } else {
                        info!("Sync: Wifi '{}' PSK identique → ignoré", res.wifi_ssid);
                        false
                    }
                } else {
                    false
                };
                // Save to NVS
                {
                    let mut storage = nvs.lock().unwrap();
                    if should_save_wifi {
                        let known = storage.get_known_networks().unwrap_or_default();
                        if !known.contains_key(&res.wifi_ssid) {
                            info!("Sync: Saving wifi SSID '{}' to NVS (newly discovered)", res.wifi_ssid);
                            storage.set_default_network(&res.wifi_ssid, &res.wifi_psk)?;
                        } else {
                            info!("Sync: Wifi SSID '{}' is already known, updating PSK", res.wifi_ssid);
                            storage.set_default_network(&res.wifi_ssid, &res.wifi_psk)?;
                        }
                    }
                    if !res.ntp_server.is_empty() {
                        info!("Sync: Saving ntpServer '{}' to NVS", res.ntp_server);
                        storage.set_str("ntpServer", &res.ntp_server)?;
                    }
                }
                return Ok((res.distance, should_save_wifi));
            }
            Err(e) => {
                warn!("Mesh sync attempt {} failed: {:?}", attempt + 1, e);
                last_err = e;
            }
        }
    }
    Err(last_err)
}

fn try_mesh_sync_request(gateway_ip: std::net::Ipv4Addr, my_ip: &str, my_name: &str) -> Result<SyncResponse> {
    let config = esp_idf_svc::http::client::Configuration {
        buffer_size: Some(1024),
        crt_bundle_attach: None,
        ..Default::default()
    };
    let mut connection = esp_idf_svc::http::client::EspHttpConnection::new(&config)?;
    let encoded_name = percent_encode(my_name);
    let url = format!("http://{}/api/mesh/sync?mac={}&ip={}&name={}", gateway_ip, get_mac_address(), my_ip, encoded_name);
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
    
    let res: SyncResponse = serde_json::from_slice(&body)?;
    Ok(res)
}

#[derive(serde::Deserialize)]
struct SyncResponse {
    wifi_ssid: String,
    wifi_psk: String,
    ntp_server: String,
    distance: i32,
    #[serde(default)]
    last_seen: Option<u32>,
}



#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum UrlValidationState {
    NotChecked,
    Checking,
    Checked(bool),
}

fn check_updates_internal(update_url: &str) -> Result<serde_json::Value, anyhow::Error> {
    if update_url.is_empty() {
        return Err(anyhow::anyhow!("URL vide"));
    }

    // Add a random cache-buster to prevent GitHub raw/CDN caching
    let rand_val = unsafe { esp_idf_sys::esp_random() };
    let mut cache_busted_url = update_url.to_string();
    if cache_busted_url.contains('?') {
        cache_busted_url.push_str(&format!("&nocache={}", rand_val));
    } else {
        cache_busted_url.push_str(&format!("?nocache={}", rand_val));
    }

    info!("[check_updates_internal] Querying URL: {}", cache_busted_url);

    let config = esp_idf_svc::http::client::Configuration {
        buffer_size: Some(2048),
        crt_bundle_attach: Some(esp_idf_sys::esp_crt_bundle_attach),
        ..Default::default()
    };
    let mut connection = esp_idf_svc::http::client::EspHttpConnection::new(&config)?;
    connection.initiate_request(esp_idf_svc::http::Method::Get, &cache_busted_url, &[])?;
    connection.initiate_response()?;

    let status = connection.status();
    if status != 200 {
        return Err(anyhow::anyhow!("Upstream error: HTTP {}", status));
    }

    let mut body = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        match connection.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => body.extend_from_slice(&chunk[..n]),
            Err(e) => return Err(anyhow::anyhow!("Read error: {:?}", e)),
        }
    }

    let list: serde_json::Value = serde_json::from_slice(&body)?;
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
            return Ok(entry);
        }
    }

    Err(anyhow::anyhow!("Aucun firmware compatible (ESP32-S3) trouvé"))
}

fn main() -> Result<()> {
    log::set_logger(&LOGGER)
        .map(|()| log::set_max_level(log::LevelFilter::Info))
        .expect("Failed to initialize custom logger");
    
    // Désactiver le Task Watchdog Timer — les scans Wi-Fi matériels
    // de l'ESP32-S3 utilisent ets_delay_us qui bloque le CPU et empêche
    // IDLE1 de nourrir le watchdog, provoquant des resets intempestifs.
    unsafe {
        esp_idf_sys::esp_task_wdt_deinit();
    }
    info!("Task Watchdog Timer désactivé.");
    
    let peripherals = Peripherals::take().context("Failed to take ESP32 Peripherals")?;

    // Initialiser la LED RMT sur GPIO 48 (canal 0) avant tout appel à set_led_color
    #[allow(deprecated)]
    {
        common::led::init_led(peripherals.rmt.channel0, peripherals.pins.gpio48)
            .context("Failed to init RMT LED driver")?;
    }

    // La LED est gérée par le thread pattern (led.rs) via set_sta_status/set_ap_status

    info!("\x1b[35mWhisperEye Production Application Starting Up (Version {})...\x1b[0m", FW_VERSION);

    let sys_loop = EspSystemEventLoop::take().context("Failed to take System Event Loop")?;

    // Log SoftAP client connection attempts (station connected/disconnected)
    let _wifi_event_sub = sys_loop.subscribe::<esp_idf_svc::wifi::WifiEvent, _>(|event| {
        match event {
            esp_idf_svc::wifi::WifiEvent::ApStaConnected(info) => {
                let mac = info.mac();
                info!("[WIFI AP] Station connectée -> MAC: {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}, AID: {}",
                    mac[0], mac[1], mac[2], mac[3], mac[4], mac[5], info.aid());
                wifi::AP_CLIENT_CONNECTED.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            esp_idf_svc::wifi::WifiEvent::ApStaDisconnected(info) => {
                let mac = info.mac();
                info!("[WIFI AP] Station déconnectée -> MAC: {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}, AID: {}",
                    mac[0], mac[1], mac[2], mac[3], mac[4], mac[5], info.aid());
            }
            _ => {}
        }
    })?;
    let nvs_default = EspDefaultNvsPartition::take().context("Failed to take NVS Partition")?;

    // Initialize NVS Storage helper
    let nvs_storage = Arc::new(Mutex::new(NvsStorage::new(nvs_default.clone())?));
    
    // Shared repository URL validation state
    let url_validation_state = Arc::new(Mutex::new(UrlValidationState::NotChecked));
    
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
    // (gpio48 a déjà été consommé par init_led, on extrait les pins individuellement)
    let boot_pin_gpio = peripherals.pins.gpio0;
    let modem = peripherals.modem;

    // Initialize Static Devices from pins
    let static_devs = Arc::new(std::sync::Mutex::new(static_devices::StaticDevices::init(
        peripherals.pins.gpio9,
        peripherals.pins.gpio47,
        peripherals.pins.gpio21,
        peripherals.pins.gpio14,
        peripherals.pins.gpio36,
        peripherals.pins.gpio35,
    )?));


    // Scan 1-Wire bus dynamically at boot
    let onewr_pin = peripherals.pins.gpio39;
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

    // Read provisioning channel from NVS. The old mesh fields are kept for NVS/UI compatibility.
    let (mesh_channel, mesh_ssid, mesh_pmk) = {
        let storage = nvs_storage.lock().unwrap();
        let channel = storage.get_i32("wifiChannel")?.unwrap_or(11) as u8;
        let ssid = storage.get_str("meshSsid")?.unwrap_or_default();
        let ssid = if ssid.is_empty() { "Esp32MeshNetwork".to_string() } else { ssid };
        let pmk = storage.get_str("meshPmk")?.unwrap_or_default();
        let pmk = if pmk.is_empty() { "Mesh-IoT@Espressif!".to_string() } else { pmk };
        info!("Provisioning config: channel={}, legacy_ssid='{}', legacy_pmk='{}...{}'",
            channel, ssid, &pmk[..std::cmp::min(2, pmk.len())], &pmk[std::cmp::max(pmk.len().saturating_sub(2), 0)..]);
        (channel, ssid, pmk)
    };

    // Initialize Wi-Fi (consuming only the modem, leaving other pins untouched)
    let wifi_manager = NetManager::new(modem, sys_loop.clone(), nvs_default, mesh_ssid, mesh_pmk, mesh_channel)?;
    let wifi_manager = Arc::new(Mutex::new(wifi_manager));

    let mesh_state = Arc::new(Mutex::new(MeshState {
        is_root: false,
        distance: -1,
        nodes: std::collections::HashMap::new(),
        ip_addresses: std::collections::HashMap::new(),
        node_names: std::collections::HashMap::new(),
        pairing_until: None,
    }));

    // Monitor physical BOOT button (GPIO0) to trigger pairing mode
    let boot_pin = PinDriver::input(boot_pin_gpio, Pull::Up)?;

    let wifi_manager_boot = Arc::clone(&wifi_manager);
    let mesh_state_boot = Arc::clone(&mesh_state);

    thread::Builder::new()
        .name("boot_button_worker".to_string())
        .stack_size(4096)
        .spawn(move || {
            let mut pressed_ticks = 0;
            loop {
                if boot_pin.is_low() {
                    pressed_ticks += 1;
                    if pressed_ticks == 20 { // 20 * 100ms = 2 seconds
                        info!("BOOT button held for 2 seconds! Triggering pairing mode.");
                        {
                            let mut net = wifi_manager_boot.lock().unwrap();
                            net.state = NetState::ApPairing;
                            net.pairing_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(120));
                            if let Err(e) = net.setup_provisioning_ap() {
                                warn!("Failed to apply provisioning mode from BOOT button: {:?}", e);
                            }
                            let mut state = mesh_state_boot.lock().unwrap();
                            state.pairing_until = net.pairing_until;
                        }
                    }
                } else {
                    pressed_ticks = 0;
                }
                thread::sleep(std::time::Duration::from_millis(100));
            }
        })?;
    
    {
        let mut net = wifi_manager.lock().unwrap();
        net.state = NetState::WifiPreferred;
        net.last_state_change = std::time::Instant::now();
    }

    // Initialize SNTP client unconditionally
    let _sntp = {
        let ntp_server = {
            let storage = nvs_storage.lock().unwrap();
            storage.get_str("ntpServer").ok().flatten().unwrap_or_default()
        };

        let use_custom = is_valid_fqdn(&ntp_server);
        let sntp = if use_custom {
            info!("Initializing SNTP with custom server: {} (fallback pool.ntp.org)", ntp_server);
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
            info!("Initializing SNTP with default pool (ntpServer='{}' invalide ou absent)", ntp_server);
            EspSntp::new_default()
        };

        if sntp.is_err() {
            warn!("Failed to initialize SNTP service");
        }
        sntp.ok()
    };

    // Démarrer le thread de connectivité réseau
    NetManager::start_controller_thread(
        Arc::clone(&wifi_manager),
        Arc::clone(&nvs_storage),
        Arc::clone(&mesh_state),
    )?;

    // Spawn robust periodic task scheduler
    let cron_handle = cron::spawn_cron_scheduler(
        Arc::clone(&nvs_storage),
        Arc::clone(&wifi_manager),
        Arc::clone(&mesh_state),
    )
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

    // GET /proxy?mac=<MAC_ADRESSE>
    let proxy_mesh = Arc::clone(&mesh_state);
    server.fn_handler("/proxy", esp_idf_svc::http::Method::Get, move |req| -> Result<(), anyhow::Error> {
        let uri = req.uri();
        let mac = if let Some(pos) = uri.find("mac=") {
            let raw_mac = &uri[pos + 4..];
            raw_mac.split('&').next().unwrap_or("").to_string().to_uppercase()
        } else {
            String::new()
        };
        let mac = mac.replace("%3A", ":").replace("%3a", ":");

        let target_ip = {
            let state = proxy_mesh.lock().unwrap();
            state.ip_addresses.get(&mac).cloned()
        };

        let target_ip = match target_ip {
            Some(ip) if ip != "0.0.0.0" => ip,
            _ => {
                let mut response = req.into_status_response(404)?;
                response.write(format!("Noeud enfant avec la MAC '{}' introuvable", mac).as_bytes())?;
                return Ok(());
            }
        };

        info!("Proxying request for child MAC {} to http://{}/", mac, target_ip);
        
        let config = esp_idf_svc::http::client::Configuration {
            buffer_size: Some(512),
            crt_bundle_attach: None,
            ..Default::default()
        };
        
        let mut connection = esp_idf_svc::http::client::EspHttpConnection::new(&config)?;
        let target_url = format!("http://{}/", target_ip);
        connection.initiate_request(esp_idf_svc::http::Method::Get, &target_url, &[])?;
        connection.initiate_response()?;

        let status = connection.status();
        // Streamer directement la réponse chunk par chunk (pas de bufferisation)
        let mut response = req.into_response(status, Some("OK"), &[
            ("Access-Control-Allow-Origin", "*"),
        ])?;
        let mut chunk = [0u8; 512];
        loop {
            match connection.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    response.write(&chunk[..n])?;
                }
                Err(e) => anyhow::bail!("Failed to read proxy response: {:?}", e),
            }
        }
        
        Ok(())
    })?;

    // Captive Portal HTTP Redirects for Mobile Auto-Popup (iOS, Android, Windows)
    server.fn_handler("/generate_204", esp_idf_svc::http::Method::Get, |req| -> Result<(), anyhow::Error> {
        let subnet = wifi::AP_IP_B.load(std::sync::atomic::Ordering::Relaxed);
        let location = format!("http://192.168.{}.1/", subnet);
        let mut response = req.into_response(302, Some("Found"), &[("Location", &location)])?;
        response.write(b"Redirecting to captive portal...")?;
        Ok(())
    })?;

    server.fn_handler("/hotspot-detect.html", esp_idf_svc::http::Method::Get, |req| -> Result<(), anyhow::Error> {
        let subnet = wifi::AP_IP_B.load(std::sync::atomic::Ordering::Relaxed);
        let location = format!("http://192.168.{}.1/", subnet);
        let mut response = req.into_response(302, Some("Found"), &[("Location", &location)])?;
        response.write(b"Redirecting to captive portal...")?;
        Ok(())
    })?;

    server.fn_handler("/ncsi.txt", esp_idf_svc::http::Method::Get, |req| -> Result<(), anyhow::Error> {
        let subnet = wifi::AP_IP_B.load(std::sync::atomic::Ordering::Relaxed);
        let location = format!("http://192.168.{}.1/", subnet);
        let mut response = req.into_response(302, Some("Found"), &[("Location", &location)])?;
        response.write(b"Redirecting to captive portal...")?;
        Ok(())
    })?;

    server.fn_handler("/connecttest.txt", esp_idf_svc::http::Method::Get, |req| -> Result<(), anyhow::Error> {
        let subnet = wifi::AP_IP_B.load(std::sync::atomic::Ordering::Relaxed);
        let location = format!("http://192.168.{}.1/", subnet);
        let mut response = req.into_response(302, Some("Found"), &[("Location", &location)])?;
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
        
        let ip = if let Some(pos) = uri.find("ip=") {
            let raw_ip = &uri[pos + 3..];
            raw_ip.split('&').next().unwrap_or("0.0.0.0").to_string()
        } else {
            "0.0.0.0".to_string()
        };
        
        let name = if let Some(pos) = uri.find("name=") {
            let raw_name = &uri[pos + 5..];
            let decoded = raw_name.split('&').next().unwrap_or("").to_string();
            percent_decode(&decoded)
        } else {
            String::new()
        };
        
        info!("Received Mesh sync request from MAC: {} (IP: {}, Name: '{}')", mac, ip, name);
        
        {
            let mut state = mesh_state_sync.lock().unwrap();
            if mac != "unknown" {
                state.nodes.insert(mac.clone(), std::time::SystemTime::now());
                state.ip_addresses.insert(mac.clone(), ip);
                if !name.is_empty() {
                    state.node_names.insert(mac, name);
                }
            }
            if state.pairing_until.is_some() {
                state.pairing_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(10));
                info!("Mesh sync called during pairing mode. Reducing remaining pairing time to 10 seconds.");
            }
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
        let last_seen = {
            let storage = nvs_sync.lock().unwrap();
            storage.get_default_network_last_seen().unwrap_or(None)
        };
        
        let json = serde_json::json!({
            "wifi_ssid": wifi_ssid,
            "wifi_psk": wifi_psk,
            "ntp_server": ntp_server,
            "distance": distance,
            "last_seen": last_seen,
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
    let url_state_clone = Arc::clone(&url_validation_state);
    server.fn_handler("/api/status", esp_idf_svc::http::Method::Get, move |req| -> Result<(), anyhow::Error> {
        extend_pairing!(mesh_state_clone);
        let storage = nvs_clone.lock().unwrap();
        let wifi = wifi_clone.lock().unwrap();
        
        let (m_root, m_distance, m_nodes_count, pairing_seconds_remaining, m_children) = {
            let state = mesh_state_clone.lock().unwrap();
            let rem = if let Some(until) = state.pairing_until {
                let now = std::time::Instant::now();
                if now < until {
                    (until - now).as_secs() as i64
                } else {
                    0
                }
            } else {
                0
            };
            // Expiration : 120s sans sync → nœud considéré déconnecté
            let expire_after = std::time::Duration::from_secs(120);
            let now = std::time::SystemTime::now();
            let active_nodes: Vec<(String, std::time::SystemTime)> = state.nodes.iter()
                .filter(|(_, t)| now.duration_since(**t).unwrap_or_default() < expire_after)
                .map(|(mac, t)| (mac.clone(), *t))
                .collect();
            let active_ips: Vec<serde_json::Value> = active_nodes.iter()
                .filter_map(|(mac, _)| {
                    let ip = state.ip_addresses.get(mac);
                    let name = state.node_names.get(mac);
                    ip.map(|ip| {
                        let mut obj = serde_json::json!({"mac": mac, "ip": ip});
                        if let Some(n) = name {
                            obj["name"] = serde_json::json!(n);
                        }
                        obj
                    })
                })
                .collect();
            (state.is_root, state.distance, active_nodes.len(), rem, active_ips)
        };
        let (m_channel, m_id, m_pmk, m_ssid) = {
            let channel = storage.get_i32("wifiChannel")?.unwrap_or(11) as u8;
            let id = storage.get_str("meshId")?.unwrap_or_default();
            let pmk = storage.get_str("meshPmk")?.unwrap_or_default();
            let ssid = storage.get_str("meshSsid")?.unwrap_or_default();
            (channel, id, pmk, ssid)
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

        let sta_ip_info = wifi.wifi.wifi().sta_netif().get_ip_info().ok();
        let ap_ip_info = wifi.wifi.wifi().ap_netif().get_ip_info().ok();

        // 1. Wi-Fi (Box Connection)
        let (wifi_ip, wifi_gateway, wifi_cidr) = if m_root {
            if let Some(info) = sta_ip_info {
                (info.ip.to_string(), info.subnet.gateway.to_string(), info.subnet.mask.0)
            } else {
                ("0.0.0.0".to_string(), "0.0.0.0".to_string(), 0)
            }
        } else {
            ("".to_string(), "".to_string(), 0)
        };

        // 2. Mesh Connection
        let mesh_ip = if let Some(info) = sta_ip_info {
            info.ip.to_string()
        } else {
            "0.0.0.0".to_string()
        };

        let mesh_ap_ip = if let Some(info) = ap_ip_info {
            format!("{}/24", info.ip)
        } else {
            let subnet = wifi::AP_IP_B.load(std::sync::atomic::Ordering::Relaxed);
            format!("192.168.{}.1/24", subnet)
        };

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
        let update_url_valid_val = if update_url.is_empty() {
            serde_json::Value::Null
        } else {
            let mut state_lock = url_state_clone.lock().unwrap();
            match *state_lock {
                UrlValidationState::NotChecked => {
                    *state_lock = UrlValidationState::Checking;
                    let thread_state_clone = Arc::clone(&url_state_clone);
                    let url_to_check = update_url.clone();
                    let _ = thread::Builder::new()
                        .name("url_check_worker".to_string())
                        .stack_size(8192)
                        .spawn(move || {
                            info!("Démarrage de la vérification de l'URL du dépôt en arrière-plan : {}", url_to_check);
                            let is_ok = check_updates_internal(&url_to_check).is_ok();
                            let mut lock = thread_state_clone.lock().unwrap();
                            *lock = UrlValidationState::Checked(is_ok);
                            info!("Fin de la vérification en arrière-plan. Résultat = {}", is_ok);
                        });
                    serde_json::Value::Null
                }
                UrlValidationState::Checking => serde_json::Value::Null,
                UrlValidationState::Checked(valid) => serde_json::Value::Bool(valid),
            }
        };

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
            "wifi_ip": wifi_ip,
            "wifi_gateway": wifi_gateway,
            "wifi_cidr": wifi_cidr,
            "mesh_ip": mesh_ip,
            "mesh_ap_ip": mesh_ap_ip,
            "sys_time": now_str,
            "ntp_server": ntp_server,
            "metrics_url": metrics_url,
            "fw_version": FW_VERSION,
            "last_ota_success": last_ota_success,
            "last_ota_dl": last_ota_dl,
            "last_ota_write": last_ota_write,
            "update_url": update_url,
            "update_url_valid": update_url_valid_val,
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
            "mesh_enabled": true,
            "mesh_root": m_root,
            "mesh_distance": m_distance,
            "mesh_nodes_count": m_nodes_count,
            "mesh_children": m_children,
            "mesh_channel": m_channel,
            "mesh_id": m_id,
            "mesh_pmk": m_pmk,
            "mesh_ssid": m_ssid,
            "pairing_seconds_remaining": pairing_seconds_remaining,
            "identify_remaining_secs": common::led::identify_remaining_secs(),
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
    let cap_mesh = Arc::clone(&mesh_state);
    server.fn_handler("/api/capacity", esp_idf_svc::http::Method::Get, move |req| -> Result<(), anyhow::Error> {
        extend_pairing!(cap_mesh);
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
                    let meta = dynamic_devices::get_sensor_meta(dev.id.as_str());
                    let corr = dynamic_devices::get_correction_formula(&cap_nvs, dev.id.as_str());
                    let mut sensor_json = serde_json::json!({
                        "Name": dev.id,
                        "description": dev.name,
                        "Type": s_type,
                        "Unit": unit,
                        "correction_formula": corr,
                    });
                    if let Some(ref m) = meta {
                        sensor_json["uncertainty"] = serde_json::json!(m.uncertainty);
                        sensor_json["range_min"] = serde_json::json!(m.range_min);
                        sensor_json["range_max"] = serde_json::json!(m.range_max);
                    }
                    sensors.push(sensor_json);
                }
                id if id.starts_with("onewr:") => {
                    let meta = dynamic_devices::get_sensor_meta(id);
                    let corr = dynamic_devices::get_correction_formula(&cap_nvs, id);
                    let mut sensor_json = serde_json::json!({
                        "Name": dev.id,
                        "description": dev.name,
                        "Type": "Temperature",
                        "Unit": "°C",
                        "correction_formula": corr,
                    });
                    if let Some(ref m) = meta {
                        sensor_json["uncertainty"] = serde_json::json!(m.uncertainty);
                        sensor_json["range_min"] = serde_json::json!(m.range_min);
                        sensor_json["range_max"] = serde_json::json!(m.range_max);
                    }
                    sensors.push(sensor_json);
                }
                id if id.ends_with("_T") && id.contains("0x44") => {
                    let meta = dynamic_devices::get_sensor_meta(id);
                    let corr = dynamic_devices::get_correction_formula(&cap_nvs, id);
                    let mut sensor_json = serde_json::json!({
                        "Name": dev.id,
                        "description": dev.name,
                        "Type": "Temperature",
                        "Unit": "°C",
                        "correction_formula": corr,
                    });
                    if let Some(ref m) = meta {
                        sensor_json["uncertainty"] = serde_json::json!(m.uncertainty);
                        sensor_json["range_min"] = serde_json::json!(m.range_min);
                        sensor_json["range_max"] = serde_json::json!(m.range_max);
                    }
                    sensors.push(sensor_json);
                }
                id if id.ends_with("_H") && id.contains("0x44") => {
                    let meta = dynamic_devices::get_sensor_meta(id);
                    let corr = dynamic_devices::get_correction_formula(&cap_nvs, id);
                    let mut sensor_json = serde_json::json!({
                        "Name": dev.id,
                        "description": dev.name,
                        "Type": "Humidite",
                        "Unit": "%RH",
                        "correction_formula": corr,
                    });
                    if let Some(ref m) = meta {
                        sensor_json["uncertainty"] = serde_json::json!(m.uncertainty);
                        sensor_json["range_min"] = serde_json::json!(m.range_min);
                        sensor_json["range_max"] = serde_json::json!(m.range_max);
                    }
                    sensors.push(sensor_json);
                }
                id if id.contains("0x62") && !id.ends_with("_T") && !id.ends_with("_H") && !id.ends_with("_P") => {
                    let meta = dynamic_devices::get_sensor_meta(id);
                    let corr = dynamic_devices::get_correction_formula(&cap_nvs, id);
                    let mut sensor_json = serde_json::json!({
                        "Name": dev.id,
                        "description": dev.name,
                        "Type": "CO2",
                        "Unit": "ppm",
                        "correction_formula": corr,
                    });
                    if let Some(ref m) = meta {
                        sensor_json["uncertainty"] = serde_json::json!(m.uncertainty);
                        sensor_json["range_min"] = serde_json::json!(m.range_min);
                        sensor_json["range_max"] = serde_json::json!(m.range_max);
                    }
                    sensors.push(sensor_json);
                }
                // BME280 (futur)
                id if id.ends_with("_T") && (id.contains("0x76") || id.contains("0x77")) => {
                    let meta = dynamic_devices::get_sensor_meta(id);
                    let corr = dynamic_devices::get_correction_formula(&cap_nvs, id);
                    let mut sensor_json = serde_json::json!({
                        "Name": dev.id,
                        "description": dev.name,
                        "Type": "Temperature",
                        "Unit": "°C",
                        "correction_formula": corr,
                    });
                    if let Some(ref m) = meta {
                        sensor_json["uncertainty"] = serde_json::json!(m.uncertainty);
                        sensor_json["range_min"] = serde_json::json!(m.range_min);
                        sensor_json["range_max"] = serde_json::json!(m.range_max);
                    }
                    sensors.push(sensor_json);
                }
                id if id.ends_with("_H") && (id.contains("0x76") || id.contains("0x77")) => {
                    let meta = dynamic_devices::get_sensor_meta(id);
                    let corr = dynamic_devices::get_correction_formula(&cap_nvs, id);
                    let mut sensor_json = serde_json::json!({
                        "Name": dev.id,
                        "description": dev.name,
                        "Type": "Humidite",
                        "Unit": "%RH",
                        "correction_formula": corr,
                    });
                    if let Some(ref m) = meta {
                        sensor_json["uncertainty"] = serde_json::json!(m.uncertainty);
                        sensor_json["range_min"] = serde_json::json!(m.range_min);
                        sensor_json["range_max"] = serde_json::json!(m.range_max);
                    }
                    sensors.push(sensor_json);
                }
                id if id.ends_with("_P") && (id.contains("0x76") || id.contains("0x77")) => {
                    let meta = dynamic_devices::get_sensor_meta(id);
                    let corr = dynamic_devices::get_correction_formula(&cap_nvs, id);
                    let mut sensor_json = serde_json::json!({
                        "Name": dev.id,
                        "description": dev.name,
                        "Type": "Pression",
                        "Unit": "hPa",
                        "correction_formula": corr,
                    });
                    if let Some(ref m) = meta {
                        sensor_json["uncertainty"] = serde_json::json!(m.uncertainty);
                        sensor_json["range_min"] = serde_json::json!(m.range_min);
                        sensor_json["range_max"] = serde_json::json!(m.range_max);
                    }
                    sensors.push(sensor_json);
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
    let updates_mesh = Arc::clone(&mesh_state);
    let updates_url_state = Arc::clone(&url_validation_state);
    server.fn_handler("/api/check_updates", esp_idf_svc::http::Method::Get, move |req| -> Result<(), anyhow::Error> {
        extend_pairing!(updates_mesh);
        let uri = req.uri();
        let query_url = if let Some(pos) = uri.find("url=") {
            let raw_url = &uri[pos + 4..];
            let percent_encoded = raw_url.split('&').next().unwrap_or("");
            percent_decode(percent_encoded)
        } else {
            "".to_string()
        };

        let is_nvs_url = query_url.is_empty();
        let update_url = if !is_nvs_url {
            query_url
        } else {
            let storage = nvs_updates_clone.lock().unwrap();
            storage.get_str("updateAvailable")?.unwrap_or_default()
        };

        if update_url.is_empty() {
            warn!("Aucune URL de mise à jour configurée dans la NVS.");
            let mut response = req.into_status_response(400)?;
            response.write(b"No update URL configured")?;
            return Ok(());
        }

        let matched_entry = match check_updates_internal(&update_url) {
            Ok(entry) => {
                if is_nvs_url {
                    let mut state_lock = updates_url_state.lock().unwrap();
                    *state_lock = UrlValidationState::Checked(true);
                }
                entry
            }
            Err(e) => {
                if is_nvs_url {
                    let mut state_lock = updates_url_state.lock().unwrap();
                    *state_lock = UrlValidationState::Checked(false);
                }
                let mut response = req.into_status_response(502)?;
                response.write(format!("Error: {:?}", e).as_bytes())?;
                return Ok(());
            }
        };

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
    let history_mesh = Arc::clone(&mesh_state);
    server.fn_handler("/api/history", esp_idf_svc::http::Method::Get, move |req| -> Result<(), anyhow::Error> {
        extend_pairing!(history_mesh);
        let history = cron_history_clone.get_sensor_history();
        let response_data = serde_json::to_string(&history)?;
        let mut response = req.into_response(200, Some("OK"), &[
            ("Content-Type", "application/json"),
            ("Access-Control-Allow-Origin", "*")
        ])?;
        response.write(response_data.as_bytes())?;
        Ok(())
    })?;

    // GET /api/ssids (Active hardware Wi-Fi scan via active_scan_ssid)
    let wifi_scan_clone = Arc::clone(&wifi_manager);
    let nvs_ssids_clone = Arc::clone(&nvs_storage);
    let ssids_mesh = Arc::clone(&mesh_state);
    server.fn_handler("/api/ssids", esp_idf_svc::http::Method::Get, move |req| -> Result<(), anyhow::Error> {
        extend_pairing!(ssids_mesh);
        let mut wifi = wifi_scan_clone.lock().unwrap();
        // active_scan_ssid fait un scan général (ssid=None) et remplit scan_cache
        let scan_ok = wifi.active_scan_ssid(""); // SSID bidon, le scan est général
        let ssids = if scan_ok {
            wifi.scan_cache.clone()
        } else {
            // Fallback au cache précédent
            let mut fallback = wifi.scan_cache.clone();
            if fallback.is_empty() {
                fallback = vec!["IoT".to_string(), "Maison_WiFi".to_string(), "WhisperEye-Mesh".to_string(), "Freebox-Private".to_string()];
            }
            fallback
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
    let sensors_mesh = Arc::clone(&mesh_state);
    server.fn_handler("/api/sensors", esp_idf_svc::http::Method::Get, move |req| -> Result<(), anyhow::Error> {
        extend_pairing!(sensors_mesh);
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
    let periphs_mesh = Arc::clone(&mesh_state);
    server.fn_handler("/api/peripherals", esp_idf_svc::http::Method::Get, move |req| -> Result<(), anyhow::Error> {
        extend_pairing!(periphs_mesh);
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
    let rename_mesh = Arc::clone(&mesh_state);
    server.fn_handler("/api/peripherals", esp_idf_svc::http::Method::Post, move |mut req| -> Result<(), anyhow::Error> {
        extend_pairing!(rename_mesh);
        let mut buf = vec![0u8; 256];
        let bytes_read = req.read(&mut buf)?;
        let payload: RenamePayload = serde_json::from_slice(&buf[..bytes_read])?;

        if !is_valid_name(&payload.name, 24) {
            let mut response = req.into_status_response(400)?;
            response.write(b"Nom de peripherique invalide. Il doit faire 24 caracteres max, sans espaces, ni ' ou ` ou :.")?;
            return Ok(());
        }

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

    // POST /api/sensor-correction — enregistre la formule de correction d'un capteur dans la NVS
    // La formule utilise les noms des capteurs avec l'extension .raw pour la valeur brute.
    // Ex: a * i2c:0:0x44_T.raw + b  ou  a * i2c:0:0x44_T.raw + i2c:0:0x44_H.raw/100
    let corr_nvs = Arc::clone(&nvs_storage);
    let corr_mesh = Arc::clone(&mesh_state);
    server.fn_handler("/api/sensor-correction", esp_idf_svc::http::Method::Post, move |mut req| -> Result<(), anyhow::Error> {
        extend_pairing!(corr_mesh);
        let mut buf = vec![0u8; 512];
        let bytes_read = req.read(&mut buf)?;
        #[derive(serde::Deserialize)]
        struct CorrPayload {
            id: String,
            formula: String,
        }
        let payload: CorrPayload = serde_json::from_slice(&buf[..bytes_read])?;
        let formula = payload.formula.trim();
        if formula.is_empty() {
            let mut response = req.into_status_response(400)?;
            response.write(b"La formule ne peut pas etre vide")?;
            return Ok(());
        }
        // Validation : la formule accepte les noms de capteurs avec .raw, chiffres, opérateurs
        let valid_chars = |c: char| c.is_alphanumeric() || "+-*/^(). _:%xabcdefABCDEF".contains(c);
        if !formula.chars().all(valid_chars) {
            let mut response = req.into_status_response(400)?;
            response.write(b"Formule invalide : caracteres non autorises (utilisez a-z, 0-9, +-*/^().raw, :)")?;
            return Ok(());
        }
        // Vérifier que les identifiants référencés sont des capteurs valides (terminent par .raw)
        for word in formula.split(|c: char| !c.is_alphanumeric() && c != '.' && c != ':' && c != '_') {
            let trimmed = word.trim();
            if trimmed.is_empty() { continue; }
            if trimmed.ends_with(".raw") {
                let sensor_id = &trimmed[..trimmed.len()-4];
                // Vérifier que le capteur existe dans le registre
                let registry = dynamic_devices::DeviceRegistry::new(Arc::clone(&corr_nvs));
                let devs = registry.get_devices();
                if !devs.contains_key(sensor_id) && sensor_id != "x" {
                    warn!("Formule de correction référence un capteur inconnu: '{}'", sensor_id);
                    // On accepte quand même (le capteur pourrait être ajouté plus tard)
                }
            }
        }
        dynamic_devices::set_correction_formula(&corr_nvs, &payload.id, formula)?;
        info!("Correction formula set for '{}': '{}'", payload.id, formula);
        let mut response = req.into_ok_response()?;
        response.write(b"OK")?;
        Ok(())
    })?;

    // POST /api/actuators
    let act_clone = Arc::clone(&actuators_state);
    let static_devs_clone = Arc::clone(&static_devs);
    let actuators_mesh = Arc::clone(&mesh_state);
    server.fn_handler("/api/actuators", esp_idf_svc::http::Method::Post, move |mut req| -> Result<(), anyhow::Error> {
        extend_pairing!(actuators_mesh);
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

    // POST /api/identify — déclenche le clignotement blanc rapide de la LED pendant 15s (cumulatif)
    let identify_mesh = Arc::clone(&mesh_state);
    server.fn_handler("/api/identify", esp_idf_svc::http::Method::Post, move |req| -> Result<(), anyhow::Error> {
        extend_pairing!(identify_mesh);
        let stop_utc = common::led::extend_identify(15);
        let remaining = common::led::identify_remaining_secs();
        let stop_str = format!("{:?}", stop_utc);
        info!("Identify triggered, LED will blink white until UTC: {} ({}s remaining)", stop_str, remaining);
        let json = serde_json::json!({
            "status": "ok",
            "identify_stop_utc": stop_str,
            "identify_remaining_secs": remaining,
        });
        let response_data = serde_json::to_string(&json)?;
        let mut response = req.into_ok_response()?;
        response.write(response_data.as_bytes())?;
        Ok(())
    })?;

    // POST /api/restart — redémarre l'ESP32 sur la partition production, sans délai
    let restart_mesh = Arc::clone(&mesh_state);
    server.fn_handler("/api/restart", esp_idf_svc::http::Method::Post, move |req| -> Result<(), anyhow::Error> {
        extend_pairing!(restart_mesh);
        info!("Redémarrage demandé via API. Redémarrage immédiat...");
        let json = serde_json::json!({"status": "ok", "message": "Redémarrage immédiat..."});
        let response_data = serde_json::to_string(&json)?;
        let mut response = req.into_ok_response()?;
        response.write(response_data.as_bytes())?;
        unsafe {
            esp_idf_sys::esp_restart();
        }
        #[allow(unreachable_code)]
        Ok(())
    })?;

    // POST /api/clear-totp
    let nvs_clear_totp = Arc::clone(&nvs_storage);
    let clear_totp_mesh = Arc::clone(&mesh_state);
    server.fn_handler("/api/clear-totp", esp_idf_svc::http::Method::Post, move |mut req| -> Result<(), anyhow::Error> {
        extend_pairing!(clear_totp_mesh);
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
    let reset_mesh = Arc::clone(&mesh_state);
    server.fn_handler("/api/reset", esp_idf_svc::http::Method::Post, move |mut req| -> Result<(), anyhow::Error> {
        extend_pairing!(reset_mesh);
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

    // POST /api/mesh/pair (legacy URL): enable provisioning captive portal for 120 seconds.
    let wifi_pair = Arc::clone(&wifi_manager);
    let mesh_state_pair = Arc::clone(&mesh_state);
    server.fn_handler("/api/mesh/pair", esp_idf_svc::http::Method::Post, move |req| -> Result<(), anyhow::Error> {
        info!("Enabling provisioning pairing mode for 120 seconds...");
        {
            let mut net = wifi_pair.lock().unwrap();
            net.state = NetState::ApPairing;
            net.pairing_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(120));
            if let Err(e) = net.setup_provisioning_ap() {
                warn!("Failed to apply provisioning pairing mode: {:?}", e);
            }
            let mut state = mesh_state_pair.lock().unwrap();
            state.pairing_until = net.pairing_until;
        }
        
        let mut response = req.into_ok_response()?;
        response.write(b"Provisioning mode enabled for 120 seconds")?;
        Ok(())
    })?;

    // POST /api/config (triggers immediate restart to recovery_boot if update_url differs)
    let nvs_clone = Arc::clone(&nvs_storage);
    let wifi_clone = Arc::clone(&wifi_manager);
    let config_mesh = Arc::clone(&mesh_state);
    let config_url_state = Arc::clone(&url_validation_state);
    server.fn_handler("/api/config", esp_idf_svc::http::Method::Post, move |mut req| -> Result<(), anyhow::Error> {
        extend_pairing!(config_mesh);
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
                    if !is_valid_name(trimmed, 16) {
                        let mut response = req.into_status_response(400)?;
                        response.write(b"Nom de l'extendeur invalide. Il doit faire 16 caracteres max, sans espaces, ni ' ou ` ou :.")?;
                        return Ok(());
                    }
                    info!("Saving extName to NVS: {}", trimmed);
                    storage.set_str("extName", trimmed)?;
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

                // mesh_channel is no longer a user choice, aligned automatically on active wifiChannel.
                if let Some(ref mesh_id) = payload.mesh_id {
                    let current_id = storage.get_str("meshId")?.unwrap_or_default();
                    if mesh_id != &current_id {
                        info!("Saving meshId to NVS: {}", mesh_id);
                        storage.set_str("meshId", mesh_id)?;
                        if !apply_only {
                            should_reboot_production = true;
                        }
                    }
                }
                if let Some(ref mesh_pmk) = payload.mesh_pmk {
                    let current_pmk = storage.get_str("meshPmk")?.unwrap_or_default();
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
                let mut final_psk = psk.to_string();
                let mut wifi = wifi_clone.lock().unwrap();
                let mut storage = nvs_clone.lock().unwrap();
                
                if psk.is_empty() {
                    let known_networks = storage.get_known_networks().unwrap_or_default();
                    if let Some(entry) = known_networks.get(ssid) {
                        final_psk = entry.psk.clone();
                    }
                }

                info!("\x1b[35;1mConnexion directe au SSID '{}' (sans scan préalable)...\x1b[0m", ssid);
                if wifi.try_sta_connect(ssid, &final_psk, false, 0).unwrap_or(false) {
                    wifi_success = true;
                }
                
                if wifi_success {
                    info!("Connexion réussie au SSID '{}'. Sauvegarde dans le NVS...", ssid);
                    storage.set_default_network(ssid, &final_psk)?;
                    let _ = storage.update_wifi_last_seen(ssid);
                    wifi.state = NetState::WifiOk;
                    wifi.retry_count = 0;
                    wifi.backoff_delay = std::time::Duration::from_secs(2);
                    let _ = wifi.stop_provisioning_ap_if_not_pairing();
                } else {
                    warn!("Échec de la connexion Wi-Fi à '{}'.", ssid);
                    wifi.state = NetState::ProvisioningScan;
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
                let current_url = storage.get_str("updateAvailable")?.unwrap_or_default();
                if update_url != &current_url {
                    let mut state_lock = config_url_state.lock().unwrap();
                    *state_lock = UrlValidationState::NotChecked;
                }
                
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

fn is_valid_name(name: &str, max_len: usize) -> bool {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.len() > max_len {
        return false;
    }
    for c in trimmed.chars() {
        if c == ' ' || c == '\'' || c == '`' || c == ':' {
            return false;
        }
    }
    true
}

fn is_valid_fqdn(name: &str) -> bool {
    if name.is_empty() || name == "default" || name == "empty" {
        return false;
    }
    if name.len() > 253 {
        return false;
    }
    // Doit contenir au moins un point (host.domain)
    if !name.contains('.') {
        return false;
    }
    // Caractères autorisés : alphanumériques, tirets, points
    for c in name.chars() {
        if !c.is_ascii_alphanumeric() && c != '-' && c != '.' {
            return false;
        }
    }
    // Chaque label doit faire 1-63 caractères
    for label in name.split('.') {
        if label.is_empty() || label.len() > 63 {
            return false;
        }
        // Un label ne doit pas commencer ni finir par un tiret
        if label.starts_with('-') || label.ends_with('-') {
            return false;
        }
    }
    true
}

fn percent_decode(s: &str) -> String {
    let mut decoded = String::new();
    let mut chars = s.chars();
    while let Some(ch) = chars.next() {
        if ch == '%' {
            let h1 = chars.next();
            let h2 = chars.next();
            if let (Some(c1), Some(c2)) = (h1, h2) {
                if let Ok(val) = u8::from_str_radix(&format!("{}{}", c1, c2), 16) {
                    decoded.push(val as char);
                    continue;
                }
            }
        }
        decoded.push(ch);
    }
    decoded
}

fn percent_encode(s: &str) -> String {
    let mut encoded = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(b as char);
            }
            _ => {
                encoded.push_str(&format!("%{:02X}", b));
            }
        }
    }
    encoded
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






























































