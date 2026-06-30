pub struct Rs485;

impl Rs485 {
    pub fn init() -> Result<Self, anyhow::Error> {
        log::info!("Initializing RS485...");
        Ok(Self)
    }

    pub fn detect(&self) -> bool {
        // Non connecté/détecté pour l'instant
        false
    }

    pub fn read_value(&self) -> Option<()> {
        None
    }
}
