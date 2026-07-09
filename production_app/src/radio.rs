#[allow(dead_code)]
pub struct Radio;

#[allow(dead_code)]
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

#[allow(dead_code)]
pub fn is_present() -> bool {
    true
}
