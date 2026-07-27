use esp_idf_hal::i2c::I2cDriver;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8};

// `DETECT_ADDRESSES` (type: &[u8]) : Liste des adresses I2C du SCD40/SCD41 (0x62 par défaut chez Sensirion).
pub const DETECT_ADDRESSES: &[u8] = &[0x62];

// Variables statiques globales pour le SCD41
pub static SCD41_FOUND: AtomicBool = AtomicBool::new(false);
pub static SCD41_CO2: Mutex<i32> = Mutex::new(-255);
pub static SCD41_TEMP: Mutex<f32> = Mutex::new(-255.0);
pub static SCD41_HUM: Mutex<f32> = Mutex::new(-255.0);
pub static SCD41_CHANNEL: AtomicU8 = AtomicU8::new(0);
pub static SCD41_ADDR: AtomicU8 = AtomicU8::new(0);

// `sensirion_crc8` (type: fn(&[u8]) -> u8) : Calcule le CRC-8 Sensirion (polynôme 0x31, valeur initiale 0xFF) pour valider l'intégrité des échanges I2C.
fn sensirion_crc8(data: &[u8]) -> u8 {
    let mut crc: u8 = 0xFF;
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
    crc
}

// `Scd41Readings` (type: struct) : Structure contenant les 3 grandeurs mesurées par le capteur SCD40/SCD41.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Scd41Readings {
    // `co2` (type: i32) : Concentration de CO2 en ppm (parties par million). Ex: 400 à 5000 ppm.
    pub co2: i32,
    // `temperature` (type: f32) : Température en degrés Celsius (°C).
    pub temperature: f32,
    // `humidity` (type: f32) : Humidité relative en pourcentage (%).
    pub humidity: f32,
}

// `I2cScd41` (type: struct) : Représente une instance de capteur SCD40/SCD41 connecté sur un canal du multiplexeur I2C.
pub struct I2cScd41 {
    // `channel` (type: u8) : Canal I2C du multiplexeur TCA9548A (0 à 7).
    pub channel: u8,
    // `address` (type: u8) : Adresse I2C du composant (0x62).
    pub address: u8,
    // `is_found` (type: bool) : Indique si le capteur a été détecté et initialisé avec succès.
    pub is_found: bool,
}

impl I2cScd41 {
    // `new` (type: fn(u8, u8) -> Self) : Crée une nouvelle instance de contrôle du capteur SCD41.
    pub fn new(channel: u8, address: u8) -> Self {
        Self {
            channel,
            address,
            is_found: false,
        }
    }

    // `init` (type: fn(&mut self, &mut I2cDriver<'static>) -> Result<(), anyhow::Error>) : Envoie la commande de démarrage de mesure périodique au SCD4x.
    pub fn init(&mut self, driver: &mut I2cDriver<'static>) -> Result<(), anyhow::Error> {
        log::info!("Initializing SCD40/SCD41 CO2 sensor at channel {}, address 0x{:02x}...", self.channel, self.address);
        
        // [Note Dev Junior] : Étape 1 — STOP PERIODIC MEASUREMENT (0x3F86).
        // Obligatoire après un reboot logiciel (le capteur conserve son état s'il reste alimenté).
        // Envoyer stop_periodic_measurement garantit un état de départ connu (idle) quel que soit l'état précédent.
        let cmd_stop = [0x3F, 0x86];
        if let Err(e) = driver.write(self.address, &cmd_stop, 100) {
            log::warn!("SCD4x à 0x{:02x} : échec stop_periodic_measurement (0x3F86) : {:?}", self.address, e);
        } else {
            log::info!("SCD4x à 0x{:02x} : stop_periodic_measurement (0x3F86) OK", self.address);
        }
        // Attendre 500ms après le stop (cf. datasheet Sensirion SCD4x §1.2)
        std::thread::sleep(std::time::Duration::from_millis(500));

        // [Note Dev Junior] : Étape 2 — START PERIODIC MEASUREMENT (0x21B1).
        // Utilisation de la commande universelle `start_periodic_measurement` (0x21B1) au lieu de
        // `start_low_power_periodic_measurement` (0x21AC), car :
        //   - 0x21B1 est supportée par SCD40 ET SCD41
        //   - 0x21AC n'est supportée QUE par SCD41
        // SCD40 : intervalle ≈ 30s | SCD41 : intervalle ≈ 5s (plus de données, consommation ~19 mA)
        let cmd_start_periodic = [0x21, 0xB1];
        if let Err(e) = driver.write(self.address, &cmd_start_periodic, 100) {
            log::warn!("Échec de l'envoi de start_periodic_measurement (0x21B1) au SCD4x à 0x{:02x}: {:?}", self.address, e);
        } else {
            log::info!("Commande start_periodic_measurement (0x21B1) envoyée avec succès au SCD4x à 0x{:02x}", self.address);
        }
        // Attendre 1ms après le start (recommandation datasheet)
        std::thread::sleep(std::time::Duration::from_millis(1));

        self.is_found = true;
        SCD41_FOUND.store(true, std::sync::atomic::Ordering::Relaxed);
        SCD41_CHANNEL.store(self.channel, std::sync::atomic::Ordering::Relaxed);
        SCD41_ADDR.store(self.address, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    // `detect` (type: fn(&self) -> bool) : Retourne true si le capteur a été détecté.
    pub fn detect(&self) -> bool {
        self.is_found
    }

    // `read_value` (type: fn(&mut self, &mut I2cDriver<'static>) -> Option<Scd41Readings>) : Lit les données physiques (CO2, Température, Humidité) du SCD41 via les commandes Sensirion 0xE4B8 (Data Ready) et 0xEC05 (Read Measurement).
    pub fn read_value(&mut self, driver: &mut I2cDriver<'static>) -> Option<Scd41Readings> {
        // Step 1: Interroger la disponibilité des données (Commande 0xE4B8: get_data_ready_status)
        let cmd_get_ready = [0xE4, 0xB8];
        let mut ready_buf = [0u8; 3];
        if driver.write_read(self.address, &cmd_get_ready, &mut ready_buf, 50).is_ok() {
            if sensirion_crc8(&ready_buf[0..2]) == ready_buf[2] {
                let status = ((ready_buf[0] as u16) << 8) | (ready_buf[1] as u16);
                // Les 11 bits de poids faible indiquent si la mesure est prête (> 0)
                if (status & 0x07FF) == 0 {
                    // Donnée pas encore prête lors de ce cycle
                    return None;
                }
            }
        }

        // Step 2: Lire les mesures (Commande 0xEC05: read_measurement)
        // Format du retour (9 octets) :
        // [CO2_MSB, CO2_LSB, CO2_CRC, TEMP_MSB, TEMP_LSB, TEMP_CRC, HUM_MSB, HUM_LSB, HUM_CRC]
        let cmd_read = [0xEC, 0x05];
        let mut buf = [0u8; 9];
        if driver.write_read(self.address, &cmd_read, &mut buf, 100).is_err() {
            return None;
        }

        // Vérification des 3 octets de contrôle CRC-8 Sensirion
        if sensirion_crc8(&buf[0..2]) != buf[2] 
            || sensirion_crc8(&buf[3..5]) != buf[5] 
            || sensirion_crc8(&buf[6..8]) != buf[8] {
            log::warn!("SCD41 (0x{:02x}) : Erreur de checksum CRC-8 lors de la lecture", self.address);
            return None;
        }

        // Conversion brute
        let co2_raw = ((buf[0] as u16) << 8) | (buf[1] as u16);
        let temp_raw = ((buf[3] as u16) << 8) | (buf[4] as u16);
        let hum_raw = ((buf[6] as u16) << 8) | (buf[7] as u16);

        // Formules physiques Sensirion SCD40/SCD41 :
        // CO2 (ppm) = co2_raw
        let co2 = co2_raw as i32;
        // Température (°C) = -45 + 175 * temp_raw / 65536
        let temperature = -45.0 + 175.0 * (temp_raw as f32) / 65536.0;
        // Humidité (%) = 100 * hum_raw / 65536
        let humidity = 100.0 * (hum_raw as f32) / 65536.0;

        // Mise à jour des variables globales pour accès rapide
        if let Ok(mut c) = SCD41_CO2.lock() { *c = co2; }
        if let Ok(mut t) = SCD41_TEMP.lock() { *t = temperature; }
        if let Ok(mut h) = SCD41_HUM.lock() { *h = humidity; }

        Some(Scd41Readings {
            co2,
            temperature,
            humidity,
        })
    }
}

