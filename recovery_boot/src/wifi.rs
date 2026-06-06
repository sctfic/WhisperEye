use esp_idf_svc::wifi::{BlockingWifi, EspWifi, Configuration, ClientConfiguration, AccessPointConfiguration, AuthMethod};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_hal::peripherals::Peripherals;
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
    pub fn new(peripherals: Peripherals, sys_loop: EspSystemEventLoop, nvs: EspDefaultNvsPartition) -> Result<Self> {
        let esp_wifi = EspWifi::new(peripherals.modem, sys_loop.clone(), Some(nvs))
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
        
        // Wait and connect with timeout
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
        
        // If connection fails, stop Wi-Fi to clean up before transitioning
        let _ = self.wifi.stop();
        Ok(false)
    }

    pub fn start_ap(&mut self) -> Result<()> {
        info!("Starting AP mode: 'ESP32-Configuration'...");
        
        let config = Configuration::AccessPoint(AccessPointConfiguration {
            ssid: "ESP32-Configuration".try_into().unwrap(),
            ssid_hidden: false,
            channel: 6,
            auth_method: AuthMethod::None, // Open network
            ..Default::default()
        });

        self.wifi.set_configuration(&config)?;
        self.wifi.start()?;
        
        info!("AP mode started successfully!");
        
        // Start captive portal DNS server in background
        thread::spawn(|| {
            if let Err(e) = run_captive_dns_server() {
                error!("Captive DNS Server error: {:?}", e);
            }
        });

        Ok(())
    }
}

/// Simple UDP DNS server that intercepts all queries and responds with 192.168.71.1
fn run_captive_dns_server() -> Result<()> {
    let socket = UdpSocket::bind("0.0.0.0:53").context("Could not bind DNS port 53")?;
    info!("Captive DNS Server running on UDP port 53...");
    
    let mut buf = [0u8; 512];
    
    loop {
        match socket.recv_from(&mut buf) {
            Ok((size, src)) => {
                if size < 12 {
                    continue; // Invalid DNS query
                }
                
                // DNS Header parsing
                let transaction_id = &buf[0..2];
                let questions = ((buf[4] as u16) << 8) | (buf[5] as u16);
                
                if questions == 0 {
                    continue;
                }
                
                // Construct captive portal DNS Response
                let mut response = Vec::new();
                
                // Transaction ID
                response.extend_from_slice(transaction_id);
                // Flags: Response, Opcode standard, Authoritative, No Error
                response.extend_from_slice(&[0x81, 0x80]);
                // Questions count
                response.extend_from_slice(&buf[4..6]);
                // Answer RRs count (match the questions count to resolve everything)
                response.extend_from_slice(&buf[4..6]);
                // Authority/Additional RRs: 0
                response.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
                
                // Find where the question details end in the request
                let mut q_idx = 12;
                for _ in 0..questions {
                    while q_idx < size && buf[q_idx] != 0 {
                        let label_len = buf[q_idx] as usize;
                        q_idx += 1 + label_len;
                    }
                    q_idx += 5; // skip null byte, type (2 bytes), class (2 bytes)
                }
                
                if q_idx > size {
                    continue; // Overflow/Malformed query
                }
                
                // Copy queries (questions section)
                response.extend_from_slice(&buf[12..q_idx]);
                
                // Append answers for each question pointing to 192.168.71.1
                let mut current_offset = 12;
                for _ in 0..questions {
                    // Answer name pointer to the corresponding query name
                    response.extend_from_slice(&[0xc0, current_offset as u8]);
                    
                    // Type: A record (0x0001), Class: IN (0x0001)
                    response.extend_from_slice(&[0x00, 0x01, 0x00, 0x01]);
                    // TTL: 60 seconds (0x0000003c)
                    response.extend_from_slice(&[0x00, 0x00, 0x00, 0x3c]);
                    // Data length: 4 bytes
                    response.extend_from_slice(&[0x00, 0x04]);
                    // Address: 192.168.71.1
                    response.extend_from_slice(&[192, 168, 71, 1]);
                    
                    // Advance pointer offset
                    while current_offset < size && buf[current_offset] != 0 {
                        let len = buf[current_offset] as usize;
                        current_offset += 1 + len;
                    }
                    current_offset += 5;
                }
                
                if let Err(e) = socket.send_to(&response, src) {
                    warn!("Failed to send DNS response: {:?}", e);
                }
            }
            Err(e) => {
                error!("DNS socket recv error: {:?}", e);
                thread::sleep(Duration::from_millis(500));
            }
        }
    }
}
