pub mod i2c_scd41;
pub mod i2c_bme280;
pub mod i2c_sht3x;
pub mod i2c_sht4x;

use esp_idf_hal::i2c::{I2cDriver, I2cConfig, I2C0};
use esp_idf_hal::gpio::{Gpio37, Gpio38};
use std::sync::Mutex;

pub static CHANNEL_POLARITIES: Mutex<[bool; 8]> = Mutex::new([false; 8]);
pub static I2C_DEVICES_COUNT: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub fn scan_i2c_devices() -> Vec<(u8, u8)> {
    if let Ok(i2c) = I2c::init() {
        i2c.found_devices
    } else {
        Vec::new()
    }
}

pub struct I2c {
    pub bme280s: Vec<i2c_bme280::I2cBme280>,
    pub scd41s: Vec<i2c_scd41::I2cScd41>,
    pub sht3xs: Vec<i2c_sht3x::I2cSht3x>,
    pub sht4xs: Vec<i2c_sht4x::I2cSht4x>,
    pub found_devices: Vec<(u8, u8)>,
}

fn get_driver(reversed: bool) -> Result<I2cDriver<'static>, esp_idf_sys::EspError> {
    let config = I2cConfig::new().baudrate(esp_idf_hal::units::FromValueType::kHz(100).into());
    unsafe {
        let i2c = I2C0::steal();
        if reversed {
            let sda = Gpio37::steal();
            let scl = Gpio38::steal();
            I2cDriver::new(i2c, sda, scl, &config)
        } else {
            let sda = Gpio38::steal();
            let scl = Gpio37::steal();
            I2cDriver::new(i2c, sda, scl, &config)
        }
    }
}

fn select_channel(driver: &mut I2cDriver<'static>, channel: u8) -> Result<(), esp_idf_sys::EspError> {
    let control_byte = 1 << channel;
    driver.write(0x74, &[control_byte], 50)
}

fn disable_channels(driver: &mut I2cDriver<'static>) -> Result<(), esp_idf_sys::EspError> {
    driver.write(0x74, &[0x00], 50)
}

impl I2c {
    pub fn init() -> Result<Self, anyhow::Error> {
        log::info!("Initializing I2C bus...");
        let mut i2c = Self {
            bme280s: Vec::new(),
            scd41s: Vec::new(),
            sht3xs: Vec::new(),
            sht4xs: Vec::new(),
            found_devices: Vec::new(),
        };
        
        // Effectuer la détection/le scan dynamique des périphériques au démarrage
        i2c.detect();

        Ok(i2c)
    }

    pub fn detect(&mut self) -> bool {
        log::info!("[I2C] Starting dynamic scan based on submodules detection addresses...");
        let mut found = Vec::new();
        let mut polarities = [false; 8];
        let channels = [0, 1, 2, 3, 4, 7];

        // Vider les anciennes listes
        self.bme280s.clear();
        self.scd41s.clear();
        self.sht3xs.clear();
        self.sht4xs.clear();

        // Adresses à tester
        let target_addresses = [0x44, 0x45, 0x62, 0x76, 0x77];

        for &ch in &channels {
            log::debug!("[I2C SCAN] #{}", ch);

            // 1. Essayer avec la polarité standard (SDA=38, SCL=37)
            if let Ok(mut driver) = get_driver(false) {
                if select_channel(&mut driver, ch).is_ok() {
                    let mut channel_found = false;
                    for &addr in &target_addresses {
                        if driver.write(addr, &[0x00], 50).is_ok() {
                            log::info!("[I2C SCAN] Ch{}: Found I2C device at 0x{:02x} (Standard polarity)", ch, addr);
                            found.push((ch, addr));
                            channel_found = true;
                            self.register_device(ch, addr, &mut driver);
                        }
                    }
                    if channel_found {
                        polarities[ch as usize] = false;
                        let _ = disable_channels(&mut driver);
                        drop(driver);
                        continue;
                    }
                }
                let _ = disable_channels(&mut driver);
                drop(driver);
            }

            // 2. Si rien ne répond en standard, essayer avec la polarité inversée (SDA=37, SCL=38)
            let mut activated = false;
            if let Ok(mut driver) = get_driver(false) {
                if select_channel(&mut driver, ch).is_ok() {
                    activated = true;
                }
                drop(driver);
            }

            if activated {
                if let Ok(mut driver_rev) = get_driver(true) {
                    let mut channel_found = false;
                    for &addr in &target_addresses {
                        if driver_rev.write(addr, &[0x00], 50).is_ok() {
                            log::info!("[I2C SCAN] Ch{}: Found I2C device at 0x{:02x} (Reversed polarity)", ch, addr);
                            found.push((ch, addr));
                            channel_found = true;
                            self.register_device(ch, addr, &mut driver_rev);
                        }
                    }
                    if channel_found {
                        polarities[ch as usize] = true;
                    }
                    drop(driver_rev);
                }

                if let Ok(mut driver) = get_driver(false) {
                    let _ = disable_channels(&mut driver);
                    drop(driver);
                }
            }
        }

        if let Ok(mut lock) = CHANNEL_POLARITIES.lock() {
            *lock = polarities;
        }

        self.found_devices = found;

        let total_count = (self.bme280s.len() + self.scd41s.len() + self.sht3xs.len() + self.sht4xs.len()) as u8;
        I2C_DEVICES_COUNT.store(total_count, std::sync::atomic::Ordering::Relaxed);

        !self.found_devices.is_empty()
    }

    fn register_device(&mut self, channel: u8, addr: u8, driver: &mut I2cDriver<'static>) {
        if i2c_bme280::DETECT_ADDRESSES.contains(&addr) {
            let mut dev = i2c_bme280::I2cBme280::new(channel, addr);
            let _ = dev.init(driver);
            self.bme280s.push(dev);
        } else if i2c_scd41::DETECT_ADDRESSES.contains(&addr) {
            let mut dev = i2c_scd41::I2cScd41::new(channel, addr);
            let _ = dev.init(driver);
            self.scd41s.push(dev);
        } else if i2c_sht3x::DETECT_ADDRESSES.contains(&addr) {
            let mut dev = i2c_sht3x::I2cSht3x::new(channel, addr);
            let _ = dev.init(driver);
            self.sht3xs.push(dev);
        } else if i2c_sht4x::DETECT_ADDRESSES.contains(&addr) {
            let mut dev = i2c_sht4x::I2cSht4x::new(channel, addr);
            let _ = dev.init(driver);
            self.sht4xs.push(dev);
        }
    }

    pub fn read_value(&mut self) -> (Option<i2c_bme280::Bme280Readings>, Option<i2c_scd41::Scd41Readings>, Option<i2c_sht3x::Sht3xReadings>, Option<i2c_sht4x::Sht4xReadings>) {
        let mut bme_res = None;
        let mut scd_res = None;
        let mut sht3_res = None;
        let mut sht4_res = None;

        // Lire le premier BME280 s'il y en a un
        if let Some(bme) = self.bme280s.first_mut() {
            let reversed = CHANNEL_POLARITIES.lock().map(|p| p[bme.channel as usize]).unwrap_or(false);
            
            let mut channel_selected = false;
            if let Ok(mut mux_driver) = get_driver(false) {
                if select_channel(&mut mux_driver, bme.channel).is_ok() {
                    channel_selected = true;
                }
                drop(mux_driver);
            }
            
            if channel_selected {
                if let Ok(mut driver) = get_driver(reversed) {
                    bme_res = bme.read_value(&mut driver);
                    drop(driver);
                }
                
                if let Ok(mut mux_driver) = get_driver(false) {
                    let _ = disable_channels(&mut mux_driver);
                    drop(mux_driver);
                }
            }
        }

        // Lire le SCD41 s'il y en a un
        if let Some(scd) = self.scd41s.first_mut() {
            let reversed = CHANNEL_POLARITIES.lock().map(|p| p[scd.channel as usize]).unwrap_or(false);
            
            let mut channel_selected = false;
            if let Ok(mut mux_driver) = get_driver(false) {
                if select_channel(&mut mux_driver, scd.channel).is_ok() {
                    channel_selected = true;
                }
                drop(mux_driver);
            }
            
            if channel_selected {
                if let Ok(mut driver) = get_driver(reversed) {
                    scd_res = scd.read_value(&mut driver);
                    drop(driver);
                }
                
                if let Ok(mut mux_driver) = get_driver(false) {
                    let _ = disable_channels(&mut mux_driver);
                    drop(mux_driver);
                }
            }
        }

        // Lire le SHT3x s'il y en a un
        if let Some(sht3) = self.sht3xs.first_mut() {
            let reversed = CHANNEL_POLARITIES.lock().map(|p| p[sht3.channel as usize]).unwrap_or(false);
            
            let mut channel_selected = false;
            if let Ok(mut mux_driver) = get_driver(false) {
                if select_channel(&mut mux_driver, sht3.channel).is_ok() {
                    channel_selected = true;
                }
                drop(mux_driver);
            }
            
            if channel_selected {
                if let Ok(mut driver) = get_driver(reversed) {
                    sht3_res = sht3.read_value(&mut driver);
                    drop(driver);
                }
                
                if let Ok(mut mux_driver) = get_driver(false) {
                    let _ = disable_channels(&mut mux_driver);
                    drop(mux_driver);
                }
            }
        }

        // Lire le SHT4x s'il y en a un
        if let Some(sht4) = self.sht4xs.first_mut() {
            let reversed = CHANNEL_POLARITIES.lock().map(|p| p[sht4.channel as usize]).unwrap_or(false);
            
            let mut channel_selected = false;
            if let Ok(mut mux_driver) = get_driver(false) {
                if select_channel(&mut mux_driver, sht4.channel).is_ok() {
                    channel_selected = true;
                }
                drop(mux_driver);
            }
            
            if channel_selected {
                if let Ok(mut driver) = get_driver(reversed) {
                    sht4_res = sht4.read_value(&mut driver);
                    drop(driver);
                }
                
                if let Ok(mut mux_driver) = get_driver(false) {
                    let _ = disable_channels(&mut mux_driver);
                    drop(mux_driver);
                }
            }
        }

        (bme_res, scd_res, sht3_res, sht4_res)
    }
}
