use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ActuatorsState {
    pub rla: bool,
    pub rlb: bool,
    pub swpwr: bool,
    pub ina: bool,
    pub inb: bool,
}

impl Default for ActuatorsState {
    fn default() -> Self {
        Self {
            rla: false,
            rlb: false,
            swpwr: true, // Keep system 5V/3V powered by default
            ina: false,
            inb: false,
        }
    }
}
