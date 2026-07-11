use esp_idf_hal::i2c::I2cDriver;
use std::thread;
use std::time::Duration;
use std::sync::Mutex;

pub static SHT3X_TEMP: Mutex<f32> = Mutex::new(-255.0);
pub static SHT3X_HUM: Mutex<f32> = Mutex::new(-255.0);

pub static SHT4X_TEMP: Mutex<f32> = Mutex::new(-255.0);
pub static SHT4X_HUM: Mutex<f32> = Mutex::new(-255.0);

pub const DETECT_ADDRESSES: &[u8] = &[0x44, 0x45]; // SHT3x et SHT4x adresses par défaut 0x44 et 0x45

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum ShtModel {
    Sht3x,
    Sht4x,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ShtReadings {
    pub temperature: f32,
    pub humidity: f32,
}

pub struct I2cSht {
    pub channel: u8,
    pub address: u8,
    pub is_found: bool,
    pub model: Option<ShtModel>,
}

impl I2cSht {
    pub fn new(channel: u8, address: u8) -> Self {
        Self {
            channel,
            address,
            is_found: false,
            model: None,
        }
    }

    pub fn init(&mut self, driver: &mut I2cDriver<'static>) -> Result<(), anyhow::Error> {
        log::info!("Initializing SHT family sensor at channel {}, address 0x{:02x}...", self.channel, self.address);
        
        // Tester d'abord en tant que SHT4x
        if self.probe_sht4x(driver) {
            self.model = Some(ShtModel::Sht4x);
            self.is_found = true;
            log::info!("Detected SHT4x sensor at channel {}, address 0x{:02x}", self.channel, self.address);
        }
        // Sinon, tester en tant que SHT3x
        else if self.probe_sht3x(driver) {
            self.model = Some(ShtModel::Sht3x);
            self.is_found = true;
            log::info!("Detected SHT3x sensor at channel {}, address 0x{:02x}", self.channel, self.address);
        }
        // Sinon, non trouvé
        else {
            log::warn!("No SHT sensor found at channel {}, address 0x{:02x}", self.channel, self.address);
            self.is_found = false;
        }
        Ok(())
    }

    pub fn detect(&self) -> bool {
        self.is_found
    }

    fn probe_sht4x(&self, driver: &mut I2cDriver<'static>) -> bool {
        if driver.write(self.address, &[0xFD], 50).is_err() {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
        let mut buf = [0u8; 6];
        if driver.read(self.address, &mut buf, 50).is_err() {
            return false;
        }
        check_crc(&[buf[0], buf[1]], buf[2]) && check_crc(&[buf[3], buf[4]], buf[5])
    }

    fn probe_sht3x(&self, driver: &mut I2cDriver<'static>) -> bool {
        if driver.write(self.address, &[0x24, 0x00], 50).is_err() {
            return false;
        }
        thread::sleep(Duration::from_millis(15));
        let mut buf = [0u8; 6];
        if driver.read(self.address, &mut buf, 50).is_err() {
            return false;
        }
        check_crc(&[buf[0], buf[1]], buf[2]) && check_crc(&[buf[3], buf[4]], buf[5])
    }

    pub fn read_value(&mut self, driver: &mut I2cDriver<'static>) -> Option<ShtReadings> {
        if !self.is_found {
            return None;
        }

        let model = self.model.or_else(|| {
            if self.probe_sht4x(driver) {
                self.model = Some(ShtModel::Sht4x);
            } else if self.probe_sht3x(driver) {
                self.model = Some(ShtModel::Sht3x);
            }
            self.model
        });

        match model {
            Some(ShtModel::Sht4x) => {
                if driver.write(self.address, &[0xFD], 50).is_ok() {
                    thread::sleep(Duration::from_millis(10));
                    let mut buf = [0u8; 6];
                    if driver.read(self.address, &mut buf, 50).is_ok() {
                        if check_crc(&[buf[0], buf[1]], buf[2]) && check_crc(&[buf[3], buf[4]], buf[5]) {
                            let t_ticks = ((buf[0] as u32) << 8) | (buf[1] as u32);
                            let rh_ticks = ((buf[3] as u32) << 8) | (buf[4] as u32);
                            
                            let temp = -45.0 + 175.0 * (t_ticks as f32 / 65535.0);
                            let hum = -6.0 + 125.0 * (rh_ticks as f32 / 65535.0);
                            return Some(ShtReadings {
                                temperature: temp,
                                humidity: hum.clamp(0.0, 100.0),
                            });
                        }
                    }
                }
            }
            Some(ShtModel::Sht3x) => {
                if driver.write(self.address, &[0x24, 0x00], 50).is_ok() {
                    thread::sleep(Duration::from_millis(15));
                    let mut buf = [0u8; 6];
                    if driver.read(self.address, &mut buf, 50).is_ok() {
                        if check_crc(&[buf[0], buf[1]], buf[2]) && check_crc(&[buf[3], buf[4]], buf[5]) {
                            let t_ticks = ((buf[0] as u32) << 8) | (buf[1] as u32);
                            let rh_ticks = ((buf[3] as u32) << 8) | (buf[4] as u32);
                            
                            let temp = -45.0 + 175.0 * (t_ticks as f32 / 65535.0);
                            let hum = 100.0 * (rh_ticks as f32 / 65535.0);
                            return Some(ShtReadings {
                                temperature: temp,
                                humidity: hum.clamp(0.0, 100.0),
                            });
                        }
                    }
                }
            }
            None => {}
        }
        None
    }
}

fn check_crc(data: &[u8; 2], checksum: u8) -> bool {
    let mut crc = 0xFFu8;
    for &byte in data {
        crc ^= byte;
        for _ in 0..8 {
            if (crc & 0x80) != 0 {
                crc = (crc << 1) ^ 0x31;
            } else {
                crc <<= 1;
            }
        }
    }
    crc == checksum
}
