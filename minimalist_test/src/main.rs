use esp_idf_hal::gpio::{PinDriver, InputOutput, Gpio39, Pull};
use esp_idf_hal::delay::Ets;
use esp_idf_sys as sys;
use log::{info, warn, error, debug};

// CRC8 Dallas/Maxim : X^8 + X^5 + X^4 + 1 (0x8C)
pub fn calculate_crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &byte in data {
        let mut temp = byte;
        for _ in 0..8 {
            let mix = (crc ^ temp) & 0x01;
            crc >>= 1;
            if mix != 0 {
                crc ^= 0x8C;
            }
            temp >>= 1;
        }
    }
    crc
}

pub struct OneWire<'d> {
    pin: PinDriver<'d, InputOutput>,
}

impl<'d> OneWire<'d> {
    pub fn new(pin: Gpio39<'d>) -> Result<Self, anyhow::Error> {
        info!("[1-Wire] Libération du GPIO 39 (désactivation de la fonction JTAG)...");
        unsafe {
            sys::gpio_reset_pin(39);
        }
        info!("[1-Wire] Initialisation du GPIO 39 en mode InputOutput Open-Drain...");
        let mut pin_driver = PinDriver::input_output_od(pin, Pull::Up)?;
        
        // Initialiser la ligne à l'état haut (repos)
        pin_driver.set_high()?;
        Ets::delay_us(100);
        
        let initial_state = pin_driver.is_high();
        info!(
            "[1-Wire] État initial de la ligne (repos avec pull-up externe de 1kΩ) : {}",
            if initial_state { "HAUT (Correct - 3.3V)" } else { "BAS (Alerte - court-circuit ou absence de pull-up !)" }
        );
        
        Ok(Self { pin: pin_driver })
    }

    /// Effectue un Reset du bus 1-Wire et vérifie la présence d'un composant.
    pub fn reset(&mut self) -> bool {
        critical_section::with(|_| {
            // Ligne au repos doit être haute
            let state_before = self.pin.is_high();
            
            // 1. Tirer la ligne vers le bas
            self.pin.set_low().unwrap();
            Ets::delay_us(480);

            // 2. Libérer la ligne (remonte grâce au pull-up)
            self.pin.set_high().unwrap();
            // Attendre 15 us pour voir la transition de remontée
            Ets::delay_us(15);
            let state_after_15us = self.pin.is_high();
            
            // Attendre encore 50 us (total 65 us depuis la libération) pour lire l'impulsion de présence du capteur (le capteur force à 0)
            Ets::delay_us(50);
            let presence = self.pin.is_low(); // Si c'est bas, le capteur signale sa présence
            
            // Attendre la fin du slot de reset (415 us restants)
            Ets::delay_us(415);
            let state_end = self.pin.is_high();
            
            warn!(
                "[1-Wire Reset] Avant: {}, Transition 15us: {}, Présence (65us): {}, Fin: {}",
                if state_before { "H" } else { "B" },
                if state_after_15us { "H" } else { "B" },
                if presence { "DÉTECTÉE (Bas)" } else { "ABSENTE (Haut)" },
                if state_end { "H" } else { "B" }
            );
            
            presence
        })
    }

    /// Écrit un bit sur le bus 1-Wire.
    pub fn write_bit(&mut self, bit: bool) {
        critical_section::with(|_| {
            if bit {
                // Écriture d'un bit 1
                self.pin.set_low().unwrap();
                Ets::delay_us(6);
                self.pin.set_high().unwrap();
                Ets::delay_us(64);
            } else {
                // Écriture d'un bit 0
                self.pin.set_low().unwrap();
                Ets::delay_us(60);
                self.pin.set_high().unwrap();
                Ets::delay_us(10);
            }
        });
    }

    /// Lit un bit sur le bus 1-Wire.
    pub fn read_bit(&mut self) -> bool {
        critical_section::with(|_| {
            self.pin.set_low().unwrap();
            Ets::delay_us(2); // Flanc descendant de 2µs (détection du slot de lecture par le capteur)
            self.pin.set_high().unwrap();
            Ets::delay_us(8); // Attente de 8µs (total 10µs du slot) pour échantillonner la ligne au milieu de la fenêtre de validité (15µs max)
            let bit = self.pin.is_high();
            Ets::delay_us(60); // Fin du cycle (durée minimale du slot 1-wire : 60µs)
            bit
        })
    }

    /// Écrit un octet complet (LSB first).
    pub fn write_byte(&mut self, mut byte: u8) {
        for _ in 0..8 {
            self.write_bit((byte & 0x01) != 0);
            byte >>= 1;
        }
    }

    /// Lit un octet complet (LSB first).
    pub fn read_byte(&mut self) -> u8 {
        let mut byte = 0u8;
        for i in 0..8 {
            if self.read_bit() {
                byte |= 1 << i;
            }
        }
        byte
    }

    /// Scanne le bus à la recherche de capteurs DS18B20 connectés.
    /// Retourne un vecteur d'adresses ROM représentées sous forme de tableau d'octets `[u8; 8]`.
pub fn search_roms(&mut self) -> Vec<[u8; 8]> {
    info!("[1-Wire Search] Démarrage de la recherche des composants sur le bus (Search ROM 0xF0)...");
    let mut devices = Vec::new();
    let mut last_discrepancy = 0;
    let mut last_device_flag = false;
    let mut rom_no = [0u8; 8]; // ROM en cours de construction

    while !last_device_flag {
        if !self.reset() {
            warn!("[1-Wire Search] Aucun périphérique n'a répondu au Reset.");
            break;
        }

        self.write_byte(0xF0); // Search ROM

        let mut last_zero = 0;
        let mut rom_bit_number = 1;
        let mut discrepancy_marker = 0; // Position de la dernière divergence de CETTE itération

        for byte_idx in 0..8 {
            let mut current_byte = 0u8;
            for bit_idx in 0..8 {
                let ibit = self.read_bit();           // bit réel
                let ibit_complement = self.read_bit(); // complément

                let direction = if ibit && ibit_complement {
                    // 1,1 → pas de dispositif
                    debug!("[1-Wire Search] Pas de réponse au bit {} (bus haut). Fin.", rom_bit_number);
                    return devices;
                } else if !ibit && !ibit_complement {
                    // 0,0 → collision (discrepancy)
                    if rom_bit_number == last_discrepancy {
                        // On reprend exactement là où on s'était arrêté : prendre 1 cette fois
                        1
                    } else if rom_bit_number > last_discrepancy {
                        // Nouvelle divergence : prendre 0, mémoriser
                        discrepancy_marker = rom_bit_number;
                        0
                    } else {
                        // Avant la dernière divergence : suivre le chemin mémorisé dans rom_no
                        (rom_no[byte_idx] >> bit_idx) & 0x01
                    }
                } else {
                    // Pas de collision : bit unique
                    if ibit { 1 } else { 0 }
                };

                if direction == 1 {
                    current_byte |= 1 << bit_idx;
                }

                self.write_bit(direction != 0);
                rom_bit_number += 1;
            }
            rom_no[byte_idx] = current_byte;
        }

        // Mise à jour de l'état pour la prochaine itération
        last_discrepancy = discrepancy_marker;
        if last_discrepancy == 0 {
            last_device_flag = true; // Plus de divergence → dernier périphérique
        }

        let hex_addr = rom_no.iter().map(|b| format!("{:02x}", b)).collect::<String>();
        let calculated_crc = calculate_crc8(&rom_no[0..7]);
        let is_crc_valid = calculated_crc == rom_no[7];

        info!(
            "[1-Wire Search] Périphérique trouvé : ROM 0x{} (Famille: 0x{:02x}, CRC: {})",
            hex_addr.to_uppercase(),
            rom_no[0],
            if is_crc_valid { "VALIDE" } else { "INVALIDE" }
        );

        if rom_no[0] == 0x28 {
            if is_crc_valid {
                if !devices.contains(&rom_no) {
                    devices.push(rom_no);
                } else {
                    warn!("[1-Wire Search] ROM 0x{} déjà présente (doublon anormal).", hex_addr.to_uppercase());
                    // Sécurité : forcer l'arrêt si doublon
                    last_device_flag = true;
                }
            } else {
                warn!(
                    "[1-Wire Search] DS18B20 ROM 0x{} CRC invalide (calc: 0x{:02x}, rcv: 0x{:02x}). Ignoré.",
                    hex_addr.to_uppercase(), calculated_crc, rom_no[7]
                );
            }
        } else {
            info!("[1-Wire Search] Périphérique code famille 0x{:02x} ignoré.", rom_no[0]);
        }
    }

    devices
}

    /// Lance la conversion de température globale pour tous les capteurs (Skip ROM).
    pub fn start_conversion(&mut self) -> Result<(), anyhow::Error> {
        debug!("[DS18B20] Envoi de la commande de conversion globale (Skip ROM 0xCC + Convert T 0x44)...");
        if !self.reset() {
            anyhow::bail!("Aucun capteur n'a répondu au Reset avant la commande de conversion");
        }
        self.write_byte(0xCC); // Skip ROM
        self.write_byte(0x44); // Convert T
        Ok(())
    }

    /// Lit le scratchpad d'un DS18B20 spécifique et extrait la température après validation du CRC.
    pub fn read_temperature(&mut self, rom: &[u8; 8]) -> Result<f32, anyhow::Error> {
        let hex_rom = rom.iter().map(|b| format!("{:02x}", b)).collect::<String>().to_uppercase();
        debug!("[DS18B20] Début de la lecture du scratchpad pour le capteur 0x{}...", hex_rom);
        
        if !self.reset() {
            anyhow::bail!("Le capteur 0x{} n'a pas répondu au Reset avant la lecture", hex_rom);
        }
        
        // Match ROM
        self.write_byte(0x55);
        for &byte in rom {
            self.write_byte(byte);
        }

        // Read Scratchpad
        self.write_byte(0xBE);
        
        // Lire les 9 octets du scratchpad
        let mut scratchpad = [0u8; 9];
        for i in 0..9 {
            scratchpad[i] = self.read_byte();
        }

        // Calculer et vérifier le CRC8 du scratchpad
        let calculated_crc = calculate_crc8(&scratchpad[0..8]);
        let received_crc = scratchpad[8];
        
        debug!(
            "[DS18B20] Scratchpad reçu pour 0x{} : [{}] CRC calculé: 0x{:02x}, CRC reçu: 0x{:02x}",
            hex_rom,
            scratchpad.iter().map(|b| format!("0x{:02x}", b)).collect::<Vec<String>>().join(", "),
            calculated_crc,
            received_crc
        );

        if calculated_crc != received_crc {
            anyhow::bail!(
                "Erreur de CRC scratchpad pour le capteur 0x{} (attendu: 0x{:02x}, reçu: 0x{:02x})",
                hex_rom, calculated_crc, received_crc
            );
        }

        // Extraction de la température (12-bit par défaut, LSB à l'index 0, MSB à l'index 1)
        let lsb = scratchpad[0];
        let msb = scratchpad[1];
        
        // Sign extend sur 16 bits
        let temp_raw = ((msb as i16) << 8) | (lsb as i16);
        // La résolution est de 0.0625 °C (1/16 °C) par bit pour 12-bit
        let temp_c = (temp_raw as f32) / 16.0;

        Ok(temp_c)
    }
}

fn main() -> Result<(), anyhow::Error> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    
    // Forcer le framework log de Rust à laisser passer les messages de debug
    log::set_max_level(log::LevelFilter::Debug);

    unsafe {
        esp_idf_sys::esp_log_level_set(
            b"minimalist_test\0".as_ptr() as *const std::os::raw::c_char,
            esp_idf_sys::esp_log_level_t_ESP_LOG_DEBUG,
        );
    }

    info!("==================================================================");
    info!("   WhisperEye - Firmware de diagnostic DS18B20 (GPIO 39)          ");
    info!("   Version : Pur Rust Bit-Banging avec diagnostics avancés        ");
    info!("   Configuration matérielle : 3 fils, pull-up externe de 1kΩ      ");
    info!("==================================================================");

    // Initialiser les périphériques de l'ESP32
    let peripherals = esp_idf_hal::peripherals::Peripherals::take()?;
    let pin_gpio39 = peripherals.pins.gpio39;

    let mut bus = OneWire::new(pin_gpio39)?;

    loop {
        info!("------------------------------------------------------------------");
        info!("[Main] Lancement d'un cycle de détection sur le bus 1-Wire...");
        
        // Rechercher tous les capteurs connectés
        let active_probes = bus.search_roms();
        
        if active_probes.is_empty() {
            error!("[Main] Aucun capteur DS18B20 valide trouvé sur le bus.");
            info!("[Main] Nouvelle tentative dans 5 secondes...");
            std::thread::sleep(std::time::Duration::from_secs(5));
            continue;
        }

        info!("[Main] {} capteur(s) DS18B20 configuré(s) sur le bus.", active_probes.len());

        // Effectuer 10 lectures consécutives sur ces capteurs avant de refaire une recherche
        for cycle in 1..=10 {
            info!("[Main] --- Cycle de lecture {}/10 ---", cycle);
            
            // 1. Déclencher la conversion globale (Skip ROM + Convert T)
            match bus.start_conversion() {
                Ok(()) => {
                    // Attente de 800 ms pour s'assurer que la conversion 12 bits est prête
                    std::thread::sleep(std::time::Duration::from_millis(800));

                    // 2. Lire la température de chaque capteur détecté
                    for (i, probe_rom) in active_probes.iter().enumerate() {
                        let hex_rom = probe_rom.iter().map(|b| format!("{:02x}", b)).collect::<String>().to_uppercase();
                        match bus.read_temperature(probe_rom) {
                            Ok(temp) => {
                                info!(
                                    "[MESURE] Capteur #{} [ROM: 0x{}] => Température: {:.2} °C",
                                    i + 1, hex_rom, temp
                                );
                            }
                            Err(e) => {
                                error!(
                                    "[MESURE] Échec de la lecture sur le capteur #{} [ROM: 0x{}] : {:?}",
                                    i + 1, hex_rom, e
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("[Main] Échec du déclenchement de la conversion de température : {:?}", e);
                }
            }
            
            // Attendre 3 secondes entre chaque mesure
            std::thread::sleep(std::time::Duration::from_secs(3));
        }

        info!("[Main] Fin de la série de mesures. Retour au scan pour détecter les changements à chaud.");
    }
}
