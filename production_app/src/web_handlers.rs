use std::sync::{Arc, Mutex};
use std::sync::atomic::Ordering;
use std::time::SystemTime;
use std::thread;
use log::{info, warn};
use anyhow::Result;
use esp_idf_svc::http::server::{Request, EspHttpConnection};
use crate::wifi::{self, NetManager, NetState};
use crate::dynamic_devices;
use common::nvs_storage::NvsStorage;
use crate::actuators::{Actuators, ActuatorsState, ScheduledActions};
use crate::board::Board;
use crate::i2c::I2c;
use crate::UrlValidationState;
use crate::ConfigPayload;
use crate::FW_VERSION;
use crate::WHISPEREYE_BOARD;
use crate::CHIP_TYPE;
use crate::AUTHOR_EMAIL;
use crate::AUTHOR_NAME;
use crate::AUTHOR_LINK;

pub fn handle_favicon(req: Request<&mut EspHttpConnection<'_>>) -> Result<(), anyhow::Error> {
    let mut response = req.into_response(200, Some("OK"), &[("Content-Type", "image/x-icon")])?;
    response.write(include_bytes!("../../common/src/favicon.ico"))?;
    Ok(())
}

// --- Fonctions Utilitaires ---

pub fn get_mac_address() -> String {
    let mut mac = [0u8; 6];
    unsafe {
        esp_idf_sys::esp_read_mac(mac.as_mut_ptr(), esp_idf_sys::esp_mac_type_t_ESP_MAC_WIFI_STA);
    }
    format!("{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}", mac[0], mac[1], mac[2], mac[3], mac[4], mac[5])
}

pub fn get_formatted_time() -> String {
    let now = SystemTime::now();
    let duration = now.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
    let total_secs = duration.as_secs();

    if total_secs < 86400 {
        return "1970-01-01T00:00:00Z".to_string();
    }

    let secs   = total_secs % 60;
    let mins   = (total_secs / 60) % 60;
    let hours  = (total_secs / 3600) % 24;

    let days = (total_secs / 86400) as i64;
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

pub fn percent_decode(s: &str) -> String {
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

pub fn is_valid_name(name: &str, max_len: usize) -> bool {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.len() > max_len {
        return false;
    }
    for c in trimmed.chars() {
        if c == '\'' || c == '`' || c == ':' {
            return false;
        }
    }
    true
}

pub fn is_valid_peripheral_name(name: &str, max_len: usize) -> bool {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.len() > max_len {
        return false;
    }
    for c in trimmed.chars() {
        if c == '\'' || c == '`' || c == ':' {
            return false;
        }
    }
    true
}

pub fn is_valid_fqdn(name: &str) -> bool {
    if name.is_empty() || name == "default" || name == "empty" {
        return false;
    }
    if name.len() > 253 {
        return false;
    }
    if !name.contains('.') {
        return false;
    }
    for c in name.chars() {
        if !c.is_ascii_alphanumeric() && c != '-' && c != '.' {
            return false;
        }
    }
    for label in name.split('.') {
        if label.is_empty() || label.len() > 63 {
            return false;
        }
        if label.starts_with('-') || label.ends_with('-') {
            return false;
        }
    }
    true
}

lazy_static::lazy_static! {
    static ref UPDATE_CACHE: std::sync::Mutex<Option<(serde_json::Value, std::time::Instant)>> = std::sync::Mutex::new(None);
    static ref HTTP_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());
    static ref LAST_HTTPS_ATTEMPT: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
}

pub fn check_updates_internal(update_url: &str) -> Result<serde_json::Value, anyhow::Error> {
    if update_url.is_empty() {
        return Err(anyhow::anyhow!("URL vide"));
    }

    // 1. Check if we have a valid cache (2 minutes = 120 seconds)
    {
        let cache = UPDATE_CACHE.lock().unwrap();
        if let Some((val, instant)) = &*cache {
            if instant.elapsed() < std::time::Duration::from_secs(120) {
                info!("[check_updates_internal] Cache hit! Returning cached versions.");
                return Ok(val.clone());
            }
        }
    }

    // 1.5. Rate-limiting : max 1 tentative de requête réseau toutes les 30 secondes
    {
        let mut last_attempt = LAST_HTTPS_ATTEMPT.lock().unwrap();
        if let Some(instant) = *last_attempt {
            if instant.elapsed() < std::time::Duration::from_secs(30) {
                return Err(anyhow::anyhow!("Requête HTTPS trop fréquente, attente requise"));
            }
        }
        *last_attempt = Some(std::time::Instant::now());
    }

    // Acquisition du verrou exclusif TLS/Scan pour éviter les Out Of Memory
    use std::sync::atomic::Ordering;
    if crate::wifi::TLS_OR_SCAN_ACTIVE.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        info!("[check_updates_internal] HTTPS update check postponed: Wi-Fi scan or another TLS operation is active");
        return Err(anyhow::anyhow!("Système occupé (scan en cours)"));
    }

    struct TlsReleaseGuard;
    impl Drop for TlsReleaseGuard {
        fn drop(&mut self) {
            crate::wifi::TLS_OR_SCAN_ACTIVE.store(false, Ordering::SeqCst);
        }
    }
    let _tls_guard = TlsReleaseGuard;

    // 2. Lock HTTP_MUTEX to serialize HTTPS requests and avoid out-of-memory crashes
    let _http_guard = HTTP_MUTEX.lock().unwrap();

    // 3. Re-check cache in case another thread updated it while we were waiting for the lock
    {
        let cache = UPDATE_CACHE.lock().unwrap();
        if let Some((val, instant)) = &*cache {
            if instant.elapsed() < std::time::Duration::from_secs(120) {
                return Ok(val.clone());
            }
        }
    }

    let rand_val = unsafe { esp_idf_sys::esp_random() };
    let mut cache_busted_url = update_url.to_string();
    if cache_busted_url.contains('?') {
        cache_busted_url.push_str(&format!("&nocache={}", rand_val));
    } else {
        cache_busted_url.push_str(&format!("?nocache={}", rand_val));
    }

    // `free_heap` (type: usize) : Quantité de mémoire heap RAM libre en octets avant l'appel HTTPS.
    let free_heap: usize = unsafe { esp_idf_sys::heap_caps_get_free_size(esp_idf_sys::MALLOC_CAP_8BIT) };
    // `total_heap` (type: usize) : Quantité totale de mémoire heap RAM disponible sur la puce.
    let total_heap: usize = unsafe { esp_idf_sys::heap_caps_get_total_size(esp_idf_sys::MALLOC_CAP_8BIT) };
    // `pct` (type: usize) : Pourcentage de heap libre (0..100).
    let pct: usize = if total_heap > 0 { free_heap * 100 / total_heap } else { 0 };
    // `filled` (type: usize) : Nombre de blocs remplis dans la barre (sur 20).
    let filled: usize = pct * 20 / 100;
    // `bar` (type: String) : Barre de progression ASCII (20 caractères : █ remplis, ░ vides).
    let bar: String = format!("{}{}", "█".repeat(filled), "░".repeat(20 - filled));
    info!("\x1b[36m[HEAP] {} {}% ({} Ko / {} Ko)\x1b[0m", bar, pct, free_heap / 1024, total_heap / 1024);

    // `config` (type: esp_idf_svc::http::client::Configuration) : Configuration optimisée du client HTTP pour limiter l'utilisation RAM de mbedTLS.
    let config = esp_idf_svc::http::client::Configuration {
        buffer_size: Some(1024),
        use_global_ca_store: false,
        // [Junior Dev Note] : Désactiver le bundle de certificats crt_bundle_attach permet d'économiser
        // environ 25 à 30 Ko de mémoire RAM sur le tas (heap) de l'ESP32, ce qui évite les plantages OOM.
        crt_bundle_attach: None,
        follow_redirects_policy: esp_idf_svc::http::client::FollowRedirectsPolicy::FollowNone,
        ..Default::default()
    };
    
    // `current_url` (type: String) : URL active pour la requête HTTP (évolue en cas de redirection).
    let mut current_url = cache_busted_url;
    // `body` (type: Vec<u8>) : Tampon de stockage pour accumuler le corps de la réponse reçue.
    let mut body = Vec::new();
    // `redirect_count` (type: i32) : Compteur de redirections suivies manuellement.
    let mut redirect_count = 0;
    
    loop {
        if redirect_count >= 3 {
            return Err(anyhow::anyhow!("Trop de redirections HTTP (max 3)"));
        }
        
        info!("[check_updates_internal] Requête vers : {}", current_url);
        
        // [Junior Dev Note] : Petite pause de 500ms pour laisser le tas de mémoire se stabiliser après une association Wi-Fi
        std::thread::sleep(std::time::Duration::from_millis(500));
        
        // `connection` (type: esp_idf_svc::http::client::EspHttpConnection) : Instance de connexion HTTP client.
        let mut connection = esp_idf_svc::http::client::EspHttpConnection::new(&config)?;
        
        let headers = [
            ("Cache-Control", "no-cache"),
            ("Pragma", "no-cache"),
            ("User-Agent", "WhisperEye-ESP32S3"),
        ];
        connection.initiate_request(esp_idf_svc::http::Method::Get, &current_url, &headers)?;
        connection.initiate_response()?;

        let status = connection.status();
        
        // Si redirection (3xx)
        if status == 301 || status == 302 || status == 303 || status == 307 || status == 308 {
            let mut location_url = None;
            if let Some(loc) = connection.header("Location") {
                location_url = Some(loc.to_string());
            }
            
            // Fermer explicitement la connexion active et libérer la heap RAM mbedTLS
            drop(connection);
            
            if let Some(new_url) = location_url {
                info!("[check_updates_internal] Redirection ({}) vers {}", status, new_url);
                current_url = new_url;
                redirect_count += 1;
                continue;
            } else {
                return Err(anyhow::anyhow!("Redirection sans en-tête Location"));
            }
        }

        if status != 200 {
            return Err(anyhow::anyhow!("Upstream error: HTTP {}", status));
        }

        let mut chunk = [0u8; 1024];
        loop {
            match connection.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => body.extend_from_slice(&chunk[..n]),
                Err(e) => return Err(anyhow::anyhow!("Failed to read response: {:?}", e)),
            }
        }
        break; // Lecture réussie
    }

    let val: serde_json::Value = serde_json::from_slice(&body)?;
    info!("[check_updates_internal] JSON reçu de GitHub : {}", serde_json::to_string(&val).unwrap_or_default());

    // 4. Update the cache
    {
        let mut cache = UPDATE_CACHE.lock().unwrap();
        *cache = Some((val.clone(), std::time::Instant::now()));
    }

    Ok(val)
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

pub fn perform_factory_reset(storage: &mut NvsStorage) -> Result<(), anyhow::Error> {
    info!("Factory reset triggered: removing TOTP secret, clearing Wi-Fi config, metricsUrl, autoUpdate, and deviceRenamable");
    let _ = storage.remove_key("totpSecret");
    let _ = storage.set_str("wifiKnown", "{}");
    let _ = storage.remove_key("metricsUrl");
    let _ = storage.set_i32("autoUpdate", 0);
    let _ = storage.set_i32("deviceRenamable", 0);
    Ok(())
}

// --- Logique de capacité (build_capacity_info) ---

pub fn build_capacity_info(
    cap_nvs: &Arc<Mutex<NvsStorage>>,
    cap_probes: &Arc<Vec<String>>,
    cap_act_state: &Arc<Mutex<ActuatorsState>>,
    _cap_act: &Arc<Mutex<Actuators>>,
    cap_sched: &Arc<Mutex<ScheduledActions>>,
    cap_board: &Arc<Mutex<Board>>,
    cap_i2c: &Arc<Mutex<I2c>>,
) -> Result<serde_json::Value, anyhow::Error> {
    let (rla, rlb, swpwr, ina, inb) = {
        let act = cap_act_state.lock().unwrap();
        let (ina_act, inb_act) = match act.H0.inverseur {
            -1 => (true, false),
            1 => (false, true),
            2 => (act.H0.speed_a > 0, act.H0.speed_b > 0),
            _ => (false, false),
        };
        (act.rla, act.rlb, act.swpwr, ina_act, inb_act)
    };
    
    let board_readings = {
        let mut b = cap_board.lock().unwrap();
        b.read_value(ina, inb)
    };

    let registry = dynamic_devices::DeviceRegistry::new(Arc::clone(cap_nvs));
    
    // Simuler la lecture des capteurs I2C
    let (_bme_opt, scd_opt, _sht3_opt, sht4_opt) = {
        let mut i2c = cap_i2c.lock().unwrap();
        i2c.read_value()
    };

    let ds_readings = {
        // En 1-Wire, on simule ou on lit les sondes trouvées
        let mut map = std::collections::HashMap::new();
        for probe in cap_probes.iter() {
            map.insert(probe.clone(), 23.5f32); // Fallback static value or read if required
        }
        map
    };

    let list = {
        let sched_lock = cap_sched.lock().unwrap();
        registry.get_devices_display(
            rla,
            rlb,
            swpwr,
            ina,
            inb,
            sht4_opt.as_ref().map(|s| s.temperature).unwrap_or(-255.0),
            sht4_opt.as_ref().map(|s| s.humidity).unwrap_or(-255.0),
            scd_opt.as_ref().map(|s| s.co2).unwrap_or(-255),
            &ds_readings,
            board_readings.touch,
            Some(&sched_lock.schedules),
            board_readings.vsense_volts,
            board_readings.isense_amps,
            &[],
        )
    };

    let mut sensors = Vec::new();
    let mut actuators_list = Vec::new();

    for dev in &list {
        if dev.present == Some(false) {
            continue;
        }

        match dev.id.as_str() {
            "rla" | "rlb" | "ina" | "inb" | "swpwr" => {
                let schedules = {
                    let sched_lock = cap_sched.lock().unwrap();
                    sched_lock.schedules.get(&dev.id).cloned().unwrap_or_default()
                };
                actuators_list.push(serde_json::json!({
                    "Name": dev.id,
                    "description": dev.name,
                    "Type": "tout ou rien",
                    "range": "bool:0 1",
                    "schedules": schedules,
                }));
            }
            "touch" | "vsense" | "isense" => {
                let (s_type, unit) = match dev.id.as_str() {
                    "touch" => ("Touch", "-"),
                    "vsense" => ("Voltage", "V"),
                    "isense" => ("Current", "A"),
                    _ => ("Generic", "-"),
                };
                let meta = dynamic_devices::get_sensor_meta(dev.id.as_str());
                let corr = dynamic_devices::get_correction_formula(cap_nvs, dev.id.as_str());
                let mut sensor_json = serde_json::json!({
                    "Name": dev.id,
                    "description": dev.name,
                    "Type": s_type,
                    "Unit": unit,
                    "correction_formula": corr,
                });
                if let Some(ref m) = meta {
                    sensor_json["uncertainty"] = serde_json::json!(m.uncertainty);
                    sensor_json["range"] = serde_json::json!(m.range);
                    sensor_json["range_min"] = serde_json::json!(m.range[0]);
                    sensor_json["range_max"] = serde_json::json!(m.range[1]);
                }
                sensors.push(sensor_json);
            }
            id if id.starts_with("onewr:") => {
                let meta = dynamic_devices::get_sensor_meta(id);
                let corr = dynamic_devices::get_correction_formula(cap_nvs, id);
                let mut sensor_json = serde_json::json!({
                    "Name": dev.id,
                    "description": dev.name,
                    "Type": "Temperature",
                    "Unit": "°C",
                    "correction_formula": corr,
                });
                if let Some(ref m) = meta {
                    sensor_json["uncertainty"] = serde_json::json!(m.uncertainty);
                    sensor_json["range"] = serde_json::json!(m.range);
                    sensor_json["range_min"] = serde_json::json!(m.range[0]);
                    sensor_json["range_max"] = serde_json::json!(m.range[1]);
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
        let rename_enabled = storage.get_i32("deviceRenamable")?.unwrap_or(1) == 1;
        (m, name, desc, rename_enabled)
    };

    Ok(serde_json::json!({
        "mac": mac,
        "name": name,
        "description": desc,
        "version": FW_VERSION,
        "rename_enabled": rename_enabled,
        "sensors": sensors,
        "actuators": actuators_list,
    }))
}

// --- Route Handlers ---



pub fn handle_captive_redirect(req: Request<&mut EspHttpConnection<'_>>) -> Result<()> {
    let subnet = wifi::AP_IP_B.load(Ordering::Relaxed);
    let location = format!("http://192.168.{}.1/", subnet);
    let mut response = req.into_response(302, Some("Found"), &[("Location", &location)])?;
    response.write(b"Redirecting to captive portal...")?;
    Ok(())
}



pub fn handle_api_status(
    req: Request<&mut EspHttpConnection<'_>>,
    nvs_clone: Arc<Mutex<NvsStorage>>,
    wifi_clone: Arc<Mutex<NetManager>>,
    url_state_clone: Arc<Mutex<UrlValidationState>>,
) -> Result<()> {
    let storage = nvs_clone.lock().unwrap();
    let wifi = wifi_clone.lock().unwrap();
    
    let pairing_remaining = {
        if let Some(until) = wifi.pairing_until {
            let now = std::time::Instant::now();
            if now < until {
                (until - now).as_secs() as i64
            } else {
                0
            }
        } else {
            0
        }
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
        _ => ("None", "".to_string())
    };

    let sta_ip_info = wifi.wifi.wifi().sta_netif().get_ip_info().ok();
    let ap_ip_info = wifi.wifi.wifi().ap_netif().get_ip_info().ok();

    let (wifi_ip, wifi_gateway, wifi_cidr) = if let Some(info) = sta_ip_info {
        (info.ip.to_string(), info.subnet.gateway.to_string(), info.subnet.mask.0)
    } else {
        ("0.0.0.0".to_string(), "0.0.0.0".to_string(), 0)
    };

    let ap_ip = if let Some(info) = ap_ip_info {
        format!("{}/24", info.ip)
    } else {
        let subnet = wifi::AP_IP_B.load(Ordering::Relaxed);
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
    let update_url = storage.get_str("updateRepoList")?.unwrap_or_default();
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
                        let is_ok = check_updates_internal(&url_to_check).is_ok();
                        let mut lock = thread_state_clone.lock().unwrap();
                        *lock = UrlValidationState::Checked(is_ok);
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
    let rename_enabled = storage.get_i32("deviceRenamable")?.unwrap_or(1) == 1;
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
        "ap_ip": ap_ip,
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
        "chip_type": CHIP_TYPE,
        "wifi_known": wifi_known,
        "auto_update": auto_update,
        "rename_enabled": rename_enabled,
        "has_totp": has_totp,
        "partial_totp": partial_totp,
        "ext_name": ext_name,
        "ext_desc": ext_desc,
        "pairing_remaining": pairing_remaining,
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
}

pub fn handle_api_capacity(
    req: Request<&mut EspHttpConnection<'_>>,
    cap_nvs: Arc<Mutex<NvsStorage>>,
    cap_probes: Arc<Vec<String>>,
    cap_act_state: Arc<Mutex<ActuatorsState>>,
    cap_act: Arc<Mutex<Actuators>>,
    cap_sched: Arc<Mutex<ScheduledActions>>,
    cap_board: Arc<Mutex<Board>>,
    cap_i2c: Arc<Mutex<I2c>>,
) -> Result<()> {
    match build_capacity_info(&cap_nvs, &cap_probes, &cap_act_state, &cap_act, &cap_sched, &cap_board, &cap_i2c) {
        Ok(cap_json) => {
            let response_data = serde_json::to_string(&cap_json)?;
            let mut response = req.into_response(200, Some("OK"), &[
                ("Content-Type", "application/json"),
                ("Access-Control-Allow-Origin", "*")
            ])?;
            response.write(response_data.as_bytes())?;
            Ok(())
        }
        Err(e) => {
            let mut response = req.into_response(500, Some("Internal Server Error"), &[
                ("Content-Type", "text/plain"),
            ])?;
            response.write(format!("Error: {:?}", e).as_bytes())?;
            Ok(())
        }
    }
}

pub fn handle_check_updates(
    req: Request<&mut EspHttpConnection<'_>>,
    nvs_updates_clone: Arc<Mutex<NvsStorage>>,
    updates_url_state: Arc<Mutex<UrlValidationState>>,
) -> Result<()> {
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
        storage.get_str("updateRepoList")?.unwrap_or_default()
    };

    if update_url.is_empty() {
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
}

pub fn handle_api_ssids(
    req: Request<&mut EspHttpConnection<'_>>,
    wifi_scan_clone: Arc<Mutex<NetManager>>,
    nvs_ssids_clone: Arc<Mutex<NvsStorage>>,
) -> Result<()> {
    let mut wifi = wifi_scan_clone.lock().unwrap();
    let scan_ok = wifi.active_scan_ssid("");
    let ssids = if scan_ok {
        wifi.scan_cache.clone()
    } else {
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
}

pub fn handle_api_sensors(
    req: Request<&mut EspHttpConnection<'_>>,
    _sensors_nvs: Arc<Mutex<NvsStorage>>,
    i2c: Arc<Mutex<I2c>>,
) -> Result<()> {
    let (bme_opt, scd_opt, sht3_opt, sht4_opt) = {
        let mut bus = i2c.lock().unwrap();
        bus.read_value()
    };

    // Construire le JSON des capteurs compatibles avec ce qui est attendu
    let mut readings = serde_json::json!({
        "i2c:0:0x44_T": sht4_opt.as_ref().map(|s| s.temperature).unwrap_or(-255.0),
        "i2c:0:0x44_H": sht4_opt.as_ref().map(|s| s.humidity).unwrap_or(-255.0),
        "i2c:0:0x45_T": sht3_opt.as_ref().map(|s| s.temperature).unwrap_or(-255.0),
        "i2c:0:0x45_H": sht3_opt.as_ref().map(|s| s.humidity).unwrap_or(-255.0),
        "i2c:0:0x62": scd_opt.as_ref().map(|s| s.co2).unwrap_or(-255),
    });

    if let Some(bme) = bme_opt {
        readings["i2c:0:0x76_T"] = serde_json::json!(bme.temperature);
        readings["i2c:0:0x76_H"] = serde_json::json!(bme.humidity);
        readings["i2c:0:0x76_P"] = serde_json::json!(bme.pressure);
    }

    let response_data = serde_json::to_string(&readings)?;
    let mut response = req.into_ok_response()?;
    response.write(response_data.as_bytes())?;
    Ok(())
}

pub fn handle_api_peripherals(
    req: Request<&mut EspHttpConnection<'_>>,
    periphs_nvs: Arc<Mutex<NvsStorage>>,
    periphs_act_state: Arc<Mutex<ActuatorsState>>,
    periphs_probes: Arc<Vec<String>>,
    periphs_sched: Arc<Mutex<ScheduledActions>>,
    periphs_board: Arc<Mutex<Board>>,
    periphs_i2c: Arc<Mutex<I2c>>,
    cron_handle: crate::cron::CronHandle,
) -> Result<()> {
    let (rla, rlb, swpwr, ina, inb) = {
        let act = periphs_act_state.lock().unwrap();
        let (ina_act, inb_act) = match act.H0.inverseur {
            -1 => (true, false),
            1 => (false, true),
            2 => (act.H0.speed_a > 0, act.H0.speed_b > 0),
            _ => (false, false),
        };
        (act.rla, act.rlb, act.swpwr, ina_act, inb_act)
    };
    
    let board_readings = {
        let mut b = periphs_board.lock().unwrap();
        b.read_value(ina, inb)
    };

    let (_bme_opt, scd_opt, _sht3_opt, sht4_opt) = {
        let mut i2c = periphs_i2c.lock().unwrap();
        i2c.read_value()
    };

    let ds_readings = {
        let mut map = std::collections::HashMap::new();
        if let Ok(opt_temps) = crate::one_wire::ONEWIRE_TEMPERATURES.lock() {
            if let Some(ref global_temps) = *opt_temps {
                for probe in periphs_probes.iter() {
                    let temp = global_temps.get(probe).cloned().unwrap_or(-255.0);
                    map.insert(probe.clone(), temp);
                }
            } else {
                for probe in periphs_probes.iter() {
                    map.insert(probe.clone(), -255.0);
                }
            }
        } else {
            for probe in periphs_probes.iter() {
                map.insert(probe.clone(), -255.0);
            }
        }
        map
    };

    let registry = dynamic_devices::DeviceRegistry::new(Arc::clone(&periphs_nvs));
    let history = cron_handle.get_sensor_history();
    let list = {
        let sched_lock = periphs_sched.lock().unwrap();
        registry.get_devices_display(
            rla,
            rlb,
            swpwr,
            ina,
            inb,
            sht4_opt.as_ref().map(|s| s.temperature).unwrap_or(-255.0),
            sht4_opt.as_ref().map(|s| s.humidity).unwrap_or(-255.0),
            scd_opt.as_ref().map(|s| s.co2).unwrap_or(-255),
            &ds_readings,
            board_readings.touch,
            Some(&sched_lock.schedules),
            board_readings.vsense_volts,
            board_readings.isense_amps,
            &history,
        )
    };

    let response_data = serde_json::to_string(&list)?;
    let mut response = req.into_response(200, Some("OK"), &[
        ("Content-Type", "application/json"),
        ("Access-Control-Allow-Origin", "*")
    ])?;
    response.write(response_data.as_bytes())?;
    Ok(())
}

pub fn handle_rename_peripherals(mut req: Request<&mut EspHttpConnection<'_>>, rename_nvs: Arc<Mutex<NvsStorage>>) -> Result<()> {
    #[derive(serde::Deserialize)]
    struct PeripheralPayload {
        id: String,
        name: Option<String>,
        address: Option<String>,
        polarity: Option<String>,
        unit: Option<String>,
        uncertainty: Option<String>,
        range: Option<[f64; 2]>,
        correction_formula: Option<String>,
        step: Option<u8>,
        pwm_val: Option<u8>,
        schedules: Option<Vec<crate::actuators::ScheduledAction>>,
    }
    let mut buf = vec![0u8; 1024];
    let bytes_read = req.read(&mut buf)?;
    log::info!("[POST /api/peripherals] Bytes read: {}", bytes_read);

    let payload: PeripheralPayload = match serde_json::from_slice(&buf[..bytes_read]) {
        Ok(p) => p,
        Err(e) => {
            log::warn!("[POST /api/peripherals] JSON deserialization failed: {:?}", e);
            let mut response = req.into_status_response(400)?;
            response.write(format!("JSON error: {:?}", e).as_bytes())?;
            return Ok(());
        }
    };

    if let Some(ref n) = payload.name {
        if !is_valid_peripheral_name(n, 64) {
            log::warn!("[POST /api/peripherals] Invalid name validation failed for: '{}'", n);
            let mut response = req.into_status_response(400)?;
            response.write(b"Nom de peripherique invalide. Il doit faire 64 caracteres max, sans ' ou ` ou :.")?;
            return Ok(());
        }
    }

    let mut registry = dynamic_devices::DeviceRegistry::new(Arc::clone(&rename_nvs));
    if let Err(e) = registry.update_device_properties(
        &payload.id,
        payload.name,
        payload.address,
        payload.polarity,
        payload.unit,
        payload.uncertainty,
        payload.range,
        payload.correction_formula,
        payload.step,
        payload.pwm_val,
        payload.schedules,
    ) {
        log::warn!("[POST /api/peripherals] update_device_properties failed for id '{}': {:?}", payload.id, e);
        let mut response = req.into_status_response(400)?;
        response.write(format!("Error: {}", e).as_bytes())?;
        return Ok(());
    }

    log::info!("[POST /api/peripherals] Properties update successful!");
    let mut response = req.into_ok_response()?;
    response.write(b"Update Successful")?;
    Ok(())
}

pub fn handle_sensor_correction(mut req: Request<&mut EspHttpConnection<'_>>, corr_nvs: Arc<Mutex<NvsStorage>>) -> Result<()> {
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
    
    let valid_chars = |c: char| c.is_alphanumeric() || "+-*/^(). _:%xabcdefABCDEF".contains(c);
    if !formula.chars().all(valid_chars) {
        let mut response = req.into_status_response(400)?;
        response.write(b"Formule invalide : caracteres non autorises")?;
        return Ok(());
    }

    dynamic_devices::set_correction_formula(&corr_nvs, &payload.id, formula)?;
    let mut response = req.into_ok_response()?;
    response.write(b"OK")?;
    Ok(())
}

pub fn handle_post_actuators(
    mut req: Request<&mut EspHttpConnection<'_>>,
    act_clone: Arc<Mutex<ActuatorsState>>,
    actuators: Arc<Mutex<Actuators>>,
    nvs: Arc<Mutex<NvsStorage>>,
) -> Result<()> {
    let mut buf = vec![0u8; 256];
    let bytes_read = req.read(&mut buf)?;
    let payload: ActuatorsState = serde_json::from_slice(&buf[..bytes_read])?;
    
    info!("\x1b[35mUpdating actuators state: {:?}\x1b[0m", payload);
    {
        let mut state = act_clone.lock().unwrap();
        *state = payload.clone();
    }

    // Appliquer physiquement et sauvegarder en NVS
    {
        let mut acts = actuators.lock().unwrap();
        if let Some(speed) = payload.rla_speed {
            let _ = acts.relay_a.set_speed(speed as i32);
        }
        let _ = acts.write("rla", payload.rla);

        if let Some(speed) = payload.rlb_speed {
            let _ = acts.relay_b.set_speed(speed as i32);
        }
        let _ = acts.write("rlb", payload.rlb);

        let _ = acts.write("swpwr", payload.swpwr);
        let _ = acts.write_h0(&payload.H0);

        if let Some(brightness) = payload.screen_brightness {
            let mut storage = nvs.lock().unwrap();
            let _ = storage.set_i32("scrBrightness", brightness as i32);
        }

        // Sauvegarder dans devicesKnow de la NVS
        let registry = dynamic_devices::DeviceRegistry::new(Arc::clone(&nvs));
        let mut map = registry.load_registry();

        if let Some(entry) = map.get_mut("rla") {
            if payload.rla {
                entry.pwm_val = Some(payload.rla_speed.unwrap_or(100));
            } else {
                entry.pwm_val = Some(0);
            }
        }
        if let Some(entry) = map.get_mut("rlb") {
            if payload.rlb {
                entry.pwm_val = Some(payload.rlb_speed.unwrap_or(100));
            } else {
                entry.pwm_val = Some(0);
            }
        }
        if let Some(entry) = map.get_mut("swpwr") {
            entry.pwm_val = Some(if payload.swpwr { 100 } else { 0 });
        }
        if let Some(entry) = map.get_mut("H0") {
            entry.inverseur = Some(payload.H0.inverseur);
            if let Some(ref mut ina) = entry.ina {
                ina.pwm_val = payload.H0.speed_a;
            }
            if let Some(ref mut inb) = entry.inb {
                inb.pwm_val = payload.H0.speed_b;
            }
        }

        registry.save_registry(&map);
    }

    let response_data = serde_json::to_string(&payload)?;
    let mut response = req.into_ok_response()?;
    response.write(response_data.as_bytes())?;
    Ok(())
}

pub fn handle_actuators_control(
    mut req: Request<&mut EspHttpConnection<'_>>,
    ctrl_nvs: Arc<Mutex<NvsStorage>>,
    ctrl_act_state: Arc<Mutex<ActuatorsState>>,
    ctrl_act: Arc<Mutex<Actuators>>,
    ctrl_sched: Arc<Mutex<ScheduledActions>>,
) -> Result<()> {
    #[derive(serde::Deserialize)]
    struct ActuatorControlPayload {
        id: String,
        state: bool,
        token: String,
        #[serde(rename = "datetimeUTC")]
        datetime_utc: Option<String>,
    }
    
    let mut buf = vec![0u8; 256];
    let bytes_read = req.read(&mut buf)?;
    let payload: ActuatorControlPayload = match serde_json::from_slice(&buf[..bytes_read]) {
        Ok(p) => p,
        Err(_) => {
            let mut response = req.into_status_response(400)?;
            response.write(b"Format JSON ou payload invalide")?;
            return Ok(());
        }
    };

    let current_secret = {
        let storage = ctrl_nvs.lock().unwrap();
        storage.get_str("totpSecret")?.unwrap_or_default()
    };
    if !current_secret.is_empty() {
        if !current_secret.eq_ignore_ascii_case(&payload.token) {
            let mut response = req.into_status_response(403)?;
            response.write(b"Non autorise : token TOTP incorrect.")?;
            return Ok(());
        }
    }

    if let Some(ref datetime_utc) = payload.datetime_utc {
        let mut scheds = ctrl_sched.lock().unwrap();
        if let Err(e) = scheds.add_schedule(&payload.id, datetime_utc.clone(), payload.state) {
            let mut response = req.into_status_response(400)?;
            response.write(e.as_bytes())?;
            return Ok(());
        }
        let mut response = req.into_ok_response()?;
        response.write(b"Planification enregistree avec succes.")?;
        Ok(())
    } else {
        {
            let mut acts = ctrl_act_state.lock().unwrap();
            match payload.id.as_str() {
                "rla" => acts.rla = payload.state,
                "rlb" => acts.rlb = payload.state,
                "swpwr" => acts.swpwr = payload.state,
                "ina" => {
                    if acts.H0.inverseur == 2 {
                        acts.H0.speed_a = if payload.state { 100 } else { 0 };
                    } else {
                        if payload.state {
                            acts.H0.inverseur = -1;
                            acts.H0.speed_a = 100;
                        } else if acts.H0.inverseur == -1 {
                            acts.H0.inverseur = 0;
                        }
                    }
                }
                "inb" => {
                    if acts.H0.inverseur == 2 {
                        acts.H0.speed_b = if payload.state { 100 } else { 0 };
                    } else {
                        if payload.state {
                            acts.H0.inverseur = 1;
                            acts.H0.speed_b = 100;
                        } else if acts.H0.inverseur == 1 {
                            acts.H0.inverseur = 0;
                        }
                    }
                }
                _ => {
                    let mut response = req.into_status_response(400)?;
                    response.write(b"Identifiant d'actionneur inconnu")?;
                    return Ok(());
                }
            }
        }
        {
            let mut acts = ctrl_act.lock().unwrap();
            if payload.id == "ina" || payload.id == "inb" {
                let state_guard = ctrl_act_state.lock().unwrap();
                let _ = acts.write_h0(&state_guard.H0);
            } else {
                let _ = acts.write(&payload.id, payload.state);
            }
        }
        let mut response = req.into_ok_response()?;
        response.write(b"Action executee avec succes.")?;
        Ok(())
    }
}

pub fn handle_clear_totp(mut req: Request<&mut EspHttpConnection<'_>>, nvs_clear_totp: Arc<Mutex<NvsStorage>>) -> Result<()> {
    #[derive(serde::Deserialize)]
    struct ClearTotpPayload {
        token: String,
    }
    let mut buf = vec![0u8; 256];
    let bytes_read = req.read(&mut buf)?;
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
        let mut response = req.into_ok_response()?;
        response.write(b"OK")?;
        return Ok(());
    }
    
    if !current_secret.eq_ignore_ascii_case(&payload.token) {
        let mut response = req.into_status_response(403)?;
        let err_msg = format!("Non autorise : token TOTP incorrect. Token fourni : '{}'.", payload.token);
        response.write(err_msg.as_bytes())?;
        return Ok(());
    }
    
    {
        let mut storage = nvs_clear_totp.lock().unwrap();
        storage.remove_key("totpSecret")?;
        storage.remove_key("metricsUrl")?;
    }
    
    let mut response = req.into_ok_response()?;
    response.write(b"OK")?;
    Ok(())
}

pub fn handle_reset(mut req: Request<&mut EspHttpConnection<'_>>, nvs_reset: Arc<Mutex<NvsStorage>>) -> Result<()> {
    #[derive(serde::Deserialize)]
    struct ResetPayload {
        confirm: String,
    }
    let mut buf = vec![0u8; 128];
    let bytes_read = req.read(&mut buf)?;
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
        perform_factory_reset(&mut storage)?;
    }
    
    let mut response = req.into_ok_response()?;
    response.write(b"OK")?;
    
    let _ = thread::Builder::new()
        .name("reset_restart_worker".to_string())
        .stack_size(4096)
        .spawn(|| {
            thread::sleep(std::time::Duration::from_secs(2));
            unsafe {
                esp_idf_sys::esp_restart();
            }
        });
        
    Ok(())
}

pub fn handle_config(
    mut req: Request<&mut EspHttpConnection<'_>>,
    nvs_clone: Arc<Mutex<NvsStorage>>,
    wifi_clone: Arc<Mutex<NetManager>>,
    config_url_state: Arc<Mutex<UrlValidationState>>,
) -> Result<()> {
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

    let apply_only = payload.apply_only.unwrap_or(false);
    let mut wifi_success = true;
    let mut totp_success = true;
    let mut totp_err_msg = "";
    let should_reboot_production = false;

    {
        let mut storage = nvs_clone.lock().unwrap();
        if let Some(ref totp_secret) = payload.totp_secret {
            let current = storage.get_str("totpSecret")?.unwrap_or_default();
            if current.is_empty() {
                storage.set_str("totpSecret", totp_secret)?;
            } else {
                let current_supplied = payload.current_totp_secret.as_deref().unwrap_or("");
                if current_supplied == current || *totp_secret == current {
                    storage.set_str("totpSecret", totp_secret)?;
                } else {
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
                storage.set_str("extName", trimmed)?;
            }
            if let Some(ref ext_desc) = payload.ext_desc {
                let trimmed = ext_desc.trim();
                storage.set_str("extDesc", trimmed)?;
            }
            if let Some(ref ntp_server) = payload.ntp_server {
                let trimmed = ntp_server.trim();
                storage.set_str("ntpServer", trimmed)?;
            }
            if let Some(ref metrics_url) = payload.metrics_url {
                let trimmed = metrics_url.trim();
                let formatted = if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
                     format!("http://{}", trimmed)
                } else {
                     trimmed.to_string()
                };
                storage.set_str("metricsUrl", &formatted)?;
            }
            if let Some(rename_en) = payload.rename_enabled {
                let new_val = if rename_en { 1 } else { 0 };
                storage.set_i32("deviceRenamable", new_val)?;
            }
            // if let Some(ref mesh_id) = payload.mesh_id {
            //     let current_id = storage.get_str("meshId")?.unwrap_or_default();
            //     if mesh_id != &current_id {
            //         storage.set_str("meshId", mesh_id)?;
            //         if !apply_only {
            //             should_reboot_production = true;
            //         }
            //     }
            // }
            // if let Some(ref mesh_pmk) = payload.mesh_pmk {
            //     let current_pmk = storage.get_str("meshPmk")?.unwrap_or_default();
            //     if mesh_pmk != &current_pmk {
            //         storage.set_str("meshPmk", mesh_pmk)?;
            //         if !apply_only {
            //             should_reboot_production = true;
            //         }
            //     }
            // }
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

            if wifi.try_sta_connect(ssid, &final_psk, false, 0).unwrap_or(false) {
                wifi_success = true;
            }
            
            if wifi_success {
                storage.set_default_network(ssid, &final_psk)?;
                let _ = storage.update_wifi_last_seen(ssid);
                wifi.state = NetState::WifiOk;
                wifi.retry_count = 0;
                wifi.backoff_delay = std::time::Duration::from_secs(2);
                let _ = wifi.stop_provisioning_ap_if_not_pairing();
            } else {
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
            let current_url = storage.get_str("updateRepoList")?.unwrap_or_default();
            if update_url != &current_url {
                let mut state_lock = config_url_state.lock().unwrap();
                *state_lock = UrlValidationState::NotChecked;
            }
            let is_bin = update_url.ends_with(".bin");
            if is_bin {
                storage.set_str("updateDlUrl", update_url)?;
                storage.set_i32("otaRetry", 3)?;
            } else {
                storage.set_str("updateRepoList", update_url)?;
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
                let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs();
                let next_check = if now > 86400 * 365 { ((now / 86400) + 1) * 86400 + 14 * 3600 } else { 0 };
                storage.set_str("nextCheck", &next_check.to_string())?;
            }
        }
        if apply_only {
            false
        } else if let Some(ref update_url) = payload.update_url {
            if update_url.is_empty() {
                false
            } else {
                let current_url = storage.get_str("updateRepoList")?.unwrap_or_default();
                let is_bin = update_url.ends_with(".bin");
                is_bin || update_url != &current_url
            }
        } else {
            false
        }
    };

    if should_restart {
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
        let _ = thread::Builder::new()
            .name("production_restart_worker".to_string())
            .stack_size(4096)
            .spawn(|| {
                thread::sleep(std::time::Duration::from_secs(2));
                unsafe {
                    esp_idf_sys::esp_restart();
                }
            });
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
}

// --- Fonctions additionnelles de synchronisation de réseau ---

pub fn percent_encode(s: &str) -> String {
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

pub fn get_ext_name(nvs: &Arc<Mutex<NvsStorage>>) -> String {
    let storage = nvs.lock().unwrap();
    storage.get_str("extName").ok().flatten().unwrap_or_else(|| {
        let m = get_mac_address();
        let clean = m.replace(":", "");
        format!("WE-{}", &clean[8..12])
    })
}

#[derive(serde::Deserialize)]
struct SyncResponse {
    wifi_ssid: String,
    wifi_psk: String,
    ntp_server: String,
    #[serde(default)]
    last_seen: Option<u32>,
}

fn request_peer_provisioning(gateway_ip: std::net::Ipv4Addr, my_ip: &str, my_name: &str) -> Result<SyncResponse, anyhow::Error> {
    let config = esp_idf_svc::http::client::Configuration {
        buffer_size: Some(1024),
        crt_bundle_attach: None,
        ..Default::default()
    };
    let mut connection = esp_idf_svc::http::client::EspHttpConnection::new(&config)?;
    let encoded_name = percent_encode(my_name);
    let url = format!("http://{}/api/network/knowledge?mac={}&ip={}&name={}", gateway_ip, get_mac_address(), my_ip, encoded_name);
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
    info!("Provisioning sync response from parent: SSID='{}', last_seen={:?}", res.wifi_ssid, res.last_seen);
    Ok(res)
}

pub fn sync_from_provisioning_peer(nvs: &Arc<Mutex<NvsStorage>>, gateway_ip: std::net::Ipv4Addr) -> Result<bool, anyhow::Error> {
    let my_ip = unsafe {
        let netif = esp_idf_sys::esp_netif_get_handle_from_ifkey(b"WIFI_STA_DEF\0".as_ptr() as *const _);
        if !netif.is_null() {
            let mut ip_info = esp_idf_sys::esp_netif_ip_info_t::default();
            if esp_idf_sys::esp_netif_get_ip_info(netif, &mut ip_info) == 0 {
                let ip = ip_info.ip.addr;
                format!("{}.{}.{}.{}", ip & 0xff, (ip >> 8) & 0xff, (ip >> 16) & 0xff, (ip >> 24) & 0xff)
            } else {
                "0.0.0.0".to_string()
            }
        } else {
            "0.0.0.0".to_string()
        }
    };

    info!("Syncing Wi-Fi credentials from provisioning peer at http://{}/api/network/knowledge?mac={}&ip={} (My IP: {})", gateway_ip, get_mac_address(), my_ip, my_ip);
    thread::sleep(std::time::Duration::from_millis(500));

    let ext_name = get_ext_name(nvs);
    let mut last_err = anyhow::anyhow!("no attempt");
    for attempt in 0..3 {
        if attempt > 0 {
            info!("Provisioning sync retry {}/3...", attempt + 1);
            thread::sleep(std::time::Duration::from_millis(1500));
        }
        match request_peer_provisioning(gateway_ip, my_ip.as_str(), &ext_name) {
            Ok(res) => {
                let should_save_wifi = {
                    let (known_psk, my_last_seen) = {
                        let storage = nvs.lock().unwrap();
                        let known = storage.get_known_networks().unwrap_or_default();
                        known.get(&res.wifi_ssid)
                            .map(|e| (e.psk.clone(), e.last_seen.unwrap_or(0)))
                            .unwrap_or_default()
                    };
                    if known_psk.is_empty() {
                        info!("Sync: New wifi '{}' discovered, saving", res.wifi_ssid);
                        true
                    } else if known_psk != res.wifi_psk {
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
                };
                
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
                return Ok(should_save_wifi);
            }
            Err(e) => {
                warn!("Provisioning sync attempt {} failed: {:?}", attempt + 1, e);
                last_err = e;
            }
        }
    }
    Err(last_err)
}

pub fn handle_api_network_knowledge(
    req: Request<&mut EspHttpConnection<'_>>,
    nvs: Arc<Mutex<NvsStorage>>,
    wifi: Arc<Mutex<NetManager>>,
) -> Result<(), anyhow::Error> {
    use log::warn;
    
    let is_pairing = {
        let net = wifi.lock().unwrap();
        net.state == NetState::ApPairing
    };

    if !is_pairing {
        warn!("Blocked access to /api/network/knowledge: device is not in pairing mode");
        let mut response = req.into_response(403, Some("Forbidden"), &[
            ("Content-Type", "text/plain"),
            ("Access-Control-Allow-Origin", "*")
        ])?;
        response.write(b"Forbidden: Pairing mode is not active")?;
        return Ok(());
    }

    let storage = nvs.lock().unwrap();
    let known = storage.get_known_networks()?;
    let default_net = known.iter().find(|(_, e)| e.default.unwrap_or(false));

    let response_json = if let Some((ssid, entry)) = default_net {
        serde_json::json!({
            "wifi_ssid": ssid,
            "wifi_psk": entry.psk,
            "ntp_server": storage.get_str("ntpServer")?.unwrap_or_default(),
            "last_seen": entry.last_seen.unwrap_or(0),
        })
    } else {
        serde_json::json!({
            "wifi_ssid": "",
            "wifi_psk": "",
            "ntp_server": "",
            "last_seen": 0,
        })
    };

    let response_data = serde_json::to_string(&response_json)?;
    let mut response = req.into_ok_response()?;
    response.write(response_data.as_bytes())?;
    Ok(())
}


