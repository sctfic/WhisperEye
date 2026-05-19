use esp_idf_svc::sntp::{EspSntp, SyncStatus};
use log::{info, warn};
use std::time::SystemTime;
use anyhow::Result;

pub struct NtpManager {
    sntp: EspSntp,
}

impl NtpManager {
    /// Initialise le client NTP et lance la synchronisation en arrière-plan
    pub fn new(server: &str) -> Result<Self> {
        info!("Initialisation du client NTP avec le serveur : {}...", server);
        
        let sntp = EspSntp::new_with_servers([server])?;
        
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
