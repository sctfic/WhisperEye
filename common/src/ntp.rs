use esp_idf_svc::sntp::{EspSntp, SntpConf, SyncStatus};
use log::info;
use std::time::SystemTime;
use anyhow::Result;

pub struct NtpManager {
    sntp: EspSntp<'static>,
}

impl NtpManager {
    /// Initialise le client NTP et lance la synchronisation en arrière-plan
    pub fn new(server: &str) -> Result<Self> {
        info!("Initialisation du client NTP avec le serveur : {}...", server);
        
        let mut conf = SntpConf::default();
        if !conf.servers.is_empty() {
            conf.servers[0] = server;
        }
        let sntp = EspSntp::new(&conf)?;
        
        Ok(Self { sntp })
    }

    /// Vérifie le statut actuel de la synchronisation de l'heure
    pub fn is_synchronized(&self) -> bool {
        match self.sntp.get_sync_status() {
            SyncStatus::Completed => true,
            _ => false,
        }
    }

    /// Récupère la date et l'heure actuelle sous forme de chaîne de caractères
    pub fn get_formatted_time(&self) -> String {
        let now = SystemTime::now();
        if let Ok(duration) = now.duration_since(SystemTime::UNIX_EPOCH) {
            let secs = duration.as_secs();
            // Retourne un timestamp ou convertit en date/heure humaine
            format!("UNIX timestamp: {}", secs)
        } else {
            String::from("Heure non initialisée")
        }
    }
}
