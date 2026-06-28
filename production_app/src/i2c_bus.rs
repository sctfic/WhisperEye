use esp_idf_hal::i2c::{I2cDriver, I2cConfig};
use esp_idf_hal::gpio::{Gpio37, Gpio38};
use esp_idf_hal::i2c::I2C0;
use std::sync::Mutex;

// Stockage des polarités détectées (true = inversé, false = standard)
pub static CHANNEL_POLARITIES: Mutex<[bool; 8]> = Mutex::new([false; 8]);
pub static BME280_FOUND: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub static BME280_TEMP: Mutex<f32> = Mutex::new(-255.0);
pub static BME280_HUM: Mutex<f32> = Mutex::new(-255.0);
pub static BME280_PRESS: Mutex<f32> = Mutex::new(-255.0);
pub static BME280_VERSION: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
pub static BME280_CHANNEL: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
pub static BME280_ADDR: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

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

/// Scan channels of the TCA954 I2C Multiplexer.
/// Returns a list of (channel, i2c_address).
pub fn scan_i2c_devices() -> Vec<(u8, u8)> {
    println!("[I2C SCAN] Starting dynamic I2C scan behind TCA9548A multiplexer...");
    let mut found = Vec::new();
    let mut polarities = [false; 8];
    let channels = [0, 1, 2, 3, 4, 7];

    for &ch in &channels {
        println!("[I2C SCAN] --- Scanning channel {} ---", ch);

        // 1. Essayer avec la polarité standard (SDA=38, SCL=37)
        println!("[I2C SCAN] Ch{}: Creating Standard driver (SDA=38, SCL=37)...", ch);
        match get_driver(false) {
            Ok(mut driver) => {
                println!("[I2C SCAN] Ch{}: Standard driver created. Selecting channel...", ch);
                match select_channel(&mut driver, ch) {
                    Ok(()) => {
                        println!("[I2C SCAN] Ch{}: Multiplexer channel selected under Standard polarity. Probing...", ch);
                        let mut channel_found = false;
                        for &addr in &[0x44, 0x62, 0x76, 0x77] {
                            println!("[I2C SCAN] Ch{}: Probing address 0x{:02x} (Standard polarity)...", ch, addr);
                            match driver.write(addr, &[0x00], 50) {
                                Ok(()) => {
                                    println!("[I2C SCAN] Ch{}: Found I2C device at 0x{:02x} (Standard polarity)", ch, addr);
                                    if addr == 0x76 || addr == 0x77 {
                                        BME280_FOUND.store(true, std::sync::atomic::Ordering::Relaxed);
                                        BME280_CHANNEL.store(ch, std::sync::atomic::Ordering::Relaxed);
                                        BME280_ADDR.store(addr, std::sync::atomic::Ordering::Relaxed);
                                    }
                                    found.push((ch, addr));
                                    channel_found = true;
                                }
                                Err(e) => {
                                    println!("[I2C SCAN] Ch{}: No response from address 0x{:02x} (Standard polarity): {:?}", ch, addr, e);
                                }
                            }
                        }
                        if channel_found {
                            polarities[ch as usize] = false;
                            println!("[I2C SCAN] Ch{}: Device(s) found. Disabling channels...", ch);
                            let _ = disable_channels(&mut driver);
                            println!("[I2C SCAN] Ch{}: Dropping Standard driver...", ch);
                            drop(driver);
                            continue;
                        }
                    }
                    Err(e) => {
                        println!("[I2C SCAN] Ch{}: Failed to select channel under Standard polarity: {:?}", ch, e);
                    }
                }
                println!("[I2C SCAN] Ch{}: Disabling channels on Standard driver...", ch);
                let _ = disable_channels(&mut driver);
                println!("[I2C SCAN] Ch{}: Dropping Standard driver...", ch);
                drop(driver);
            }
            Err(e) => {
                println!("[I2C SCAN] Ch{}: Failed to create Standard driver: {:?}", ch, e);
            }
        }

        // 2. Si rien ne répond en standard, essayer avec la polarité inversée (SDA=37, SCL=38)
        let mut activated = false;
        println!("[I2C SCAN] Ch{}: Re-creating Standard driver to enable channel...", ch);
        match get_driver(false) {
            Ok(mut driver) => {
                println!("[I2C SCAN] Ch{}: Enabling channel {}...", ch, ch);
                if select_channel(&mut driver, ch).is_ok() {
                    println!("[I2C SCAN] Ch{}: Channel enabled under Standard polarity.", ch);
                    activated = true;
                }
                println!("[I2C SCAN] Ch{}: Dropping Standard driver...", ch);
                drop(driver);
            }
            Err(e) => {
                println!("[I2C SCAN] Ch{}: Failed to create Standard driver: {:?}", ch, e);
            }
        }

        if activated {
            println!("[I2C SCAN] Ch{}: Creating Reversed driver (SDA=37, SCL=38)...", ch);
            match get_driver(true) {
                Ok(mut driver_rev) => {
                    println!("[I2C SCAN] Ch{}: Reversed driver created. Probing...", ch);
                    let mut channel_found = false;
                    for &addr in &[0x44, 0x62, 0x76, 0x77] {
                        println!("[I2C SCAN] Ch{}: Probing address 0x{:02x} (Reversed polarity)...", ch, addr);
                        match driver_rev.write(addr, &[0x00], 50) {
                            Ok(()) => {
                                println!("[I2C SCAN] Ch{}: Found I2C device at 0x{:02x} (Reversed polarity)", ch, addr);
                                if addr == 0x76 || addr == 0x77 {
                                    BME280_FOUND.store(true, std::sync::atomic::Ordering::Relaxed);
                                    BME280_CHANNEL.store(ch, std::sync::atomic::Ordering::Relaxed);
                                    BME280_ADDR.store(addr, std::sync::atomic::Ordering::Relaxed);
                                }
                                found.push((ch, addr));
                                channel_found = true;
                            }
                            Err(e) => {
                                println!("[I2C SCAN] Ch{}: No response from address 0x{:02x} (Reversed polarity): {:?}", ch, addr, e);
                            }
                        }
                    }
                    if channel_found {
                        polarities[ch as usize] = true;
                    }
                    println!("[I2C SCAN] Ch{}: Dropping Reversed driver...", ch);
                    drop(driver_rev);
                }
                Err(e) => {
                    println!("[I2C SCAN] Ch{}: Failed to create Reversed driver: {:?}", ch, e);
                }
            }

            println!("[I2C SCAN] Ch{}: Restoring Standard driver for multiplexer cleanup...", ch);
            match get_driver(false) {
                Ok(mut driver) => {
                    let _ = disable_channels(&mut driver);
                    drop(driver);
                }
                Err(e) => {
                    println!("[I2C SCAN] Ch{}: Failed to restore Standard driver: {:?}", ch, e);
                }
            }
        }
    }

    // Sauvegarder les polarités détectées
    if let Ok(mut lock) = CHANNEL_POLARITIES.lock() {
        *lock = polarities;
    }

    println!("[I2C SCAN] Dynamic I2C scan complete. Found: {:?}", found);
    found
}

fn read_registers(driver: &mut I2cDriver<'static>, addr: u8, reg: u8, data: &mut [u8]) -> Result<(), esp_idf_sys::EspError> {
    driver.write_read(addr, &[reg], data, 50)
}

fn write_register(driver: &mut I2cDriver<'static>, addr: u8, reg: u8, val: u8) -> Result<(), esp_idf_sys::EspError> {
    driver.write(addr, &[reg, val], 50)
}

pub fn read_bme280_hardware() -> Result<(f32, f32, f32), i32> {
    if !BME280_FOUND.load(std::sync::atomic::Ordering::Relaxed) {
        return Err(-255); // capteur absent
    }
    
    let ch = BME280_CHANNEL.load(std::sync::atomic::Ordering::Relaxed);
    let addr = BME280_ADDR.load(std::sync::atomic::Ordering::Relaxed);
    
    let reversed = {
        if let Ok(polarities) = CHANNEL_POLARITIES.lock() {
            polarities[ch as usize]
        } else {
            false
        }
    };
    
    // 1. Sélectionner le canal sur le multiplexeur
    {
        let mut mux_driver = get_driver(false).map_err(|_| -254)?;
        select_channel(&mut mux_driver, ch).map_err(|_| -254)?;
    }
    
    // 2. Communiquer avec le BME280
    let result = (|| -> Result<(f32, f32, f32), i32> {
        let mut bme_driver = get_driver(reversed).map_err(|_| -254)?;
        
        let mut calib_data1 = [0u8; 24];
        read_registers(&mut bme_driver, addr, 0x88, &mut calib_data1).map_err(|_| -254)?;
        
        let dig_t1 = u16::from_le_bytes([calib_data1[0], calib_data1[1]]);
        let dig_t2 = i16::from_le_bytes([calib_data1[2], calib_data1[3]]);
        let dig_t3 = i16::from_le_bytes([calib_data1[4], calib_data1[5]]);
        let dig_p1 = u16::from_le_bytes([calib_data1[6], calib_data1[7]]);
        let dig_p2 = i16::from_le_bytes([calib_data1[8], calib_data1[9]]);
        let dig_p3 = i16::from_le_bytes([calib_data1[10], calib_data1[11]]);
        let dig_p4 = i16::from_le_bytes([calib_data1[12], calib_data1[13]]);
        let dig_p5 = i16::from_le_bytes([calib_data1[14], calib_data1[15]]);
        let dig_p6 = i16::from_le_bytes([calib_data1[16], calib_data1[17]]);
        let dig_p7 = i16::from_le_bytes([calib_data1[18], calib_data1[19]]);
        let dig_p8 = i16::from_le_bytes([calib_data1[20], calib_data1[21]]);
        let dig_p9 = i16::from_le_bytes([calib_data1[22], calib_data1[23]]);

        let mut dig_h1 = [0u8; 1];
        read_registers(&mut bme_driver, addr, 0xA1, &mut dig_h1).map_err(|_| -254)?;
        
        let mut calib_data2 = [0u8; 7];
        read_registers(&mut bme_driver, addr, 0xE1, &mut calib_data2).map_err(|_| -254)?;
        let dig_h2 = i16::from_le_bytes([calib_data2[0], calib_data2[1]]);
        let dig_h3 = calib_data2[2];
        let dig_h4 = ((calib_data2[3] as i16) << 4) | ((calib_data2[4] & 0x0F) as i16);
        let dig_h5 = ((calib_data2[5] as i16) << 4) | (((calib_data2[4] & 0xF0) >> 4) as i16);
        let dig_h6 = calib_data2[6] as i8;

        write_register(&mut bme_driver, addr, 0xF2, 0x01).map_err(|_| -254)?;
        write_register(&mut bme_driver, addr, 0xF4, 0x27).map_err(|_| -254)?;
        
        std::thread::sleep(std::time::Duration::from_millis(15));
        
        let mut raw_data = [0u8; 8];
        read_registers(&mut bme_driver, addr, 0xF7, &mut raw_data).map_err(|_| -254)?;
        
        let adc_p = ((raw_data[0] as i32) << 12) | ((raw_data[1] as i32) << 4) | ((raw_data[2] as i32) >> 4);
        let adc_t = ((raw_data[3] as i32) << 12) | ((raw_data[4] as i32) << 4) | ((raw_data[5] as i32) >> 4);
        let adc_h = ((raw_data[6] as i32) << 8) | (raw_data[7] as i32);

        let var1 = (((adc_t >> 3) - ((dig_t1 as i32) << 1)) * (dig_t2 as i32)) >> 11;
        let var2 = (((((adc_t >> 4) - (dig_t1 as i32)) * ((adc_t >> 4) - (dig_t1 as i32))) >> 12) * (dig_t3 as i32)) >> 14;
        let t_fine = var1 + var2;
        let temperature = ((t_fine * 5 + 128) >> 8) as f32 / 100.0;

        let mut var1_p = (t_fine as f64 / 2.0) - 64000.0;
        let mut var2_p = var1_p * var1_p * (dig_p6 as f64) / 32768.0;
        var2_p = var2_p + var1_p * (dig_p5 as f64) * 2.0;
        var2_p = (var2_p / 4.0) + ((dig_p4 as f64) * 65536.0);
        var1_p = ((dig_p3 as f64) * var1_p * var1_p / 524288.0 + (dig_p2 as f64) * var1_p) / 524288.0;
        var1_p = (1.0 + var1_p / 32768.0) * (dig_p1 as f64);
        
        let pressure = if var1_p == 0.0 {
            return Err(-253);
        } else {
            let mut p = 1048576.0 - adc_p as f64;
            p = ((p - (var2_p / 4096.0)) * 6250.0) / var1_p;
            let var1_p_final = (dig_p9 as f64) * p * p / 2147483648.0;
            let var2_p_final = p * (dig_p8 as f64) / 32768.0;
            p = p + (var1_p_final + var2_p_final + (dig_p7 as f64)) / 16.0;
            p as f32 / 100.0
        };

        let mut h = (t_fine as f64) - 76800.0;
        h = (adc_h as f64 - (((dig_h4 as f64) * 64.0) + ((dig_h5 as f64) / 16384.0) * h)) *
            ((dig_h2 as f64) / 65536.0 * (1.0 + (dig_h6 as f64) / 67108864.0 * h *
            (1.0 + (dig_h3 as f64) / 67108864.0 * h)));
        h = h * (1.0 - (dig_h1[0] as f64) * h / 524288.0);
        let humidity = if h > 100.0 {
            100.0
        } else if h < 0.0 {
            0.0
        } else {
            h as f32
        };

        Ok((temperature, humidity, pressure))
    })();

    // 3. Désélectionner le canal
    if let Ok(mut mux_driver) = get_driver(false) {
        let _ = disable_channels(&mut mux_driver);
    }
    
    result
}
