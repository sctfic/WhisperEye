use esp_idf_svc::wifi::{BlockingWifi, EspWifi, Configuration, ClientConfiguration, AccessPointConfiguration, AuthMethod};
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
}

impl WifiManager {
    pub fn new(modem: esp_idf_hal::modem::Modem<'static>, sys_loop: EspSystemEventLoop, nvs: EspDefaultNvsPartition) -> Result<Self> {
        let esp_wifi = EspWifi::new(modem, sys_loop.clone(), Some(nvs))
            .context("Failed to create EspWifi")?;
        let wifi = BlockingWifi::wrap(esp_wifi, sys_loop)?;
        Ok(Self { wifi, scan_cache: Vec::new() })
    }

    pub fn perform_initial_scan(&mut self) -> Result<()> {
        info!("Performing boot-time active Wi-Fi scan...");
        let config = Configuration::Client(ClientConfiguration::default());
        let _ = self.wifi.set_configuration(&config);
        let _ = self.wifi.start();
        match self.wifi.scan() {
            Ok(list) => {
                let mut ssids: Vec<String> = list.into_iter()
                    .map(|n| n.ssid.to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                ssids.sort();
                ssids.dedup();
                info!("Boot-time scan successful: found {} networks.", ssids.len());
                self.scan_cache = ssids;
            }
            Err(e) => {
                warn!("Boot-time active Wi-Fi scan failed: {:?}", e);
            }
        }
        Ok(())
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
        
        thread::spawn(|| {
            if let Err(e) = run_captive_dns_server() {
                error!("Captive DNS Server error: {:?}", e);
            }
        });

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
        Ok(false)
    }

    /// Démarre l'AP Mesh seule (sans connexion STA).
    /// Appelé une seule fois au boot quand le Mesh est activé.
    /// L'AP reste active en permanence pour les clients Mesh et le frontend.
    pub fn start_mesh_ap_only(&mut self, ssid_ap: &str, _psk_ap: &str, channel_ap: u8) -> Result<()> {
        info!("Starting Mesh AP '{}' on channel {} (no STA yet)...", ssid_ap, channel_ap);

        // Configurer en Mixed avec un client vide : l'AP démarre, le STA n'est pas connecté
        let config = Configuration::Mixed(
            ClientConfiguration::default(),
            AccessPointConfiguration {
                ssid: ssid_ap.try_into().unwrap(),
                ssid_hidden: false,
                channel: channel_ap,
                auth_method: AuthMethod::None,
                password: "".try_into().unwrap(),
                ..Default::default()
            }
        );

        self.wifi.set_configuration(&config)?;
        self.wifi.start()?;
        info!("Mesh AP '{}' started successfully.", ssid_ap);

        // Démarrer le serveur DNS captive portal pour les clients AP
        thread::spawn(|| {
            if let Err(e) = run_captive_dns_server() {
                error!("Captive DNS Server error: {:?}", e);
            }
        });

        Ok(())
    }

    /// Tente une connexion STA vers un SSID sans reconfigurer l'AP.
    /// L'AP Mesh reste active tout au long de la tentative.
    /// Retourne true si la connexion STA a réussi.
    pub fn try_sta_on_mesh(&mut self, ssid_sta: &str, psk_sta: &str, ssid_ap: &str, _psk_ap: &str, channel_ap: u8) -> Result<bool> {
        info!("Trying STA '{}' while Mesh AP '{}' stays active...", ssid_sta, ssid_ap);

        // Stopper proprement avant de reconfigurer le STA
        let _ = self.wifi.disconnect();
        let _ = self.wifi.stop();

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
                auth_method: AuthMethod::None,
                password: "".try_into().unwrap(),
                ..Default::default()
            }
        );

        self.wifi.set_configuration(&config)?;
        self.wifi.start()?;

        match self.wifi.connect() {
            Ok(_) => {
                info!("Waiting for DHCP lease (STA on Mesh)...");
                match self.wifi.wait_netif_up() {
                    Ok(_) => {
                        let ip_info = self.wifi.wifi().sta_netif().get_ip_info()?;
                        info!("STA connected on Mesh! IP: {:?}", ip_info.ip);
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

        // En cas d'échec STA, remettre un Mixed propre sans STA (AP seule)
        let _ = self.wifi.disconnect();
        let _ = self.wifi.stop();

        let fallback = Configuration::Mixed(
            ClientConfiguration::default(),
            AccessPointConfiguration {
                ssid: ssid_ap.try_into().unwrap(),
                ssid_hidden: false,
                channel: channel_ap,
                auth_method: AuthMethod::None,
                password: "".try_into().unwrap(),
                ..Default::default()
            }
        );
        self.wifi.set_configuration(&fallback)?;
        self.wifi.start()?;
        info!("STA failed. Mesh AP '{}' restored (AP-only mode).", ssid_ap);

        Ok(false)
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
