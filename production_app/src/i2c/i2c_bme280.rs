use esp_idf_hal::i2c::I2cDriver;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8};

pub const DETECT_ADDRESSES: &[u8] = &[0x76, 0x77];

// Variables globales statiques déplacées de i2c_bus.rs
pub static BME280_FOUND: AtomicBool = AtomicBool::new(false);
pub static BME280_TEMP: Mutex<f32> = Mutex::new(-255.0);
pub static BME280_HUM: Mutex<f32> = Mutex::new(-255.0);
pub static BME280_PRESS: Mutex<f32> = Mutex::new(-255.0);
pub static BME280_CHANNEL: AtomicU8 = AtomicU8::new(0);
pub static BME280_ADDR: AtomicU8 = AtomicU8::new(0);

#[derive(Debug, Clone, serde::Serialize)]
pub struct Bme280Readings {
    pub temperature: f32,
    pub humidity: f32,
    pub pressure: f32,
}

pub struct I2cBme280 {
    pub channel: u8,
    pub address: u8,
    /// `is_bmp280` (type: bool) : true si le capteur est un BMP280 (pas d'humidité physique, Chip ID 0x58).
    pub is_bmp280: bool,
}

impl I2cBme280 {
    pub fn new(channel: u8, address: u8) -> Self {
        Self { channel, address, is_bmp280: false }
    }

    // `init` (type: fn(&mut self, &mut I2cDriver<'static>) -> Result<(), anyhow::Error>) : Initialise le capteur BME280/BMP280 en lisant le Chip ID et en configurant les registres de contrôle.
    pub fn init(&mut self, driver: &mut I2cDriver<'static>) -> Result<(), anyhow::Error> {
        log::info!("Initializing BME280 at channel {}, address 0x{:02x}...", self.channel, self.address);
        
        // `chip_id` (type: [u8; 1]) : Registre 0xD0 contenant l'identifiant du composant (0x60 pour BME280, 0x58 pour BMP280).
        let mut chip_id = [0u8; 1];
        if driver.write_read(self.address, &[0xD0], &mut chip_id, 50).is_ok() {
            if chip_id[0] == 0x58 {
                log::warn!("Capteur détecté à 0x{:02x} est un BMP280 (Température + Pression uniquement, PAS d'humidité physique !)", self.address);
                self.is_bmp280 = true;
            } else if chip_id[0] == 0x60 {
                log::info!("Capteur BME280 (Temp + Pres + Hum) confirmé à l'adresse 0x{:02x} (Chip ID: 0x60)", self.address);
                self.is_bmp280 = false;
            } else {
                log::info!("Capteur I2C à 0x{:02x} a pour Chip ID : 0x{:02x}", self.address, chip_id[0]);
            }
        }

        // [Note Dev Junior] : La spécification Bosch indique qu'un changement dans `ctrl_hum` (0xF2)
        // ne prend effet qu'APRÈS une écriture dans `ctrl_meas` (0xF4).
        // 0xF2 = 0x01 (oversampling humidité x1)
        let _ = driver.write(self.address, &[0xF2, 0x01], 50);
        // 0xF4 = 0x27 (oversampling temp x1, pres x1, mode normal)
        let _ = driver.write(self.address, &[0xF4, 0x27], 50);

        BME280_FOUND.store(true, std::sync::atomic::Ordering::Relaxed);
        BME280_CHANNEL.store(self.channel, std::sync::atomic::Ordering::Relaxed);
        BME280_ADDR.store(self.address, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    pub fn detect(&self) -> bool {
        BME280_FOUND.load(std::sync::atomic::Ordering::Relaxed)
    }

    // `read_value` (type: fn(&mut self, &mut I2cDriver<'static>) -> Option<Bme280Readings>) : Lit les registres de calibration et de mesure bruts, applique les formules de compensation Bosch BME280 et retourne la température, pression et humidité.
    pub fn read_value(&mut self, driver: &mut I2cDriver<'static>) -> Option<Bme280Readings> {
        // `calib_data1` (type: [u8; 24]) : Octets de calibration T1..T3 et P1..P9 (registres 0x88..0xA1).
        let mut calib_data1 = [0u8; 24];
        if driver.write_read(self.address, &[0x88], &mut calib_data1, 50).is_err() {
            return None;
        }
        
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

        // `dig_h1` (type: [u8; 1]) : Coefficient de calibration H1 (registre 0xA1).
        // [Note Dev Junior] : Pour un BMP280 (Chip ID 0x58), les registres d'humidité n'existent pas physiquement.
        // On saute la lecture des registres de calibration humidité et on renvoie -255.0.
        let (mut dig_h1, mut dig_h2, mut dig_h3, mut dig_h4, mut dig_h5, mut dig_h6) = 
            ([0u8; 1], 0i16, 0u8, 0i16, 0i16, 0i8);
        
        if !self.is_bmp280 {
            if driver.write_read(self.address, &[0xA1], &mut dig_h1, 50).is_err() {
                return None;
            }
            
            let mut calib_data2 = [0u8; 7];
            if driver.write_read(self.address, &[0xE1], &mut calib_data2, 50).is_err() {
                return None;
            }
            dig_h2 = i16::from_le_bytes([calib_data2[0], calib_data2[1]]);
            dig_h3 = calib_data2[2];
            dig_h4 = ((calib_data2[3] as i8 as i16) << 4) | ((calib_data2[4] & 0x0F) as i16);
            dig_h5 = ((calib_data2[5] as i8 as i16) << 4) | (((calib_data2[4] & 0xF0) >> 4) as i16);
            dig_h6 = calib_data2[6] as i8;
        }


        // Configuration du mode de mesure :
        // 0xF2 (ctrl_hum) = 0x01 (oversampling humidité x1)
        // 0xF4 (ctrl_meas) = 0x25 (oversampling temp x1, pres x1, mode Forcé 0x01)
        if driver.write(self.address, &[0xF2, 0x01], 50).is_err() { return None; }
        if driver.write(self.address, &[0xF4, 0x25], 50).is_err() { return None; }
        
        // Attente de la fin de la conversion (15ms)
        std::thread::sleep(std::time::Duration::from_millis(15));
        
        // `raw_data` (type: [u8; 8]) : Données brutes lues à partir du registre 0xF7 (Press 3B, Temp 3B, Hum 2B).
        let mut raw_data = [0u8; 8];
        if driver.write_read(self.address, &[0xF7], &mut raw_data, 50).is_err() {
            return None;
        }
        
        let adc_p = ((raw_data[0] as i32) << 12) | ((raw_data[1] as i32) << 4) | ((raw_data[2] as i32) >> 4);
        let adc_t = ((raw_data[3] as i32) << 12) | ((raw_data[4] as i32) << 4) | ((raw_data[5] as i32) >> 4);
        // `adc_h` (type: i32) : Pour BMP280, les registres 6-7 n'existent pas, on force -1.
        let adc_h = if self.is_bmp280 { -1 } else { ((raw_data[6] as i32) << 8) | (raw_data[7] as i32) };

        // Compensation de la Température
        let var1 = (((adc_t >> 3) - ((dig_t1 as i32) << 1)) * (dig_t2 as i32)) >> 11;
        let var2 = (((((adc_t >> 4) - (dig_t1 as i32)) * ((adc_t >> 4) - (dig_t1 as i32))) >> 12) * (dig_t3 as i32)) >> 14;
        let t_fine = var1 + var2;
        let temperature = ((t_fine * 5 + 128) >> 8) as f32 / 100.0;

        // Compensation de la Pression
        let mut var1_p = (t_fine as f64 / 2.0) - 64000.0;
        let mut var2_p = var1_p * var1_p * (dig_p6 as f64) / 32768.0;
        var2_p = var2_p + var1_p * (dig_p5 as f64) * 2.0;
        var2_p = (var2_p / 4.0) + ((dig_p4 as f64) * 65536.0);
        var1_p = ((dig_p3 as f64) * var1_p * var1_p / 524288.0 + (dig_p2 as f64) * var1_p) / 524288.0;
        var1_p = (1.0 + var1_p / 32768.0) * (dig_p1 as f64);
        
        let pressure = if var1_p == 0.0 {
            return None;
        } else {
            let mut p = 1048576.0 - adc_p as f64;
            p = ((p - (var2_p / 4096.0)) * 6250.0) / var1_p;
            let var1_p_final = (dig_p9 as f64) * p * p / 2147483648.0;
            let var2_p_final = p * (dig_p8 as f64) / 32768.0;
            p = p + (var1_p_final + var2_p_final + (dig_p7 as f64)) / 16.0;
            p as f32 / 100.0
        };

        // Compensation de l'Humidité (uniquement pour BME280, pas BMP280)
        let humidity = if self.is_bmp280 {
            -255.0
        } else {
            let mut h = (t_fine as f64) - 76800.0;
            h = (adc_h as f64 - (((dig_h4 as f64) * 64.0) + ((dig_h5 as f64) / 16384.0) * h)) *
                ((dig_h2 as f64) / 65536.0 * (1.0 + (dig_h6 as f64) / 67108864.0 * h *
                (1.0 + (dig_h3 as f64) / 67108864.0 * h)));
            h = h * (1.0 - (dig_h1[0] as f64) * h / 524288.0);
            if h > 100.0 { 100.0 } else if h < 0.0 { 0.0 } else { h as f32 }
        };

        // Sauvegarder dans les variables globales pour la compatibilité
        if let Ok(mut t) = BME280_TEMP.lock() { *t = temperature; }
        if let Ok(mut hu) = BME280_HUM.lock() { *hu = humidity; }
        if let Ok(mut pr) = BME280_PRESS.lock() { *pr = pressure; }

        Some(Bme280Readings {
            temperature,
            humidity,
            pressure,
        })
    }
}
