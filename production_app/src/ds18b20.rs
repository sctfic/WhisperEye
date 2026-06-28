use esp_idf_hal::gpio::{PinDriver, InputOutput, Gpio39, Pull};
use esp_idf_hal::delay::Ets;
use log::info;

pub struct OneWire<'d> {
    pin: PinDriver<'d, InputOutput>,
}

impl<'d> OneWire<'d> {
    pub fn new(pin: Gpio39<'d>) -> Result<Self, anyhow::Error> {
        let mut pin_driver = PinDriver::input_output_od(pin, Pull::Up)?;
        pin_driver.set_high()?;
        Ok(Self { pin: pin_driver })
    }

    pub fn reset(&mut self) -> bool {
        critical_section::with(|_| {
            self.pin.set_low().unwrap();
            Ets::delay_us(480);
            self.pin.set_high().unwrap();
            Ets::delay_us(70);
            let presence = self.pin.is_low();
            Ets::delay_us(410);
            presence
        })
    }

    pub fn write_bit(&mut self, bit: bool) {
        critical_section::with(|_| {
            if bit {
                self.pin.set_low().unwrap();
                Ets::delay_us(6);
                self.pin.set_high().unwrap();
                Ets::delay_us(64);
            } else {
                self.pin.set_low().unwrap();
                Ets::delay_us(60);
                self.pin.set_high().unwrap();
                Ets::delay_us(10);
            }
        });
    }

    pub fn read_bit(&mut self) -> bool {
        critical_section::with(|_| {
            self.pin.set_low().unwrap();
            Ets::delay_us(6);
            self.pin.set_high().unwrap();
            Ets::delay_us(9);
            let bit = self.pin.is_high();
            Ets::delay_us(55);
            bit
        })
    }

    pub fn write_byte(&mut self, mut byte: u8) {
        for _ in 0..8 {
            self.write_bit((byte & 0x01) != 0);
            byte >>= 1;
        }
    }

    #[allow(dead_code)]
    pub fn read_byte(&mut self) -> u8 {
        let mut byte = 0u8;
        for i in 0..8 {
            if self.read_bit() {
                byte |= 1 << i;
            }
        }
        byte
    }

    /// Dynamically scan and find ROM addresses of connected DS18B20 devices.
    /// Returns 16-character hex string representations of their 64-bit ROM IDs.
    pub fn search_roms(&mut self) -> Vec<String> {
        let mut devices = Vec::new();
        let mut last_discrepancy = 0;
        let mut last_device_flag = false;
        let mut rom_no = [0u8; 8];

        let mut attempts = 0;
        while !last_device_flag && attempts < 10 {
            attempts += 1;
            if !self.reset() {
                break;
            }
            self.write_byte(0xF0); // Search ROM command

            let mut last_zero = 0;
            let mut rom_bit_number = 1;

            for byte_idx in 0..8 {
                let mut current_byte = 0u8;
                for bit_idx in 0..8 {
                    let ibit = self.read_bit();
                    let ibit_complement = self.read_bit();

                    let direction = if ibit && ibit_complement {
                        // No response
                        return devices;
                    } else if !ibit && !ibit_complement {
                        // Collision/discrepancy
                        if rom_bit_number == last_discrepancy {
                            1
                        } else if rom_bit_number > last_discrepancy {
                            0
                        } else {
                            (rom_no[byte_idx] >> bit_idx) & 0x01
                        }
                    } else {
                        if ibit { 1 } else { 0 }
                    };

                    if direction == 0 {
                        last_zero = rom_bit_number;
                    }

                    if direction == 1 {
                        current_byte |= 1 << bit_idx;
                    }

                    self.write_bit(direction != 0);
                    rom_bit_number += 1;
                }
                rom_no[byte_idx] = current_byte;
            }

            last_discrepancy = last_zero;
            if last_discrepancy == 0 {
                last_device_flag = true;
            }

            // DS18B20 family code is 0x28
            if rom_no[0] == 0x28 {
                let hex_addr = rom_no.iter().map(|b| format!("{:02x}", b)).collect::<String>();
                devices.push(hex_addr);
            }
        }

        // Do not fallback to mock probes if no physical sensors respond, to let them be marked as absent.
        devices
    }

    /// Lancer la conversion de température globale pour tous les capteurs (Skip ROM)
    pub fn start_conversion(&mut self) -> Result<(), anyhow::Error> {
        if !self.reset() {
            anyhow::bail!("No devices responding on 1-Wire bus");
        }
        self.write_byte(0xCC); // Skip ROM
        self.write_byte(0x44); // Convert T
        Ok(())
    }

    /// Lire la température d'une sonde DS18B20 spécifique en utilisant son adresse ROM (format hex de 16 caractères)
    pub fn read_temperature(&mut self, rom: &str) -> Result<f32, anyhow::Error> {
        if rom.len() != 16 {
            anyhow::bail!("Invalid ROM length: must be 16 hex characters");
        }
        let mut rom_bytes = [0u8; 8];
        for i in 0..8 {
            rom_bytes[i] = u8::from_str_radix(&rom[i * 2..i * 2 + 2], 16)
                .map_err(|e| anyhow::anyhow!("Invalid hex byte in ROM: {:?}", e))?;
        }

        if !self.reset() {
            anyhow::bail!("Sensor not responding on reset");
        }
        
        self.write_byte(0x55); // Match ROM
        for &byte in &rom_bytes {
            self.write_byte(byte);
        }

        self.write_byte(0xBE); // Read Scratchpad
        
        let lsb = self.read_byte();
        let msb = self.read_byte();

        // Calculer la température : 12-bit, résolution 0.0625°C
        let temp_raw = ((msb as i16) << 8) | (lsb as i16);
        let temp_c = (temp_raw as f32) / 16.0;

        Ok(temp_c)
    }
}
