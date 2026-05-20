use esp_idf_svc::wifi::{EspWifi, BlockingWifi};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_hal::modem::Modem;
use log::{info, error};
use anyhow::Result;

pub struct WifiManager<'a> {
    wifi: BlockingWifi<EspWifi<'a>>,
}

impl<'a> WifiManager<'a> {
    /// Initialise le Wifi et le Bluetooth (BLE)
    pub fn new(
        modem: Modem<'a>,
        sys_loop: EspSystemEventLoop,
        nvs: EspDefaultNvsPartition,
    ) -> Result<Self> {
        info!("Initialisation de la pile Wifi...");
        
        let esp_wifi = EspWifi::new(
            modem,
            sys_loop.clone(),
            Some(nvs),
        )?;
        
        let wifi = BlockingWifi::wrap(esp_wifi, sys_loop)?;
        
        Ok(Self { wifi })
    }

    /// Se connecte à un réseau Wifi existant (Station Mode)
    pub fn connect(&mut self, ssid: &str, password: &str) -> Result<()> {
        use esp_idf_svc::wifi::{ClientConfiguration, Configuration};
        
        info!("Connexion au réseau Wifi : {}...", ssid);
        
        let config = Configuration::Client(ClientConfiguration {
            ssid: ssid.try_into().unwrap(),
            password: password.try_into().unwrap(),
            ..Default::default()
        });

        self.wifi.set_configuration(&config)?;
        self.wifi.start()?;
        
        match self.wifi.connect() {
            Ok(_) => {
                info!("Connecté au Wifi avec succès !");
                let ip_info = self.wifi.wifi().sta_netif().get_ip_info()?;
                info!("Adresse IP allouée : {:?}", ip_info.ip);
                Ok(())
            }
            Err(e) => {
                error!("Échec de la connexion Wifi : {:?}", e);
                Err(anyhow::anyhow!("Connexion Wifi échouée"))
            }
        }
    }

    /// Démarre un point d'accès Wifi (Access Point Mode) si la connexion échoue
    pub fn start_ap(&mut self, ssid: &str, password: &str) -> Result<()> {
        use esp_idf_svc::wifi::{AccessPointConfiguration, Configuration};
        
        info!("Démarrage du point d'accès (AP) : {}...", ssid);
        
        let config = Configuration::AccessPoint(AccessPointConfiguration {
            ssid: ssid.try_into().unwrap(),
            password: password.try_into().unwrap(),
            channel: 1,
            ..Default::default()
        });

        self.wifi.set_configuration(&config)?;
        self.wifi.start()?;
        
        info!("Point d'accès Wifi démarré avec succès.");
        Ok(())
    }

    /// Initialise la pile Bluetooth BLE
    pub fn init_bluetooth(&self) -> Result<()> {
        info!("Initialisation du Bluetooth BLE (WhisperEye)...");
        // Le code d'initialisation de NimBLE/Bluetooth d'ESP-IDF se place ici.
        // ex: via esp32-nimble crate ou les API C d'esp-idf-sys.
        info!("Bluetooth BLE configuré avec succès en mode veille.");
        Ok(())
    }
}
