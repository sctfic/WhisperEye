pub mod one_wire_ds18b20;

use esp_idf_hal::gpio::{PinDriver, InputOutput, Gpio39, Pull};
use esp_idf_hal::delay::Ets;
use esp_idf_sys as sys;
use log::{info, warn, debug};

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

            info!(
                "[1-Wire Search] ROM trouvée : 0x{} (CRC: {})",
                hex_addr.to_uppercase(),
                if is_crc_valid { "OK" } else { "ERREUR" }
            );

            if rom[0] == 0x28 && is_crc_valid {
                if !devices.contains(&hex_addr) {
                    info!("[1-Wire Search] DS18B20 ajouté : 0x{}", hex_addr.to_uppercase());
                    devices.push(hex_addr);
                } else {
                    warn!("[1-Wire Search] Doublon détecté, arrêt de la recherche.");
                    break;
                }
            } else if rom[0] != 0x28 {
                info!("[1-Wire Search] Périphérique de famille 0x{:02x} ignoré.", rom[0]);
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
                        debug!("[1-Wire Search] Pas de réponse au bit {} (1,1). Fin de la branche.", rom_bit_number);
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

            debug!(
                "[1-Wire Search] Fin d’un parcours : last_discrepancy={}, last_device={}",
                state.last_discrepancy, state.last_device
            );
        });
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
        debug!("[DS18B20] Début de la lecture du scratchpad pour le capteur 0x{}...", hex_rom);
        
        if !self.reset() {
            anyhow::bail!("Le capteur 0x{} n'a pas répondu au Reset avant la lecture", hex_rom);
        }
        
        // Match ROM
        self.write_byte(0x55);
        for &byte in &rom_bytes {
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
