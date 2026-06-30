use esp_idf_hal::i2c::I2cDriver;

pub const DETECT_ADDRESSES: &[u8] = &[0x45]; // Adresse par défaut typique de SHT3x

#[derive(Debug, Clone, serde::Serialize)]
pub struct Sht3xReadings {
    pub temperature: f32,
    pub humidity: f32,
}

pub struct I2cSht3x {
    pub channel: u8,
    pub address: u8,
    pub is_found: bool,
}

impl I2cSht3x {
    pub fn new(channel: u8, address: u8) -> Self {
        Self {
            channel,
            address,
            is_found: false,
        }
    }

    pub fn init(&mut self, _driver: &mut I2cDriver<'static>) -> Result<(), anyhow::Error> {
        log::info!("Initializing SHT3x sensor at channel {}, address 0x{:02x}...", self.channel, self.address);
        self.is_found = true;
        Ok(())
    }

    pub fn detect(&self) -> bool {
        self.is_found
    }

    pub fn read_value(&mut self, _driver: &mut I2cDriver<'static>) -> Option<Sht3xReadings> {
        None
    }
}
