use esp_idf_svc::wifi::{BlockingWifi, EspWifi, Configuration, ClientConfiguration, AccessPointConfiguration, AuthMethod};
use esp_idf_svc::wifi::config::{ScanConfig, ScanType};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::handle::RawHandle;
use anyhow::{Result, Context};
use log::{info, error, warn};
use std::time::Duration;
use std::net::UdpSocket;
use std::thread;
use std::sync::{Arc, Mutex};
use common::nvs_storage::NvsStorage;
use crate::MeshState;

extern "C" {
    pub fn ip_napt_enable(addr: u32, enable: i32) -> i32;
}

static DNS_SERVER_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub static AP_IP_R: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(192);
pub static AP_IP_G: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(168);
pub static AP_IP_B: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(71);
pub static AP_IP_A: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(1);

fn make_ip4_addr(a: u8, b: u8, c: u8, d: u8) -> esp_idf_sys::esp_ip4_addr_t {
    esp_idf_sys::esp_ip4_addr_t {
        addr: ((d as u32) << 24) | ((c as u32) << 16) | ((b as u32) << 8) | (a as u32),
    }
}

fn parse_ip_to_u32(a: u8, b: u8, c: u8, d: u8) -> u32 {
    ((d as u32) << 24) | ((c as u32) << 16) | ((b as u32) << 8) | (a as u32)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum NetState {
    WifiPreferred,
    WifiOk,
    WifiFallback,
    MeshOk,
    ApPairing,
}

pub struct NetManager {
    pub wifi: BlockingWifi<EspWifi<'static>>,
    pub state: NetState,
    pub pairing_until: Option<std::time::Instant>,
    pub last_state_change: std::time::Instant,
    pub retry_count: u32,
    pub scan_cache: Vec<String>,
    pub mesh_ssid: String,
    pub mesh_pmk: String,
    pub mesh_channel: u8,
    pub current_sta_ssid: Option<String>,
    pub current_ap_ssid: Option<String>,
    pub current_ap_channel: Option<u8>,
    pub current_ap_open: Option<bool>,
    pub backoff_delay: Duration,
    // Async scan state (non-bloquant)
    pub scan_pending: bool,
    pub scan_start: Option<std::time::Instant>,
    pub scan_target_ssid: String,
    pub scan_is_box: bool, // true = box Wi-Fi, false = mesh
}

impl NetManager {
    pub fn new(
        modem: esp_idf_hal::modem::Modem<'static>,
        sys_loop: EspSystemEventLoop,
        nvs: EspDefaultNvsPartition,
        mesh_ssid: String,
        mesh_pmk: String,
        mesh_channel: u8,
    ) -> Result<Self> {
        let esp_wifi = EspWifi::new(modem, sys_loop.clone(), Some(nvs))
            .context("Failed to create EspWifi")?;
        let wifi = BlockingWifi::wrap(esp_wifi, sys_loop)?;
        
        Ok(Self {
            wifi,
            state: NetState::WifiPreferred,
            pairing_until: None,
            last_state_change: std::time::Instant::now(),
            retry_count: 0,
            scan_cache: Vec::new(),
            mesh_ssid,
            mesh_pmk,
            mesh_channel,
            current_sta_ssid: None,
            current_ap_ssid: None,
            current_ap_channel: None,
            current_ap_open: None,
            backoff_delay: Duration::from_secs(2),
            scan_pending: false,
            scan_start: None,
            scan_target_ssid: String::new(),
            scan_is_box: true,
        })
    }

    pub fn setup_persistent_ap(&mut self, open_ap: bool, distance: i32) -> Result<()> {
        let subnet = match self.state {
            NetState::ApPairing => 70,
            _ => {
                if distance == 0 {
                    71
                } else if distance > 0 {
                    (71 + distance).min(79) as u8
                } else {
                    71
                }
            }
        };

        AP_IP_B.store(subnet, std::sync::atomic::Ordering::SeqCst);

        let (ssid, auth_method, password) = match self.state {
            NetState::ApPairing => (
                "ESP32-Configuration".to_string(),
                AuthMethod::None, // portail captif : reste ouvert
                "".to_string(),
            ),
            _ => {
                // Le mesh est toujours chiffré en WPA2, même si la box Wi‑Fi est ouverte
                (self.mesh_ssid.clone(), AuthMethod::WPA2Personal, self.mesh_pmk.clone())
            }
        };

        info!("Configuring SoftAP: SSID='{}', subnet=192.168.{}.1", ssid, subnet);

        let current_client_cfg = match self.wifi.get_configuration() {
            Ok(Configuration::Mixed(client_cfg, _)) => client_cfg,
            Ok(Configuration::Client(client_cfg)) => client_cfg,
            _ => ClientConfiguration::default(),
        };

        let ap_config = AccessPointConfiguration {
            ssid: ssid.as_str().try_into().unwrap(),
            ssid_hidden: false,
            channel: self.mesh_channel,
            auth_method,
            password: password.as_str().try_into().unwrap(),
            ..Default::default()
        };

        let config = Configuration::Mixed(current_client_cfg, ap_config);
        self.wifi.set_configuration(&config)?;
        self.wifi.start()?;

        let ap_netif = self.wifi.wifi().ap_netif();
        let handle = ap_netif.handle();

        unsafe {
            let _ = esp_idf_sys::esp_netif_dhcps_stop(handle);

            let ip_info = esp_idf_sys::esp_netif_ip_info_t {
                ip: make_ip4_addr(192, 168, subnet, 1),
                gw: make_ip4_addr(192, 168, subnet, 1),
                netmask: make_ip4_addr(255, 255, 255, 0),
            };

            let ret = esp_idf_sys::esp_netif_set_ip_info(handle, &ip_info);
            if ret != 0 {
                warn!("Failed to set SoftAP IP: {}", ret);
            } else {
                info!("SoftAP IP set to 192.168.{}.1", subnet);
            }

            let _ = esp_idf_sys::esp_netif_dhcps_start(handle);

            let ap_ip = parse_ip_to_u32(192, 168, subnet, 1);
            let napt_ret = ip_napt_enable(ap_ip, 1);
            if napt_ret != 0 {
                warn!("ip_napt_enable failed: {}", napt_ret);
            } else {
                info!("NAPT enabled on 192.168.{}.1", subnet);
            }
        }

        if !DNS_SERVER_RUNNING.swap(true, std::sync::atomic::Ordering::SeqCst) {
            thread::spawn(|| {
                if let Err(e) = run_captive_dns_server() {
                    error!("Captive DNS Server error: {:?}", e);
                    DNS_SERVER_RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
                }
            });
        }

        self.current_ap_ssid = Some(ssid);
        self.current_ap_channel = Some(self.mesh_channel);
        self.current_ap_open = Some(password.is_empty());

        Ok(())
    }

    pub fn try_sta_connect(&mut self, ssid: &str, psk: &str, open_ap: bool, distance: i32) -> Result<bool> {
        let masked_psk = if psk.len() > 4 {
            format!("{}...{}", &psk[..2], &psk[psk.len()-2..])
        } else if psk.is_empty() {
            "(vide/open)".to_string()
        } else {
            "***".to_string()
        };
        info!("\x1b[35;1m→ Début tentative connexion STA : SSID='{}', PSK={}, Channel={}, OpenAP={}\x1b[0m",
            ssid, masked_psk, self.mesh_channel, open_ap);

        // Se déconnecter de l'AP actuel avant de changer de SSID
        let _ = self.wifi.disconnect();
        self.current_sta_ssid = None;

        let current_ap_cfg = match self.wifi.get_configuration() {
            Ok(Configuration::Mixed(_, ap_cfg)) => ap_cfg,
            Ok(Configuration::AccessPoint(ap_cfg)) => ap_cfg,
            _ => {
                let _subnet = if distance == 0 { 71 } else if distance > 0 { (71 + distance).min(79) as u8 } else { 71 };
                let auth_method = AuthMethod::WPA2Personal;
                AccessPointConfiguration {
                    ssid: self.mesh_ssid.as_str().try_into().unwrap(),
                    ssid_hidden: false,
                    channel: self.mesh_channel,
                    auth_method,
                    password: self.mesh_pmk.as_str().try_into().unwrap(),
                    ..Default::default()
                }
            }
        };

        let config = Configuration::Mixed(
            ClientConfiguration {
                ssid: ssid.try_into().unwrap(),
                password: psk.try_into().unwrap(),
                ..Default::default()
            },
            current_ap_cfg,
        );

        self.wifi.set_configuration(&config)?;
        self.wifi.start()?;

        info!("Connecting to Wi-Fi...");
        match self.wifi.connect() {
            Ok(_) => {
                info!("Waiting for DHCP lease...");
                match self.wifi.wait_netif_up() {
                    Ok(_) => {
                        let ip_info = self.wifi.wifi().sta_netif().get_ip_info()?;
                        info!("\x1b[35;1m← STA connexion RÉUSSIE ! IP: {:?}\x1b[0m", ip_info.ip);
                        self.current_sta_ssid = Some(ssid.to_string());
                        return Ok(true);
                    }
                    Err(e) => {
                        warn!("\x1b[35;1m← DHCP échoué : {:?}\x1b[0m", e);
                    }
                }
            }
            Err(e) => {
                warn!("\x1b[35;1m← Échec connexion Wi-Fi : {:?}\x1b[0m", e);
            }
        }

        let _ = self.wifi.disconnect();
        self.current_sta_ssid = None;
        Ok(false)
    }

    pub fn active_scan_ssid(&mut self, ssid: &str) -> bool {
        info!("Performing general active scan to find SSID: '{}'", ssid);
        // Scan général (sans filtre SSID) — plus fiable que le scan ciblé
        let config = ScanConfig {
            ssid: None, // pas de filtre, on voit tout
            scan_type: ScanType::Active {
                min: Duration::from_millis(120),
                max: Duration::from_millis(300),
            },
            ..Default::default()
        };

        if let Err(e) = self.wifi.wifi_mut().start_scan(&config, false) {
            warn!("Failed to start active scan: {:?}", e);
            return false;
        }

        let start = std::time::Instant::now();
        let mut done = false;
        while start.elapsed() < Duration::from_secs(4) {
            match self.wifi.wifi().is_scan_done() {
                Ok(scan_done) => {
                    if scan_done {
                        done = true;
                        break;
                    }
                }
                Err(e) => {
                    warn!("is_scan_done failed: {:?}", e);
                    break;
                }
            }
            thread::sleep(Duration::from_millis(100));
        }

        if !done {
            warn!("Active scan timed out after 4s");
            return false;
        }

        match self.wifi.wifi_mut().get_scan_result() {
            Ok(ap_list) => {
                info!("Scan found {} APs. Looking for '{}'...", ap_list.len(), ssid);
                for ap in &ap_list {
                    info!("  AP: SSID='{}', RSSI={}, Channel={}", ap.ssid, ap.signal_strength, ap.channel);
                }
                let found = ap_list.iter().any(|ap| ap.ssid.as_str() == ssid);
                info!("SSID '{}' → {}", ssid, if found { "TROUVÉ" } else { "INTROUVABLE" });
                self.scan_cache = ap_list.into_iter()
                    .map(|ap| ap.ssid.to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                found
            }
            Err(e) => {
                warn!("Failed to get scan results: {:?}", e);
                false
            }
        }
    }

    /// Démarre un scan Wi-Fi asynchrone (non-bloquant). Le résultat sera lu via check_async_scan_result().
    #[allow(dead_code)]
    pub fn start_async_scan(&mut self, ssid: &str, is_box: bool) -> bool {
        let config = ScanConfig {
            ssid: None, // Scan général, pas de filtre — plus fiable
            scan_type: ScanType::Active {
                min: Duration::from_millis(120),
                max: Duration::from_millis(400),
            },
            ..Default::default()
        };

        match self.wifi.wifi_mut().start_scan(&config, false) {
            Ok(_) => {
                self.scan_pending = true;
                self.scan_start = Some(std::time::Instant::now());
                self.scan_target_ssid = ssid.to_string();
                self.scan_is_box = is_box;
                info!("Async scan started for SSID '{}' (box={})", ssid, is_box);
                true
            }
            Err(e) => {
                warn!("Failed to start async scan for '{}': {:?}", ssid, e);
                false
            }
        }
    }

    /// Vérifie si le scan asynchrone est terminé.
    /// Retourne Some(true) si détecté, Some(false) si pas détecté, None si toujours en cours.
    #[allow(dead_code)]
    pub fn check_async_scan_result(&mut self) -> Option<bool> {
        if !self.scan_pending {
            return None;
        }

        let elapsed = self.scan_start.map(|s| s.elapsed()).unwrap_or(Duration::from_secs(0));
        let timed_out = elapsed >= Duration::from_secs(4);

        match self.wifi.wifi().is_scan_done() {
            Ok(true) => {
                self.scan_pending = false;
                self.scan_start = None;
                let ssid = self.scan_target_ssid.clone();
                match self.wifi.wifi_mut().get_scan_result() {
                    Ok(ap_list) => {
                        info!("Async scan found {} APs. Looking for '{}'...", ap_list.len(), ssid);
                        for ap in &ap_list {
                            info!("  AP: SSID='{}', RSSI={}, Channel={}",
                                ap.ssid, ap.signal_strength, ap.channel);
                        }
                        let found = ap_list.iter().any(|ap| ap.ssid.as_str() == ssid);
                        info!("Async scan for '{}' → {}", ssid, if found { "TROUVÉ" } else { "INTROUVABLE" });
                        {
                            let cache = &mut self.scan_cache;
                            *cache = ap_list.into_iter()
                                .map(|ap| ap.ssid.to_string())
                                .filter(|s| !s.is_empty())
                                .collect();
                        }
                        Some(found)
                    }
                    Err(e) => {
                        warn!("Async scan for '{}' failed to get results: {:?}", ssid, e);
                        Some(false)
                    }
                }
            }
            Ok(false) => {
                if timed_out {
                    warn!("Async scan for '{}' timed out after 4s", self.scan_target_ssid);
                    self.scan_pending = false;
                    self.scan_start = None;
                    // Tenter d'annuler le scan en cours
                    let _ = self.wifi.wifi_mut().stop_scan();
                    Some(false)
                } else {
                    None // Toujours en cours
                }
            }
            Err(e) => {
                warn!("is_scan_done failed during async scan: {:?}", e);
                self.scan_pending = false;
                self.scan_start = None;
                Some(false)
            }
        }
    }

    pub fn start_controller_thread(
        this: Arc<Mutex<Self>>,
        nvs: Arc<Mutex<NvsStorage>>,
        mesh_state: Arc<Mutex<MeshState>>,
    ) -> Result<()> {
        thread::Builder::new()
            .name("net_controller".to_string())
            .stack_size(8192)
            .spawn(move || {
                info!("Network Controller Thread started (direct connect mode).");
                let mut last_scan_box = std::time::Instant::now() - Duration::from_secs(60);
                let mut box_retry_count: u32 = 0;
                let mut last_box_retry = std::time::Instant::now();
                let mut mesh_retry_count: u32 = 0;
                let mut last_mesh_retry = std::time::Instant::now();
                let mut ap_only_mode = false;

                loop {
                    thread::sleep(Duration::from_millis(200));
                    let now = std::time::Instant::now();

                    let (state, pairing_until, distance) = {
                        let net = this.lock().unwrap();
                        (net.state, net.pairing_until, {
                            let ms = mesh_state.lock().unwrap();
                            ms.distance
                        })
                    };

                    // --- Gestion du timeout du mode ApPairing ---
                    if state == NetState::ApPairing {
                        if let Some(until) = pairing_until {
                            if now >= until {
                                info!("Pairing mode expired. Reverting to WifiPreferred.");
                                {
                                    let mut net = this.lock().unwrap();
                                    net.state = NetState::WifiPreferred;
                                    net.pairing_until = None;
                                    net.retry_count = 0;
                                    net.backoff_delay = Duration::from_secs(2);
                                    let mut ms = mesh_state.lock().unwrap();
                                    ms.pairing_until = None;
                                    let open_ap = {
                                        let storage = nvs.lock().unwrap();
                                        let known = storage.get_known_networks().unwrap_or_default();
                                        let default_net_psk = known.values().find(|e| e.default.unwrap_or(false)).map(|e| e.psk.clone()).unwrap_or_default();
                                        default_net_psk.is_empty()
                                    };
                                    let _ = net.setup_persistent_ap(open_ap, ms.distance);
                                }
                                box_retry_count = 0;
                                last_box_retry = now;
                            }
                        }
                        continue;
                    }

                    match state {
                        NetState::ApPairing => {} // déjà filtré plus haut

                        NetState::WifiPreferred => {
                            let backoff_secs = std::cmp::min(2u64.pow(box_retry_count), 32);
                            if now.duration_since(last_box_retry) < Duration::from_secs(backoff_secs) {
                                continue;
                            }

                            let (ssid, psk) = {
                                let storage = nvs.lock().unwrap();
                                let known = storage.get_known_networks().unwrap_or_default();
                                known.iter()
                                    .find(|(_, e)| e.default.unwrap_or(false))
                                    .map(|(s, e)| (s.clone(), e.psk.clone()))
                                    .unwrap_or_default()
                            };

                            if ssid.is_empty() {
                                info!("WifiPreferred: No default Wi-Fi configured. Switching to mesh attempt.");
                                let mut net = this.lock().unwrap();
                                net.state = NetState::WifiFallback;
                                continue;
                            }

                            info!("WifiPreferred: Direct connect to SSID '{}' (retry #{})", ssid, box_retry_count);
                            let mut net = this.lock().unwrap();
                            let open_ap = psk.is_empty();
                            match net.try_sta_connect(&ssid, &psk, open_ap, 0) {
                                Ok(true) => {
                                    info!("WifiPreferred: Successfully connected to box Wi-Fi!");
                                    net.state = NetState::WifiOk;
                                    box_retry_count = 0;
                                    mesh_retry_count = 0;
                                    ap_only_mode = false;
                                    net.backoff_delay = Duration::from_secs(2);
                                    let mut ms = mesh_state.lock().unwrap();
                                    ms.is_root = true;
                                    ms.distance = 0;
                                    let _ = net.setup_persistent_ap(open_ap, 0);
                                    drop(net);
                                    // Enregistrer le timestamp de connexion dans la NVS
                                    if let Ok(mut storage) = nvs.lock() {
                                        let _ = storage.update_wifi_last_seen(&ssid);
                                    }
                                }
                                _ => {
                                    box_retry_count += 1;
                                    last_box_retry = now;
                                    warn!("WifiPreferred: Connection failed. Retry #{}. Alternating to mesh.", box_retry_count);
                                    // Alterner : passer au mesh
                                    net.state = NetState::WifiFallback;
                                }
                            }
                        }

                        NetState::WifiOk => {
                            let connected = {
                                let net = this.lock().unwrap();
                                net.wifi.is_connected().unwrap_or(false)
                            };
                            if !connected {
                                warn!("WifiOk: Connection to box Wi-Fi lost!");
                                let mut net = this.lock().unwrap();
                                net.state = NetState::WifiPreferred;
                                box_retry_count = 0;
                                mesh_retry_count = 0;
                                last_box_retry = now;
                                net.backoff_delay = Duration::from_secs(2);
                            }
                        }

                        NetState::WifiFallback => {
                            let mesh_backoff = if mesh_retry_count >= 3 { 30u64 } else { 10u64 };
                            if now.duration_since(last_mesh_retry) < Duration::from_secs(mesh_backoff) {
                                continue;
                            }
                            last_mesh_retry = now;

                            let (mesh_ssid, mesh_pmk) = {
                                let net = this.lock().unwrap();
                                (net.mesh_ssid.clone(), net.mesh_pmk.clone())
                            };

                            info!("WifiFallback: Direct connect to mesh SSID '{}' (retry #{}), PMK len={}", mesh_ssid, mesh_retry_count, mesh_pmk.len());
                            let mut net = this.lock().unwrap();
                            match net.try_sta_connect(&mesh_ssid, &mesh_pmk, mesh_pmk.is_empty(), distance) {
                                Ok(true) => {
                                    mesh_retry_count = 0;
                                    box_retry_count = 0;
                                    ap_only_mode = false;
                                    common::led::MESH_RETRIES_EXHAUSTED.store(false, std::sync::atomic::Ordering::Relaxed);
                                    // Passer immédiatement en MeshOk (distance=1), le sync se fera en arrière-plan
                                    net.state = NetState::MeshOk;
                                    {
                                        let mut ms = mesh_state.lock().unwrap();
                                        ms.is_root = false;
                                        ms.distance = 1;
                                    }
                                    let _ = net.setup_persistent_ap(mesh_pmk.is_empty(), 1);
                                    // Lancer le sync HTTP dans un thread séparé (non bloquant)
                                    let nvs_clone = Arc::clone(&nvs);
                                    let mesh_state_clone = Arc::clone(&mesh_state);
                                    let this_clone = Arc::clone(&this);
                                    drop(net);
                                    thread::Builder::new()
                                        .name("mesh_sync".to_string())
                                        .stack_size(8192)
                                        .spawn(move || {
                                            // Délai aléatoire 1-4s pour désynchroniser les ESP32
                                            let delay_ms = 1000 + (unsafe { esp_idf_sys::esp_random() } % 3000) as u64;
                                            thread::sleep(Duration::from_millis(delay_ms));
                                            let gateway_ip = {
                                                let net = this_clone.lock().unwrap();
                                                net.wifi.wifi().sta_netif().get_ip_info().map(|info| info.subnet.gateway).unwrap_or(std::net::Ipv4Addr::new(192, 168, 71, 1))
                                            };
                                            match crate::perform_mesh_sync(&nvs_clone, gateway_ip) {
                                                Ok(parent_distance) => {
                                                    info!("WifiFallback: Mesh sync successful! Parent distance: {}", parent_distance);
                                                    let mut ms = mesh_state_clone.lock().unwrap();
                                                    ms.distance = parent_distance + 1;
                                                    // Déclencher immédiatement la connexion au Wi-Fi box
                                                    if let Ok(mut net) = this_clone.lock() {
                                                        info!("WifiFallback: Mesh sync OK → tentative connexion Wi-Fi box avec les credentials synchronisés");
                                                        net.state = NetState::WifiPreferred;
                                                        net.last_state_change = std::time::Instant::now();
                                                    }
                                                }
                                                Err(e) => {
                                                    warn!("WifiFallback: Mesh sync failed (thread): {:?}. Distance=1.", e);
                                                }
                                            }
                                        })
                                        .map(|_| ())
                                        .unwrap_or_else(|e| warn!("Failed to spawn mesh_sync thread: {:?}", e));
                                }
                                _ => {
                                    mesh_retry_count += 1;
                                    warn!("WifiFallback: Failed to connect to parent Mesh. Retry #{}. Alternating to box.", mesh_retry_count);
                                    if mesh_retry_count >= 3 {
                                        common::led::MESH_RETRIES_EXHAUSTED.store(true, std::sync::atomic::Ordering::Relaxed);
                                    }
                                    // Alterner : repasser en WifiPreferred
                                    net.state = NetState::WifiPreferred;
                                    // Si les deux sont épuisés → AP-only
                                    if !ap_only_mode && box_retry_count >= 5 && mesh_retry_count >= 3 {
                                        info!("WifiFallback: Both box and mesh exhausted. Starting AP-only mode.");
                                        let _ = net.wifi.stop();
                                        let _ = net.setup_persistent_ap(mesh_pmk.is_empty(), -1);
                                        ap_only_mode = true;
                                    }
                                }
                            }
                        }

                        NetState::MeshOk => {
                            let connected = {
                                let net = this.lock().unwrap();
                                net.wifi.is_connected().unwrap_or(false)
                            };
                            if !connected {
                                warn!("MeshOk: Connection to Mesh parent lost. Transitioning to WifiFallback.");
                                let mut net = this.lock().unwrap();
                                net.state = NetState::WifiFallback;
                                mesh_retry_count = 0;
                                last_mesh_retry = now;
                            } else if last_scan_box.elapsed() >= Duration::from_secs(60) {
                                last_scan_box = std::time::Instant::now();
                                let (ssid, psk) = {
                                    let storage = nvs.lock().unwrap();
                                    let known = storage.get_known_networks().unwrap_or_default();
                                    known.iter()
                                        .find(|(_, e)| e.default.unwrap_or(false))
                                        .map(|(s, e)| (s.clone(), e.psk.clone()))
                                        .unwrap_or_default()
                                };
                                if !ssid.is_empty() {
                                    info!("MeshOk: Trying direct connect to box Wi-Fi '{}'...", ssid);
                                    let mut net = this.lock().unwrap();
                                    let _ = net.wifi.disconnect();
                                    let open_ap = psk.is_empty();
                                    if net.try_sta_connect(&ssid, &psk, open_ap, 0).unwrap_or(false) {
                                        info!("MeshOk: Reconnected to box Wi-Fi!");
                                        net.state = NetState::WifiOk;
                                        box_retry_count = 0;
                                        last_box_retry = now;
                                        net.backoff_delay = Duration::from_secs(2);
                                        let mut ms = mesh_state.lock().unwrap();
                                        ms.is_root = true;
                                        ms.distance = 0;
                                        let _ = net.setup_persistent_ap(open_ap, 0);
                                    }
                                }
                            }
                        }
                    }
                }
            })
            .context("Failed to spawn net_controller thread")?;

        Ok(())
    }
}

fn run_captive_dns_server() -> Result<()> {
    let socket = UdpSocket::bind("0.0.0.0:53").context("Could not bind DNS port 53")?;
    info!("Captive DNS Server running on UDP port 53...");
    
    let mut buf = [0u8; 512];
    
    loop {
        match socket.recv_from(&mut buf) {
            Ok((size, src)) => {
                if size < 12 {
                    continue;
                }
                
                let transaction_id = &buf[0..2];
                let questions = ((buf[4] as u16) << 8) | (buf[5] as u16);
                
                if questions == 0 {
                    continue;
                }
                
                let mut response = Vec::new();
                response.extend_from_slice(transaction_id);
                response.extend_from_slice(&[0x81, 0x80]);
                response.extend_from_slice(&buf[4..6]);
                response.extend_from_slice(&buf[4..6]);
                response.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
                
                let mut q_idx = 12;
                for _ in 0..questions {
                    while q_idx < size && buf[q_idx] != 0 {
                        let label_len = buf[q_idx] as usize;
                        q_idx += 1 + label_len;
                    }
                    q_idx += 5;
                }
                
                if q_idx > size {
                    continue;
                }
                
                response.extend_from_slice(&buf[12..q_idx]);
                
                let mut current_offset = 12;
                for _ in 0..questions {
                    response.extend_from_slice(&[0xc0, current_offset as u8]);
                    response.extend_from_slice(&[0x00, 0x01, 0x00, 0x01]);
                    response.extend_from_slice(&[0x00, 0x00, 0x00, 0x3c]);
                    response.extend_from_slice(&[0x00, 0x04]);
                    
                    response.extend_from_slice(&[
                        AP_IP_R.load(std::sync::atomic::Ordering::Relaxed),
                        AP_IP_G.load(std::sync::atomic::Ordering::Relaxed),
                        AP_IP_B.load(std::sync::atomic::Ordering::Relaxed),
                        AP_IP_A.load(std::sync::atomic::Ordering::Relaxed),
                    ]);
                    
                    while current_offset < size && buf[current_offset] != 0 {
                        let len = buf[current_offset] as usize;
                        current_offset += 1 + len;
                    }
                    current_offset += 5;
                }
                
                let _ = socket.send_to(&response, src);
            }
            Err(e) => {
                error!("DNS socket recv error: {:?}", e);
                thread::sleep(Duration::from_millis(500));
            }
        }
    }
}
