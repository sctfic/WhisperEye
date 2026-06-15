use esp_idf_svc::wifi::{BlockingWifi, EspWifi, Configuration, ClientConfiguration, AccessPointConfiguration, AuthMethod};
use esp_idf_svc::wifi::config::{ScanConfig, ScanType};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use anyhow::{Result, Context};
use log::{info, error, warn};
use std::time::Duration;
use std::net::UdpSocket;
use std::thread;

pub struct WifiManager {
    pub wifi: BlockingWifi<EspWifi<'static>>,
    pub scan_cache: Vec<String>,
    pub current_ap_ssid: Option<String>,
    pub current_ap_channel: Option<u8>,
    pub current_ap_open: Option<bool>,
    pub current_sta_ssid: Option<String>,
}

impl WifiManager {
    pub fn new(modem: esp_idf_hal::modem::Modem<'static>, sys_loop: EspSystemEventLoop, nvs: EspDefaultNvsPartition) -> Result<Self> {
        let esp_wifi = EspWifi::new(modem, sys_loop.clone(), Some(nvs))
            .context("Failed to create EspWifi")?;
        let wifi = BlockingWifi::wrap(esp_wifi, sys_loop)?;
        Ok(Self {
            wifi,
            scan_cache: Vec::new(),
            current_ap_ssid: None,
            current_ap_channel: None,
            current_ap_open: None,
            current_sta_ssid: None,
        })
    }


    pub fn start_sta(&mut self, ssid: &str, psk: &str) -> Result<bool> {
        info!("Attempting STA connection to SSID: '{}'", ssid);
        
        let config = Configuration::Client(ClientConfiguration {
            ssid: ssid.try_into().unwrap(),
            password: psk.try_into().unwrap(),
            ..Default::default()
        });

        self.wifi.set_configuration(&config)?;
        self.wifi.start()?;
        
        info!("Connecting to Wi-Fi...");
        match self.wifi.connect() {
            Ok(_) => {
                info!("Waiting for DHCP lease...");
                match self.wifi.wait_netif_up() {
                    Ok(_) => {
                        let ip_info = self.wifi.wifi().sta_netif().get_ip_info()?;
                        info!("STA Connection successful! IP: {:?}", ip_info.ip);
                        self.current_sta_ssid = Some(ssid.to_string());
                        self.current_ap_ssid = None;
                        return Ok(true);
                    }
                    Err(e) => {
                        warn!("DHCP lease failed: {:?}", e);
                    }
                }
            }
            Err(e) => {
                warn!("Wi-Fi connection failed: {:?}", e);
            }
        }
        
        let _ = self.wifi.stop();
        self.current_sta_ssid = None;
        self.current_ap_ssid = None;
        Ok(false)
    }

    pub fn start_ap(&mut self) -> Result<()> {
        info!("Starting AP mode: 'ESP32-Configuration'...");
        
        let config = Configuration::AccessPoint(AccessPointConfiguration {
            ssid: "ESP32-Configuration".try_into().unwrap(),
            ssid_hidden: false,
            channel: 6,
            auth_method: AuthMethod::None,
            ..Default::default()
        });

        self.wifi.set_configuration(&config)?;
        self.wifi.start()?;
        
        info!("AP mode started successfully!");
        
        if !DNS_SERVER_RUNNING.swap(true, std::sync::atomic::Ordering::SeqCst) {
            thread::spawn(|| {
                if let Err(e) = run_captive_dns_server() {
                    error!("Captive DNS Server error: {:?}", e);
                    DNS_SERVER_RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
                }
            });
        }

        self.current_ap_ssid = Some("ESP32-Configuration".to_string());
        self.current_ap_channel = Some(6);
        self.current_ap_open = Some(true);
        self.current_sta_ssid = None;

        Ok(())
    }

    pub fn start_ap_sta(&mut self, ssid_sta: &str, psk_sta: &str, ssid_ap: &str, psk_ap: &str, channel_ap: u8) -> Result<bool> {
        info!("Attempting Mixed mode: STA connect to '{}', AP start '{}' on channel {}", ssid_sta, ssid_ap, channel_ap);
        
        let config = Configuration::Mixed(
            ClientConfiguration {
                ssid: ssid_sta.try_into().unwrap(),
                password: psk_sta.try_into().unwrap(),
                ..Default::default()
            },
            AccessPointConfiguration {
                ssid: ssid_ap.try_into().unwrap(),
                ssid_hidden: false,
                channel: channel_ap,
                auth_method: if psk_ap.is_empty() { AuthMethod::None } else { AuthMethod::WPA2Personal },
                password: psk_ap.try_into().unwrap(),
                ..Default::default()
            }
        );

        self.wifi.set_configuration(&config)?;
        self.wifi.start()?;
        
        info!("Connecting to Wi-Fi STA in Mixed mode...");
        match self.wifi.connect() {
            Ok(_) => {
                info!("Waiting for DHCP lease in Mixed mode...");
                match self.wifi.wait_netif_up() {
                    Ok(_) => {
                        let ip_info = self.wifi.wifi().sta_netif().get_ip_info()?;
                        info!("STA Connection successful in Mixed mode! IP: {:?}", ip_info.ip);
                        self.current_ap_ssid = Some(ssid_ap.to_string());
                        self.current_ap_channel = Some(channel_ap);
                        self.current_ap_open = Some(psk_ap.is_empty());
                        self.current_sta_ssid = Some(ssid_sta.to_string());
                        return Ok(true);
                    }
                    Err(e) => {
                        warn!("DHCP lease failed in Mixed mode: {:?}", e);
                    }
                }
            }
            Err(e) => {
                warn!("Wi-Fi connection failed in Mixed mode: {:?}", e);
            }
        }
        
        // AP remains active even if STA connection fails
        self.current_ap_ssid = Some(ssid_ap.to_string());
        self.current_ap_channel = Some(channel_ap);
        self.current_ap_open = Some(psk_ap.is_empty());
        self.current_sta_ssid = None;
        Ok(false)
    }

    /// Démarre l'AP Mesh seule (sans connexion STA).
    /// Appelé une seule fois au boot quand le Mesh est activé.
    /// L'AP reste active en permanence pour les clients Mesh et le frontend.
    pub fn start_mesh_ap_only(&mut self, ssid_ap: &str, psk_ap: &str, channel_ap: u8, open_ap: bool) -> Result<()> {
        if self.wifi.is_started().unwrap_or(false)
            && self.current_ap_ssid.as_deref() == Some(ssid_ap)
            && self.current_ap_channel == Some(channel_ap)
            && self.current_ap_open == Some(open_ap)
            && self.current_sta_ssid.is_none()
        {
            info!("Mesh AP '{}' already running with correct config, skipping restart.", ssid_ap);
            return Ok(());
        }

        info!("Starting Mesh AP '{}' on channel {} (open: {})...", ssid_ap, channel_ap, open_ap);

        let (auth_method, password) = if open_ap || psk_ap.is_empty() {
            (AuthMethod::None, "".to_string())
        } else {
            (AuthMethod::WPA2Personal, psk_ap.to_string())
        };

        // Configurer en Mixed avec un client vide : l'AP démarre, le STA n'est pas connecté
        let config = Configuration::Mixed(
            ClientConfiguration::default(),
            AccessPointConfiguration {
                ssid: ssid_ap.try_into().unwrap(),
                ssid_hidden: false,
                channel: channel_ap,
                auth_method,
                password: password.as_str().try_into().unwrap(),
                ..Default::default()
            }
        );

        self.wifi.set_configuration(&config)?;
        self.wifi.start()?;
        
        self.current_ap_ssid = Some(ssid_ap.to_string());
        self.current_ap_channel = Some(channel_ap);
        self.current_ap_open = Some(open_ap);
        self.current_sta_ssid = None;

        info!("Mesh AP '{}' started successfully.", ssid_ap);

        // Démarrer le serveur DNS captive portal pour les clients AP
        if !DNS_SERVER_RUNNING.swap(true, std::sync::atomic::Ordering::SeqCst) {
            thread::spawn(|| {
                if let Err(e) = run_captive_dns_server() {
                    error!("Captive DNS Server error: {:?}", e);
                    DNS_SERVER_RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
                }
            });
        }

        Ok(())
    }

    /// Tente une connexion STA vers un SSID sans reconfigurer l'AP.
    /// L'AP Mesh reste active tout au long de la tentative.
    /// Retourne true si la connexion STA a réussi.
    pub fn try_sta_on_mesh(
        &mut self,
        ssid_sta: &str,
        psk_sta: &str,
        ssid_ap: &str,
        psk_ap: &str,
        channel_ap: u8,
        open_ap: bool,
    ) -> Result<bool> {
        // Si on est déjà connecté à cette STA en particulier, on évite de tout couper
        if self.wifi.is_started().unwrap_or(false)
            && self.current_sta_ssid.as_deref() == Some(ssid_sta)
            && self.current_ap_ssid.as_deref() == Some(ssid_ap)
            && self.current_ap_channel == Some(channel_ap)
            && self.current_ap_open == Some(open_ap)
        {
            info!("Already connected to STA '{}' with Mesh AP active. Skipping try_sta_on_mesh.", ssid_sta);
            return Ok(true);
        }

        info!("Trying STA '{}' while Mesh AP '{}' stays active (open: {})...", ssid_sta, ssid_ap, open_ap);

        // Déconnecter le STA précédent sans couper l'AP
        let _ = self.wifi.disconnect();

        let (auth_method, password) = if open_ap || psk_ap.is_empty() {
            (AuthMethod::None, "".to_string())
        } else {
            (AuthMethod::WPA2Personal, psk_ap.to_string())
        };

        let config = Configuration::Mixed(
            ClientConfiguration {
                ssid: ssid_sta.try_into().unwrap(),
                password: psk_sta.try_into().unwrap(),
                ..Default::default()
            },
            AccessPointConfiguration {
                ssid: ssid_ap.try_into().unwrap(),
                ssid_hidden: false,
                channel: channel_ap,
                auth_method,
                password: password.as_str().try_into().unwrap(),
                ..Default::default()
            }
        );

        self.wifi.set_configuration(&config)?;

        match self.wifi.connect() {
            Ok(_) => {
                info!("Waiting for DHCP lease (STA on Mesh)...");
                match self.wifi.wait_netif_up() {
                    Ok(_) => {
                        let ip_info = self.wifi.wifi().sta_netif().get_ip_info()?;
                        info!("STA connected on Mesh! IP: {:?}", ip_info.ip);
                        self.current_ap_ssid = Some(ssid_ap.to_string());
                        self.current_ap_channel = Some(channel_ap);
                        self.current_ap_open = Some(open_ap);
                        self.current_sta_ssid = Some(ssid_sta.to_string());
                        return Ok(true);
                    }
                    Err(e) => {
                        warn!("DHCP failed on Mesh STA: {:?}", e);
                    }
                }
            }
            Err(e) => {
                warn!("STA connect failed on Mesh: {:?}", e);
            }
        }

        // En cas d'échec STA, remettre un Mixed propre sans STA (AP seule) sans couper l'AP
        let _ = self.wifi.disconnect();

        let fallback = Configuration::Mixed(
            ClientConfiguration::default(),
            AccessPointConfiguration {
                ssid: ssid_ap.try_into().unwrap(),
                ssid_hidden: false,
                channel: channel_ap,
                auth_method,
                password: password.as_str().try_into().unwrap(),
                ..Default::default()
            }
        );
        self.wifi.set_configuration(&fallback)?;
        
        self.current_ap_ssid = Some(ssid_ap.to_string());
        self.current_ap_channel = Some(channel_ap);
        self.current_ap_open = Some(open_ap);
        self.current_sta_ssid = None;

        info!("STA failed. Mesh AP '{}' restored (AP-only mode).", ssid_ap);

        Ok(false)
    }

    pub fn active_scan_ssid(&mut self, ssid: &str) -> bool {
        info!("Performing targeted active scan for SSID: '{}'", ssid);
        let mut hs_ssid = heapless::String::<32>::new();
        if hs_ssid.push_str(&ssid[..std::cmp::min(ssid.len(), 32)]).is_ok() {
            let config = ScanConfig {
                ssid: Some(hs_ssid),
                scan_type: ScanType::Active {
                    min: Duration::from_millis(100),
                    max: Duration::from_millis(250),
                },
                ..Default::default()
            };
            
            if let Err(e) = self.wifi.wifi_mut().start_scan(&config, false) {
                warn!("Failed to start active scan for '{}': {:?}", ssid, e);
                return false;
            }
            
            let wait_res = self.wifi.wifi_wait_while(
                || self.wifi.wifi().is_scan_done().map(|done| !done),
                None
            );
            
            if let Err(e) = wait_res {
                warn!("Active scan wait failed for '{}': {:?}", ssid, e);
                return false;
            }
            
            match self.wifi.wifi_mut().get_scan_result() {
                Ok(ap_list) => {
                    let found = ap_list.iter().any(|ap| ap.ssid.as_str() == ssid);
                    info!("Targeted active scan for '{}' result: {}", ssid, found);
                    found
                }
                Err(e) => {
                    warn!("Targeted active scan for '{}' failed to get results: {:?}", ssid, e);
                    false
                }
            }
        } else {
            false
        }
    }
}

static DNS_SERVER_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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
                    response.extend_from_slice(&[192, 168, 71, 1]);
                    
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
