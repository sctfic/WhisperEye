use anyhow::{Context, Result};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::handle::RawHandle;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::config::{ScanConfig, ScanType};
use esp_idf_svc::wifi::{
    AccessPointConfiguration, AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi,
    PmfConfiguration,
};
use log::{error, info, warn};
use std::collections::{HashMap, HashSet};
use std::net::UdpSocket;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use common::nvs_storage::NvsStorage;
use common::led::{self, LedStaStatus, LedApStatus};

pub const PROVISIONING_SSID: &str = "ESP32-Configuration";
const PROVISIONING_SUBNET: u8 = 70;
const WIFI_RETRY_DELAY: Duration = Duration::from_secs(10);
const PAIRING_DURATION: Duration = Duration::from_secs(120);

static DNS_SERVER_RUNNING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
pub static API_DOWNLOAD_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub static API_UPLOAD_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub static TLS_OR_SCAN_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

struct TlsOrScanGuard;
impl Drop for TlsOrScanGuard {
    fn drop(&mut self) {
        TLS_OR_SCAN_ACTIVE.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}
pub static AP_IP_R: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(192);
pub static AP_IP_G: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(168);
pub static AP_IP_B: std::sync::atomic::AtomicU8 =
    std::sync::atomic::AtomicU8::new(PROVISIONING_SUBNET);
pub static AP_IP_A: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(1);

fn make_ip4_addr(a: u8, b: u8, c: u8, d: u8) -> esp_idf_sys::esp_ip4_addr_t {
    esp_idf_sys::esp_ip4_addr_t {
        addr: ((d as u32) << 24) | ((c as u32) << 16) | ((b as u32) << 8) | (a as u32),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum NetState {
    WifiPreferred,
    WifiOk,
    ProvisioningScan,
    ProvisioningOk,
    ProvisioningAp,
    ApPairing,
}

pub struct NetManager {
    pub wifi: BlockingWifi<EspWifi<'static>>,
    pub state: NetState,
    pub pairing_until: Option<Instant>,
    pub last_state_change: Instant,
    pub retry_count: u32,
    pub scan_cache: Vec<String>,
    pub scan_results: HashMap<String, i32>,
    pub provisioning_channel: u8,
    pub current_sta_ssid: Option<String>,
    pub current_ap_ssid: Option<String>,
    pub current_ap_channel: Option<u8>,
    pub current_ap_open: Option<bool>,
    pub backoff_delay: Duration,
    pub scan_pending: bool,
    pub scan_start: Option<Instant>,
    pub scan_target_ssid: String,
    pub scan_is_box: bool,
}

impl NetManager {
    pub fn new(
        modem: esp_idf_hal::modem::Modem<'static>,
        sys_loop: EspSystemEventLoop,
        nvs: EspDefaultNvsPartition,
        _mesh_ssid: String,
        _mesh_pmk: String,
        mesh_channel: u8,
    ) -> Result<Self> {
        let esp_wifi =
            EspWifi::new(modem, sys_loop.clone(), Some(nvs)).context("Failed to create EspWifi")?;
        let wifi = BlockingWifi::wrap(esp_wifi, sys_loop)?;

        Ok(Self {
            wifi,
            state: NetState::WifiPreferred,
            pairing_until: None,
            last_state_change: Instant::now(),
            retry_count: 0,
            scan_cache: Vec::new(),
            scan_results: HashMap::new(),
            provisioning_channel: mesh_channel,
            current_sta_ssid: None,
            current_ap_ssid: None,
            current_ap_channel: None,
            current_ap_open: None,
            backoff_delay: WIFI_RETRY_DELAY,
            scan_pending: false,
            scan_start: None,
            scan_target_ssid: String::new(),
            scan_is_box: true,
        })
    }

    pub fn setup_provisioning_ap(&mut self) -> Result<()> {
        AP_IP_B.store(PROVISIONING_SUBNET, std::sync::atomic::Ordering::SeqCst);
        info!(
            "Configuring provisioning captive portal: SSID='{}', subnet=192.168.{}.1",
            PROVISIONING_SSID, PROVISIONING_SUBNET
        );

        let current_client_cfg = match self.wifi.get_configuration() {
            Ok(Configuration::Mixed(client_cfg, _)) => client_cfg,
            Ok(Configuration::Client(client_cfg)) => client_cfg,
            _ => ClientConfiguration::default(),
        };

        // esp-idf-svc 0.52 does not expose WIFI_AUTH_OWE in AuthMethod.
        // Keep this AP open for compatibility; switch authmode here when the binding exposes OWE.
        let ap_config = AccessPointConfiguration {
            ssid: PROVISIONING_SSID.try_into().unwrap(),
            ssid_hidden: false,
            channel: self.provisioning_channel,
            auth_method: AuthMethod::None,
            password: "".try_into().unwrap(),
            ..Default::default()
        };

        self.wifi
            .set_configuration(&Configuration::Mixed(current_client_cfg, ap_config))?;
        self.wifi.start()?;

        let ap_netif = self.wifi.wifi().ap_netif();
        let handle = ap_netif.handle();

        unsafe {
            let _ = esp_idf_sys::esp_netif_dhcps_stop(handle);
            let ip_info = esp_idf_sys::esp_netif_ip_info_t {
                ip: make_ip4_addr(192, 168, PROVISIONING_SUBNET, 1),
                gw: make_ip4_addr(192, 168, PROVISIONING_SUBNET, 1),
                netmask: make_ip4_addr(255, 255, 255, 0),
            };

            let ret = esp_idf_sys::esp_netif_set_ip_info(handle, &ip_info);
            if ret != 0 {
                warn!("Failed to set provisioning SoftAP IP: {}", ret);
            }
            let _ = esp_idf_sys::esp_netif_dhcps_start(handle);
        }

        if !DNS_SERVER_RUNNING.swap(true, std::sync::atomic::Ordering::SeqCst) {
            thread::spawn(|| {
                if let Err(e) = run_captive_dns_server() {
                    error!("Captive DNS Server error: {:?}", e);
                    DNS_SERVER_RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
                }
            });
        }

        self.current_ap_ssid = Some(PROVISIONING_SSID.to_string());
        self.current_ap_channel = Some(self.provisioning_channel);
        self.current_ap_open = Some(true);
        Ok(())
    }

    pub fn stop_provisioning_ap_if_not_pairing(&mut self) -> Result<()> {
        if self.state == NetState::ApPairing || self.state == NetState::ProvisioningAp {
            return Ok(());
        }

        if matches!(
            self.wifi.get_configuration(),
            Ok(Configuration::Mixed(_, _))
        ) {
            let client_cfg = match self.wifi.get_configuration()? {
                Configuration::Mixed(client_cfg, _) => client_cfg,
                Configuration::Client(client_cfg) => client_cfg,
                _ => ClientConfiguration::default(),
            };
            self.wifi
                .set_configuration(&Configuration::Client(client_cfg))?;
            self.current_ap_ssid = None;
            self.current_ap_channel = None;
            self.current_ap_open = None;
        }
        Ok(())
    }

    pub fn try_sta_connect(
        &mut self,
        ssid: &str,
        psk: &str,
        _open_ap: bool,
        _distance: i32,
    ) -> Result<bool> {
        let masked_psk = if psk.len() > 4 {
            format!("{}...{}", &psk[..2], &psk[psk.len() - 2..])
        } else if psk.is_empty() {
            "(open/owe)".to_string()
        } else {
            "***".to_string()
        };
        info!(
            "STA connect attempt: SSID='{}', PSK={}, channel_hint={}",
            ssid, masked_psk, self.provisioning_channel
        );

        let _ = self.wifi.disconnect();
        self.current_sta_ssid = None;

        let current_ap_cfg = match self.wifi.get_configuration() {
            Ok(Configuration::Mixed(_, ap_cfg)) => Some(ap_cfg),
            Ok(Configuration::AccessPoint(ap_cfg)) => Some(ap_cfg),
            _ => None,
        };

        let client_cfg = ClientConfiguration {
            ssid: ssid.try_into().unwrap(),
            password: psk.try_into().unwrap(),
            auth_method: if psk.is_empty() {
                AuthMethod::None
            } else {
                AuthMethod::WPA2Personal
            },
            channel: if ssid == PROVISIONING_SSID {
                Some(self.provisioning_channel)
            } else {
                None
            },
            pmf_cfg: PmfConfiguration::NotCapable,
            ..Default::default()
        };

        let config = if let Some(ap_cfg) = current_ap_cfg {
            Configuration::Mixed(client_cfg, ap_cfg)
        } else {
            Configuration::Client(client_cfg)
        };

        self.wifi.set_configuration(&config)?;
        self.wifi.start()?;

        match self.wifi.connect() {
            Ok(_) => match self.wifi.wait_netif_up() {
                Ok(_) => {
                    let ip_info = self.wifi.wifi().sta_netif().get_ip_info()?;
                    info!("STA connection succeeded. IP: {:?}", ip_info.ip);
                    self.current_sta_ssid = Some(ssid.to_string());
                    Ok(true)
                }
                Err(e) => {
                    warn!("STA DHCP failed for '{}': {:?}", ssid, e);
                    let _ = self.wifi.disconnect();
                    Ok(false)
                }
            },
            Err(e) => {
                warn!("STA connection failed for '{}': {:?}", ssid, e);
                let _ = self.wifi.disconnect();
                Ok(false)
            }
        }
    }

    pub fn active_scan_ssid(&mut self, ssid: &str) -> bool {
        self.scan_available_networks()
            .map(|list| list.iter().any(|(s, _)| s == ssid))
            .unwrap_or(false)
    }

    pub fn scan_available_networks(&mut self) -> Result<Vec<(String, i32)>> {
        use std::sync::atomic::Ordering;
        if TLS_OR_SCAN_ACTIVE.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            anyhow::bail!("Wi-Fi scan failed: TLS or scan already active");
        }
        let _guard = TlsOrScanGuard;

        if !self.wifi.is_started().unwrap_or(false) {
            info!("Wi-Fi driver not started. Starting Wi-Fi before scan.");
            self.wifi.start()?;
        }
        info!("Performing active Wi-Fi scan");
        let config = ScanConfig {
            ssid: None,
            scan_type: ScanType::Active {
                min: Duration::from_millis(120),
                max: Duration::from_millis(300),
            },
            ..Default::default()
        };

        self.wifi
            .wifi_mut()
            .start_scan(&config, false)
            .context("Failed to start active scan")?;

        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(4) {
            if self.wifi.wifi().is_scan_done().unwrap_or(false) {
                let ap_list = self.wifi.wifi_mut().get_scan_result()?;
                let mut best: HashMap<String, i32> = HashMap::new();
                for ap in ap_list {
                    let ssid = ap.ssid.to_string();
                    if ssid.is_empty() {
                        continue;
                    }
                    best.entry(ssid)
                        .and_modify(|rssi| *rssi = (*rssi).max(ap.signal_strength as i32))
                        .or_insert(ap.signal_strength as i32);
                }
                let mut networks: Vec<(String, i32)> = best.clone().into_iter().collect();
                networks.sort_by(|a, b| b.1.cmp(&a.1));
                self.scan_results = best;
                self.scan_cache = networks.iter().map(|(ssid, _)| ssid.clone()).collect();
                info!("Scan found visible SSIDs: {:?}", self.scan_cache);
                return Ok(networks);
            }
            thread::sleep(Duration::from_millis(100));
        }

        let _ = self.wifi.wifi_mut().stop_scan();
        anyhow::bail!("Active scan timed out")
    }

    pub fn initiate_async_scan(&mut self) -> Result<()> {
        use std::sync::atomic::Ordering;
        if TLS_OR_SCAN_ACTIVE.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            anyhow::bail!("Async scan postponed: TLS or scan already active");
        }

        let mut start_scan_internal = || -> Result<()> {
            if !self.wifi.is_started().unwrap_or(false) {
                info!("Wi-Fi driver not started. Starting Wi-Fi before scan.");
                self.wifi.start()?;
            }
            info!("Starting non-blocking active Wi-Fi scan");
            let config = ScanConfig {
                ssid: None,
                scan_type: ScanType::Active {
                    min: Duration::from_millis(120),
                    max: Duration::from_millis(300),
                },
                ..Default::default()
            };

            self.wifi
                .wifi_mut()
                .start_scan(&config, false)
                .context("Failed to start active scan")?;
            
            self.scan_pending = true;
            self.scan_start = Some(Instant::now());
            Ok(())
        };

        if let Err(e) = start_scan_internal() {
            TLS_OR_SCAN_ACTIVE.store(false, Ordering::SeqCst);
            return Err(e);
        }

        Ok(())
    }

    pub fn check_async_scan_complete(&mut self) -> Result<Option<Vec<(String, i32)>>> {
        if !self.scan_pending {
            return Ok(None);
        }

        use std::sync::atomic::Ordering;
        let is_done = self.wifi.wifi().is_scan_done().unwrap_or(false);
        if is_done {
            self.scan_pending = false;
            self.scan_start = None;
            TLS_OR_SCAN_ACTIVE.store(false, Ordering::SeqCst);
            let ap_list = self.wifi.wifi_mut().get_scan_result()?;
            let mut best: HashMap<String, i32> = HashMap::new();
            for ap in ap_list {
                let ssid = ap.ssid.to_string();
                if ssid.is_empty() {
                    continue;
                }
                best.entry(ssid)
                    .and_modify(|rssi| *rssi = (*rssi).max(ap.signal_strength as i32))
                    .or_insert(ap.signal_strength as i32);
            }
            let mut networks: Vec<(String, i32)> = best.clone().into_iter().collect();
            networks.sort_by(|a, b| b.1.cmp(&a.1));
            self.scan_results = best;
            self.scan_cache = networks.iter().map(|(ssid, _)| ssid.clone()).collect();
            info!("Async scan completed. Found visible SSIDs: {:?}", self.scan_cache);
            return Ok(Some(networks));
        }

        if let Some(start) = self.scan_start {
            if start.elapsed() >= Duration::from_secs(4) {
                warn!("Async Wi-Fi scan timed out");
                let _ = self.wifi.wifi_mut().stop_scan();
                self.scan_pending = false;
                self.scan_start = None;
                TLS_OR_SCAN_ACTIVE.store(false, Ordering::SeqCst);
                return Err(anyhow::anyhow!("Scan timeout"));
            }
        }

        Ok(None)
    }

    pub fn start_controller_thread(
        this: Arc<Mutex<Self>>,
        nvs: Arc<Mutex<NvsStorage>>,
    ) -> Result<()> {
        thread::Builder::new()
            .name("net_controller".to_string())
            .stack_size(16384)
            .spawn(move || {
                info!("Network Controller Thread started (direct Wi-Fi + provisioning mode).");
                let mut last_wifi_cycle = Instant::now() - WIFI_RETRY_DELAY;

                loop {
                    thread::sleep(Duration::from_millis(200));
                    let now = Instant::now();

                    let (connected, current_ssid_val, current_ip_val) = {
                        if let Ok(net) = this.lock() {
                            let conn = net.wifi.is_connected().unwrap_or(false);
                            let mut ssid_str = String::new();
                            let mut ip_str = String::new();
                            if conn {
                                if let Some(ref s) = net.current_sta_ssid {
                                    ssid_str = s.clone();
                                }
                                if let Ok(ip_info) = net.wifi.wifi().sta_netif().get_ip_info() {
                                    ip_str = format!("{}/{}", ip_info.ip, ip_info.subnet.mask.0);
                                }
                            }
                            (conn, ssid_str, ip_str)
                        } else {
                            (false, String::new(), String::new())
                        }
                    };
                    WIFI_CONNECTED.store(connected, std::sync::atomic::Ordering::Relaxed);
                    if let Ok(mut g_ssid) = CURRENT_SSID.lock() {
                        *g_ssid = current_ssid_val;
                    }
                    if let Ok(mut g_ip) = CURRENT_IP.lock() {
                        *g_ip = current_ip_val;
                    }

                    let state = { this.lock().unwrap().state };

                    if state == NetState::ApPairing {
                        // Pendant le pairing, le WiFi STA reste actif → on garde le statut STA
                        // (déjà positionné par WifiOk/ProvisioningOk) et on ajoute le orange AP
                        led::set_ap_status(LedApStatus::ApPairing);
                        if AP_CLIENT_CONNECTED.swap(false, std::sync::atomic::Ordering::Relaxed) {
                            let _until = AP_CLIENT_CONNECTED.load(std::sync::atomic::Ordering::Relaxed); // Placeholder, unused in calculation
                            let new_until = Instant::now() + PAIRING_DURATION;
                            {
                                let mut net = this.lock().unwrap();
                                net.pairing_until = Some(new_until);
                            }
                            info!("Provisioning pairing: AP client connected, extended by 120s");
                        }

                        let expired = {
                            let net = this.lock().unwrap();
                            net.pairing_until.map(|u| now >= u).unwrap_or(true)
                        };
                        if expired {
                            info!("Provisioning pairing expired. Returning to Wi-Fi priority mode.");
                            let mut net = this.lock().unwrap();
                            net.pairing_until = None;
                            net.state = NetState::WifiPreferred;
                            net.last_state_change = now;
                            // Rétablir le LED AP à Off
                            led::set_ap_status(LedApStatus::Off);
                        }
                        continue;
                    }

                    match state {
                        NetState::WifiOk => {
                            led::set_sta_status(LedStaStatus::WifiOk);
                            let connected = {
                                let net = this.lock().unwrap();
                                net.wifi.is_connected().unwrap_or(false)
                            };
                            if !connected {
                                warn!("Wi-Fi connection lost. Returning to Wi-Fi priority mode.");
                                let mut net = this.lock().unwrap();
                                net.state = NetState::WifiPreferred;
                                net.last_state_change = now;
                                last_wifi_cycle = now - WIFI_RETRY_DELAY;
                            }
                        }

                        NetState::WifiPreferred => {
                            led::set_sta_status(LedStaStatus::WifiAttempting);
                            if now.duration_since(last_wifi_cycle) < WIFI_RETRY_DELAY {
                                continue;
                            }
                            last_wifi_cycle = now;

                            // 1. Tenter le réseau par défaut
                            if try_default_wifi(&this, &nvs).unwrap_or(false) {
                                continue;
                            }

                            // 2. Si échec ou absent, lancer le scan asynchrone
                            let mut net = this.lock().unwrap();
                            info!("Default Wi-Fi failed or not found. Starting async scan for other known networks.");
                            if let Err(e) = net.initiate_async_scan() {
                                warn!("Failed to start async scan: {:?}", e);
                                // Si le scan ne peut pas démarrer, on tente le provisioning peer directement
                                drop(net);
                                if try_provisioning_peer(&this, &nvs).unwrap_or(false) {
                                    last_wifi_cycle = now - WIFI_RETRY_DELAY;
                                    continue;
                                }
                                // Sinon AP provisioning
                                let mut net = this.lock().unwrap();
                                net.state = NetState::ProvisioningAp;
                                net.last_state_change = now;
                                led::set_sta_status(LedStaStatus::ProvisioningOk);
                                led::set_ap_status(LedApStatus::ProvisioningSsid);
                                let _ = net.setup_provisioning_ap();
                            } else {
                                net.state = NetState::ProvisioningScan;
                                net.last_state_change = now;
                            }
                        }

                        NetState::ProvisioningScan => {
                            led::set_sta_status(LedStaStatus::WifiAttempting);
                            let scan_res = {
                                let mut net = this.lock().unwrap();
                                net.check_async_scan_complete()
                            };

                            match scan_res {
                                Ok(None) => {
                                    // Scan toujours en cours, on rend la main (non-bloquant)
                                    continue;
                                }
                                Ok(Some(visible)) => {
                                    // Scan terminé avec succès, on tente de se connecter aux réseaux connus visibles
                                    if try_known_wifi_from_visible(&this, &nvs, &visible).unwrap_or(false) {
                                        continue;
                                    }
                                }
                                Err(e) => {
                                    // Scan en timeout ou erreur, on continue avec une liste vide
                                    warn!("Async scan failed during cycle: {:?}", e);
                                }
                            }

                            // Si on arrive ici, le scan est terminé (ou a échoué) et aucune connexion Wi-Fi connue n'a marché.
                            // On tente de trouver un WhisperEye de provisioning.
                            if try_provisioning_peer(&this, &nvs).unwrap_or(false) {
                                last_wifi_cycle = now - WIFI_RETRY_DELAY;
                                continue;
                            }

                            info!("No known Wi-Fi/provisioning peer reachable. Starting permanent captive portal.");
                            let mut net = this.lock().unwrap();
                            net.state = NetState::ProvisioningAp;
                            net.last_state_change = now;
                            led::set_sta_status(LedStaStatus::ProvisioningOk);
                            led::set_ap_status(LedApStatus::ProvisioningSsid);
                            if let Err(e) = net.setup_provisioning_ap() {
                                warn!("Failed to start permanent provisioning AP: {:?}", e);
                            }
                        }

                        NetState::ProvisioningAp => {
                            // En mode Portail Captif permanent, aucun scan d'arrière-plan n'est requis.
                            // On attend passivement qu'une nouvelle configuration soit injectée par l'API.
                            continue;
                        }

                        NetState::ProvisioningOk => {
                            let mut net = this.lock().unwrap();
                            net.state = NetState::WifiPreferred;
                            net.last_state_change = now;
                        }

                        NetState::ApPairing => {}
                    }
                }
            })
            .context("Failed to spawn net_controller thread")?;

        Ok(())
    }
}

fn try_default_wifi(
    this: &Arc<Mutex<NetManager>>,
    nvs: &Arc<Mutex<NvsStorage>>,
) -> Result<bool> {
    let known = {
        let storage = nvs.lock().unwrap();
        storage.get_known_networks().unwrap_or_default()
    };

    let default_network = known
        .iter()
        .find(|(_, e)| e.default.unwrap_or(false))
        .map(|(s, e)| (s.clone(), e.psk.clone()));

    if let Some((default_ssid, default_psk)) = default_network.as_ref() {
        let mut net = this.lock().unwrap();
        info!("Trying default Wi-Fi once: '{}'", default_ssid);
        if net.try_sta_connect(default_ssid, default_psk, false, 0)? {
            net.state = NetState::WifiOk;
            net.retry_count = 0;
            let _ = net.stop_provisioning_ap_if_not_pairing();
            drop(net);
            led::set_sta_status(LedStaStatus::WifiOk);
            led::set_ap_status(LedApStatus::Off);
            if let Ok(mut storage) = nvs.lock() {
                let _ = storage.update_wifi_last_seen(&default_ssid);
            }
            return Ok(true);
        }
    }
    Ok(false)
}

fn try_known_wifi_from_visible(
    this: &Arc<Mutex<NetManager>>,
    nvs: &Arc<Mutex<NvsStorage>>,
    visible: &[(String, i32)],
) -> Result<bool> {
    let known = {
        let storage = nvs.lock().unwrap();
        storage.get_known_networks().unwrap_or_default()
    };

    let default_network = known
        .iter()
        .find(|(_, e)| e.default.unwrap_or(false))
        .map(|(s, e)| (s.clone(), e.psk.clone()));

    let visible_ssids: HashSet<String> = visible.iter().map(|(ssid, _)| ssid.clone()).collect();
    for (ssid, entry) in known.iter() {
        if default_network
            .as_ref()
            .map(|(s, _)| s == ssid)
            .unwrap_or(false)
            || !visible_ssids.contains(ssid)
        {
            continue;
        }

        let mut net = this.lock().unwrap();
        info!("Trying visible known Wi-Fi: '{}'", ssid);
        if net.try_sta_connect(ssid, &entry.psk, false, 0)? {
            net.state = NetState::WifiOk;
            net.retry_count = 0;
            let _ = net.stop_provisioning_ap_if_not_pairing();
            drop(net);
            led::set_sta_status(LedStaStatus::WifiOk);
            led::set_ap_status(LedApStatus::Off);
            if let Ok(mut storage) = nvs.lock() {
                let _ = storage.set_default_network_by_ssid(ssid);
                let _ = storage.update_wifi_last_seen(ssid);
            }
            return Ok(true);
        }
    }

    Ok(false)
}

fn try_provisioning_peer(
    this: &Arc<Mutex<NetManager>>,
    nvs: &Arc<Mutex<NvsStorage>>,
) -> Result<bool> {
    {
        let mut net = this.lock().unwrap();
        info!("Trying provisioning peer '{}' (direct connection on channel {})", PROVISIONING_SSID, net.provisioning_channel);
        if !net.try_sta_connect(PROVISIONING_SSID, "", true, -1)? {
            return Ok(false);
        }
        net.state = NetState::ProvisioningOk;
        led::set_sta_status(LedStaStatus::ProvisioningOk);
    }

    let gateway_ip = {
        let net = this.lock().unwrap();
        net.wifi
            .wifi()
            .sta_netif()
            .get_ip_info()
            .map(|info| info.subnet.gateway)
            .unwrap_or(std::net::Ipv4Addr::new(192, 168, PROVISIONING_SUBNET, 1))
    };

    match crate::web_handlers::sync_from_provisioning_peer(nvs, gateway_ip) {
        Ok(had_new_wifi) => {
            info!(
                "Provisioning sync completed from {}. New/updated Wi-Fi: {}",
                gateway_ip, had_new_wifi
            );
            Ok(had_new_wifi)
        }
        Err(e) => {
            warn!("Provisioning sync failed from {}: {:?}", gateway_ip, e);
            Ok(false)
        }
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

/// Flag signalé par main.rs quand un client se connecte à l'AP (ApStaConnected)
pub static AP_CLIENT_CONNECTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Statut de la connexion WiFi cliente
pub static WIFI_CONNECTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub static CURRENT_SSID: Mutex<String> = Mutex::new(String::new());
pub static CURRENT_IP: Mutex<String> = Mutex::new(String::new());

pub fn get_ap_num_clients() -> usize {
    unsafe {
        let mut sta_list = esp_idf_sys::wifi_sta_list_t::default();
        if esp_idf_sys::esp_wifi_ap_get_sta_list(&mut sta_list) == 0 {
            sta_list.num as usize
        } else {
            0
        }
    }
}
