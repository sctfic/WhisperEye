use crate::one_wire::OneWire;
use std::collections::HashMap;

pub struct OneWireDs18b20 {
    pub roms: Vec<String>,
}

impl OneWireDs18b20 {
    pub fn new() -> Self {
        Self { roms: Vec::new() }
    }

    pub fn init(&mut self, bus: &mut OneWire) -> Result<(), anyhow::Error> {
        log::info!("Scanning DS18B20 1-Wire sensors...");
        self.roms = bus.search_roms();
        Ok(())
    }

    pub fn detect(&self) -> bool {
        !self.roms.is_empty()
    }

    pub fn read_value(&mut self, bus: &mut OneWire) -> Option<HashMap<String, f32>> {
        let mut readings = HashMap::new();
        if self.roms.is_empty() {
            return None;
        }

        if bus.start_conversion().is_ok() {
            std::thread::sleep(std::time::Duration::from_millis(750));
            for rom in &self.roms {
                match bus.read_temperature(rom) {
                    Ok(temp) => {
                        readings.insert(rom.clone(), temp);
                    }
                    Err(e) => {
                        log::warn!("Failed to read temperature for probe {}: {:?}", rom, e);
                        readings.insert(rom.clone(), -255.0);
                    }
                }
            }
            Some(readings)
        } else {
            log::warn!("Failed to start temperature conversion on 1-Wire bus");
            for rom in &self.roms {
                readings.insert(rom.clone(), -255.0);
            }
            Some(readings)
        }
    }
}
