pub mod one_wire_ds18b20;

use esp_idf_hal::gpio::{PinDriver, InputOutput, Gpio39, Pull};
use esp_idf_hal::delay::Ets;
use esp_idf_sys as sys;
macro_rules! info {
    ($fmt:literal) => {
        log::info!(concat!("\x1b[36m", $fmt, "\x1b[0m"));
    };
    ($fmt:literal, $($arg:tt)*) => {
        log::info!(concat!("\x1b[36m", $fmt, "\x1b[0m"), $($arg)*);
    };
}
macro_rules! warn {
    ($fmt:literal) => {
        log::warn!(concat!("\x1b[36m", $fmt, "\x1b[0m"));
    };
    ($fmt:literal, $($arg:tt)*) => {
        log::warn!(concat!("\x1b[36m", $fmt, "\x1b[0m"), $($arg)*);
    };
}
#[allow(unused_macros)]
macro_rules! debug {
    ($fmt:literal) => {
        log::debug!(concat!("\x1b[36m", $fmt, "\x1b[0m"));
    };
    ($fmt:literal, $($arg:tt)*) => {
        log::debug!(concat!("\x1b[36m", $fmt, "\x1b[0m"), $($arg)*);
    };
}

pub static ONEWIRE_DEVICES_COUNT: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
pub static ONEWIRE_TEMPERATURES: std::sync::Mutex<Option<std::collections::HashMap<String, f32>>> = std::sync::Mutex::new(None);

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

/// Structure représentant l'état de la recherche récursive.
pub struct SearchState {
    rom: [u8; 8],               // ROM en cours de construction
    last_discrepancy: usize,     // Position (1..64) du dernier embranchement pris en 0
    last_device: bool,
}

impl SearchState {
    pub fn new() -> Self {
        Self {
            rom: [0u8; 8],
            last_discrepancy: 0,
            last_device: false,
        }
    }
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
    #[inline(always)]
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
            
            info!(
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
    #[inline(always)]
    pub fn write_bit(&mut self, bit: bool) {
        critical_section::with(|_| {
            if bit {
                // Écriture d'un bit 1
                self.pin.set_low().unwrap();
                Ets::delay_us(6); // Flanc descendant robuste de 6µs
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
    #[inline(always)]
    pub fn read_bit(&mut self) -> bool {
        critical_section::with(|_| {
            self.pin.set_low().unwrap();
            Ets::delay_us(3); // Temps bas de 3µs pour assurer le déclenchement du slot par le capteur
            self.pin.set_high().unwrap();
            Ets::delay_us(10); // Échantillonnage à 13µs total depuis le début du slot (milieu de la fenêtre de 15µs)
            let bit = self.pin.is_high();
            Ets::delay_us(55); // Fin du cycle
            bit
        })
    }

    /// Écrit un octet complet (LSB first).
    #[inline(always)]
    pub fn write_byte(&mut self, mut byte: u8) {
        for _ in 0..8 {
            self.write_bit((byte & 0x01) != 0);
            byte >>= 1;
        }
    }

    /// Lit un octet complet (LSB first).
    #[inline(always)]
    pub fn read_byte(&mut self) -> u8 {
        let mut byte = 0u8;
        for i in 0..8 {
            if self.read_bit() {
                byte |= 1 << i;
            }
        }
        byte
    }

    /// Lecture du scratchpad d'un capteur (retourne les 9 octets bruts).
    /// Appelé après un Reset + Match ROM + 0xBE.
    fn read_scratchpad_raw(&mut self, rom_bytes: &[u8; 8]) -> Option<[u8; 9]> {
        // Laisser impérativement le bus au repos à l'état HAUT pendant 5ms
        // avant d'entamer une nouvelle transaction. Évite les collisions et dysfonctionnements transitoires.
        self.pin.set_high().unwrap();
        Ets::delay_ms(5);

        if !self.reset() {
            return None;
        }
        // Délai de stabilisation après Reset et présence
        Ets::delay_ms(2);

        self.write_byte(0x55); // Match ROM
        for &byte in rom_bytes {
            self.write_byte(byte);
        }
        self.write_byte(0xBE); // Read Scratchpad
        let mut scratchpad = [0u8; 9];
        for i in 0..9 {
            scratchpad[i] = self.read_byte();
        }
        Some(scratchpad)
    }

    /// Vérifie l'authenticité du capteur DS18B20 en lisant ses valeurs par défaut
    /// au démarrage (Power-On Reset) avant toute conversion de température.
    /// Un vrai DS18B20 doit contenir :
    /// - Octets 0 & 1 (Température) : 0x50 et 0x05 (+85.0 °C d'usine)
    /// - Octet 4 (Configuration) : 0x7F (Résolution 12-bits par défaut)
    pub fn verify_authenticity(&mut self, rom: &str) -> bool {
        if rom.len() != 16 {
            return false;
        }
        let mut rom_bytes = [0u8; 8];
        for i in 0..8 {
            if let Ok(b) = u8::from_str_radix(&rom[i * 2..i * 2 + 2], 16) {
                rom_bytes[i] = b;
            } else {
                return false;
            }
        }

        let hex_rom = rom.to_uppercase();
        info!("[1-Wire] Vérification authenticité de la sonde 0x{}...", hex_rom);

        // Lecture du scratchpad à froid (sans lancer de conversion)
        if let Some(scratchpad) = self.read_scratchpad_raw(&rom_bytes) {
            let calculated_crc = calculate_crc8(&scratchpad[0..8]);
            let received_crc = scratchpad[8];

            if calculated_crc != received_crc {
                warn!("[1-Wire]   Sonde 0x{} : Erreur de CRC à froid (calc: 0x{:02x}, recu: 0x{:02x}). Authenticité indéterminée.", hex_rom, calculated_crc, received_crc);
                return false;
            }

            let t_lsb = scratchpad[0];
            let t_msb = scratchpad[1];
            let config = scratchpad[4];

            let is_authentic = t_lsb == 0x50 && t_msb == 0x05 && config == 0x7F;

            if is_authentic {
                info!(
                    "[1-Wire]   Sonde 0x{} : AUTHENTIQUE (Temp d'usine: 0x{:02x} 0x{:02x} (+85C), Config: 0x{:02x} (12-bit))",
                    hex_rom, t_lsb, t_msb, config
                );
            } else {
                warn!(
                    "[1-Wire]   Sonde 0x{} : CLONE / CONTREFAÇON suspectée ! Valeurs à froid : Temp=0x{:02x} 0x{:02x}, Config=0x{:02x} (attendu: 0x50 0x05 et 0x7F)",
                    hex_rom, t_lsb, t_msb, config
                );
            }
            is_authentic
        } else {
            warn!("[1-Wire]   Sonde 0x{} : Impossible de lire le scratchpad à froid.", hex_rom);
            false
        }
    }

    /// Scanne le bus à la recherche de capteurs DS18B20 connectés.
    /// Retourne un vecteur de chaînes hex décrivant les ROMs.
    pub fn search_roms(&mut self) -> Vec<String> {
        info!("[1-Wire Search] Démarrage de la recherche récursive des composants...");
        let mut devices = Vec::new();
        let mut state = SearchState::new();

        // Lancement de la première recherche
        self.search_next(&mut state);

        let mut iterations = 0;
        // Boucle principale : tant qu’on trouve un périphérique valide, on continue
        loop {
            iterations += 1;
            if iterations > 10 {
                warn!("[1-Wire Search] Nombre maximum d'itérations (10) atteint. Arrêt de sécurité.");
                break;
            }

            let rom = state.rom;
            let hex_addr = rom.iter().map(|b| format!("{:02x}", b)).collect::<String>();
            let calculated_crc = calculate_crc8(&rom[0..7]);
            let is_crc_valid = calculated_crc == rom[7];

            if rom[0] == 0x28 && is_crc_valid {
                if !devices.contains(&hex_addr) {
                    info!(
                        "[1-Wire Search] ROM trouvée : 0x{} (CRC: OK) -> DS18B20 ajouté",
                        hex_addr.to_uppercase()
                    );
                    devices.push(hex_addr);
                } else {
                    warn!("[1-Wire Search] Doublon détecté, arrêt de la recherche.");
                    break;
                }
            } else {
                info!(
                    "[1-Wire Search] ROM trouvée : 0x{} (CRC: {}) - Famille 0x{:02x} ignorée",
                    hex_addr.to_uppercase(),
                    if is_crc_valid { "OK" } else { "ERREUR" },
                    rom[0]
                );
            }

            if state.last_device {
                break;
            }

            // Préparer l’état pour la recherche suivante (flip du dernier embranchement)
            self.search_next(&mut state);
        }

        info!("[1-Wire Search] Recherche terminée. {} DS18B20 trouvé(s).", devices.len());
        devices
    }

    /// Effectue un cycle complet de recherche 1-Wire (Reset + Search ROM + parcours des 64 bits).
    fn search_next(&mut self, state: &mut SearchState) {
        critical_section::with(|_| {
            // 1. Reset
            if !self.reset() {
                warn!("[1-Wire Search] Aucune présence détectée pendant le Reset.");
                state.last_device = true;
                return;
            }

            // 2. Commande Search ROM
            self.write_byte(0xF0);

            let mut rom_bit_number = 1;
            let mut discrepancy_marker = 0;

            for byte_idx in 0..8 {
                let mut current_byte = 0u8;
                for bit_idx in 0..8 {
                    // Lecture des deux bits (bit vrai et son complément)
                    let ibit = self.read_bit();
                    let ibit_complement = self.read_bit();

                    if ibit && ibit_complement {
                        // 1,1 -> aucun dispositif sur le bus
                        info!("[1-Wire Search] Pas de réponse au bit {} (1,1). Fin de la branche.", rom_bit_number);
                        state.last_device = true;
                        return;
                    }

                    let direction = if !ibit && !ibit_complement {
                        // Collision (0,0) → divergence
                        if rom_bit_number < state.last_discrepancy {
                            // On suit le chemin mémorisé
                            (state.rom[byte_idx] >> bit_idx) & 0x01
                        } else if rom_bit_number == state.last_discrepancy {
                            // C'est la position à flipper → prendre 1 cette fois
                            1
                        } else {
                            // Nouvelle divergence rencontrée au‑delà de la dernière connue
                            discrepancy_marker = rom_bit_number;
                            0
                        }
                    } else {
                        // Pas de collision : un seul bit valide
                        if ibit { 1 } else { 0 }
                    };

                    if direction == 1 {
                        current_byte |= 1 << bit_idx;
                    }

                    // Écriture du bit de direction
                    self.write_bit(direction != 0);
                    rom_bit_number += 1;
                }
                state.rom[byte_idx] = current_byte;
            }

            // Mise à jour pour la prochaine itération
            state.last_discrepancy = discrepancy_marker;
            state.last_device = discrepancy_marker == 0;

            info!(
                "[1-Wire Search] Fin d’un parcours : last_discrepancy={}, last_device={}",
                state.last_discrepancy, state.last_device
            );
        });
    }

    pub fn start_conversion(&mut self) -> Result<(), anyhow::Error> {
        info!("réveil 1Wire brodcast (Skip ROM (0xCC))");
        if !self.reset() {
            anyhow::bail!("Aucun capteur n'a répondu au Reset avant la commande de conversion");
        }
        self.write_byte(0xCC); // Skip ROM
        self.write_byte(0x44); // Convert T
        Ok(())
    }

    /// Configure la résolution d'un DS18B20 spécifique à 10 bits (0.25°C).
    /// Enregistre cette configuration dans la mémoire EEPROM du DS18B20.
    pub fn configure_resolution_10bit(&mut self, rom: &str) -> Result<(), anyhow::Error> {
        if rom.len() != 16 {
            anyhow::bail!("Invalid ROM length: must be 16 hex characters");
        }
        let mut rom_bytes = [0u8; 8];
        for i in 0..8 {
            rom_bytes[i] = u8::from_str_radix(&rom[i * 2..i * 2 + 2], 16)?;
        }
        let hex_rom = rom.to_uppercase();
        info!("[1-Wire] Configuration de la sonde 0x{} en 10-bits...", hex_rom);

        if !self.reset() {
            anyhow::bail!("Le capteur 0x{} n'a pas répondu au Reset pour configurer la résolution", hex_rom);
        }
        Ets::delay_ms(2);

        // 1. Match ROM
        self.write_byte(0x55);
        for &byte in &rom_bytes {
            self.write_byte(byte);
        }

        // 2. Write Scratchpad (0x4E)
        // Les octets suivants sont : TH (User 1), TL (User 2), Config Register
        self.write_byte(0x4E);
        self.write_byte(0x4B); // TH par défaut
        self.write_byte(0x46); // TL par défaut
        self.write_byte(0x3F); // Configuration : 10-bits (R1=0, R0=1) -> 0x3F (0b00111111)

        // 3. Sauvegarder dans l'EEPROM (Copy Scratchpad 0x48) pour persister après extinction
        if !self.reset() {
            anyhow::bail!("Échec de communication lors de la sauvegarde EEPROM");
        }
        Ets::delay_ms(2);
        self.write_byte(0x55);
        for &byte in &rom_bytes {
            self.write_byte(byte);
        }
        self.write_byte(0x48); // Copy Scratchpad

        // Laisser du temps pour l'écriture EEPROM (10ms minimum)
        Ets::delay_ms(15);
        info!("[1-Wire]   Sonde 0x{} configurée en 10-bits avec succès !", hex_rom);
        Ok(())
    }

    /// Lit le scratchpad d'un DS18B20 spécifique et extrait la température après validation du CRC.
    pub fn read_temperature(&mut self, rom: &str) -> Result<f32, anyhow::Error> {
        if rom.len() != 16 {
            anyhow::bail!("Invalid ROM length: must be 16 hex characters");
        }
        let mut rom_bytes = [0u8; 8];
        for i in 0..8 {
            rom_bytes[i] = u8::from_str_radix(&rom[i * 2..i * 2 + 2], 16)
                .map_err(|e| anyhow::anyhow!("Invalid hex byte in ROM: {:?}", e))?;
        }

        let hex_rom = rom.to_uppercase();
        const MAX_RETRIES: usize = 3;

        for attempt in 1..=MAX_RETRIES {
            info!(
                "[DS18B20] Lecture scratchpad 0x{} (tentative {}/{})",
                hex_rom, attempt, MAX_RETRIES
            );

            let scratchpad = match self.read_scratchpad_raw(&rom_bytes) {
                Some(sp) => sp,
                None => {
                    warn!(
                        "[DS18B20] 0x{} : pas de réponse au Reset (tentative {})",
                        hex_rom, attempt
                    );
                    // Recovery : laisser le bus se stabiliser
                    Ets::delay_ms(10);
                    continue;
                }
            };

            let calculated_crc = calculate_crc8(&scratchpad[0..8]);
            let received_crc = scratchpad[8];

            info!(
                "[DS18B20] Scratchpad 0x{} : [{}] CRC calc=0x{:02x} recu=0x{:02x}",
                hex_rom,
                scratchpad.iter().map(|b| format!("{:02x}", b)).collect::<Vec<String>>().join(" "),
                calculated_crc,
                received_crc
            );

            if calculated_crc != received_crc {
                warn!(
                    "[DS18B20] 0x{} : erreur CRC (attendu 0x{:02x}, recu 0x{:02x}) - tentative {}/{}",
                    hex_rom, calculated_crc, received_crc, attempt, MAX_RETRIES
                );
                // Recovery bus : reset pour purger l'état du capteur
                let _ = self.reset();
                Ets::delay_ms(5);
                continue;
            }

            // CRC OK -> extraction température.
            // Pour 10-bits de résolution :
            // Les bits 0 et 1 (de l'octet LSB) sont indéfinis et doivent être mis à 0.
            let mut lsb = scratchpad[0];
            let msb = scratchpad[1];

            // Configuration :
            let config = scratchpad[4];
            let is_10bit = (config & 0x60) == 0x20; // R1=0, R0=1

            let temp_raw = if is_10bit {
                lsb &= 0xFC; // Masque les bits 0 et 1
                let raw = ((msb as i16) << 8) | (lsb as i16);
                raw
            } else {
                ((msb as i16) << 8) | (lsb as i16)
            };

            let temp_c = (temp_raw as f32) / 16.0;

            if attempt > 1 {
                info!(
                    "[DS18B20] 0x{} : lecture réussie après {} tentatives -> {:.2}C (10-bit: {})",
                    hex_rom, attempt, temp_c, is_10bit
                );
            }

            return Ok(temp_c);
        }

        anyhow::bail!(
            "[DS18B20] 0x{} : échec après {} tentatives (CRC ou Reset)",
            hex_rom, MAX_RETRIES
        )
    }
}
