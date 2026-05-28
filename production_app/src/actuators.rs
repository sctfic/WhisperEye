use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ActuatorsState {
    pub relay_1: bool,
    pub pwm_intensity: u32,
}

impl Default for ActuatorsState {
    fn default() -> Self {
        Self {
            relay_1: false,
            pwm_intensity: 0,
        }
    }
}
