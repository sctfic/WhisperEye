use esp_idf_hal::i2c::I2cDriver;

pub const DETECT_ADDRESSES: &[u8] = &[0x62];

#[derive(Debug, Clone, serde::Serialize)]
pub struct Scd41Readings {
    pub co2: i32,
}

pub struct I2cScd41 {
    pub channel: u8,
    pub address: u8,
    pub is_found: bool,
}

impl I2cScd41 {
    pub fn new(channel: u8, address: u8) -> Self {
        Self {
            channel,
            address,
            is_found: false,
        }
    }

    pub fn init(&mut self, _driver: &mut I2cDriver<'static>) -> Result<(), anyhow::Error> {
        log::info!("Initializing SCD41 CO2 sensor at channel {}, address 0x{:02x}...", self.channel, self.address);
        self.is_found = true;
        Ok(())
    }

    pub fn detect(&self) -> bool {
        self.is_found
    }

    pub fn read_value(&mut self, _driver: &mut I2cDriver<'static>) -> Option<Scd41Readings> {
        // Logique de lecture physique à implémenter plus tard.
        // Pour l'instant, on retourne None ou une valeur de simulation (on retourne None pour être rigoureux).
        None
    }
}
