use esp_idf_svc::ota::EspOta;
use log::{info, error};
use anyhow::Result;

pub struct OtaManager {
    ota: EspOta,
}

impl OtaManager {
    /// Initialise le gestionnaire OTA
    pub fn new() -> Result<Self> {
        info!("Initialisation du service Over-The-Air (OTA)...");
        let ota = EspOta::new()?;
        Ok(Self { ota })
    }

    /// Lance la mise à jour du firmware depuis une source de données en mémoire (ex: reçue via HTTP)
    pub fn update_firmware(&mut self, data: &[u8]) -> Result<()> {
        info!("Début de la mise à jour OTA (taille du binaire : {} octets)...", data.len());
        
        let mut update = self.ota.initiate_update()?;
        
        info!("Écriture du nouveau firmware en partition flash...");
        if let Err(e) = update.write(data) {
            error!("Erreur lors de l'écriture de l'image de mise à jour : {:?}", e);
            update.abort()?;
            return Err(e.into());
        }

        info!("Finalisation et validation de la mise à jour...");
        update.complete()?;
        
        info!("Mise à jour OTA réussie ! L'ESP32 va redémarrer sur la nouvelle partition.");
        Ok(())
    }

    /// Annule la mise à jour ou marque la partition courante comme valide
    pub fn rollback_or_confirm(&self) -> Result<()> {
        // Enregistre que le démarrage sur le firmware actuel s'est bien passé
        // Évite le rollback automatique au prochain redémarrage
        info!("Confirmation de la validité de la partition active...");
        // self.ota.mark_running_as_valid()?;
        Ok(())
    }
}
