pub struct Radio;

impl Radio {
    pub fn init() -> Result<Self, anyhow::Error> {
        log::info!("Initializing Radio...");
        Ok(Self)
    }

    pub fn detect(&self) -> bool {
        // La radio est présente sur la carte de production par défaut
        true
    }

    pub fn read_value(&self) -> Option<()> {
        None
    }
}

pub fn is_present() -> bool {
    true
}
