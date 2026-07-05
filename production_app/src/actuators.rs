use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use esp_idf_hal::gpio::*;
use esp_idf_hal::ledc::*;
use esp_idf_hal::units::*;

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

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ScheduledAction {
    pub datetime_utc: String, // format YYYY-MM-DDTHH:MM:SSZ
    pub state: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ScheduledActions {
    // Clé: "rla", "rlb", "swpwr", "ina", "inb"
    pub schedules: HashMap<String, Vec<ScheduledAction>>,
}

impl ScheduledActions {
    pub fn add_schedule(&mut self, actuator_id: &str, datetime_utc: String, state: bool) -> Result<(), String> {
        let list = self.schedules.entry(actuator_id.to_string()).or_insert_with(Vec::new);
        
        // Validation : max 3 planifications par actionneur
        if list.len() >= 3 {
            return Err("Limite de planifications atteinte (max 3 par actionneur).".to_string());
        }

        // Éliminer les doublons pour le même timestamp exact
        if list.iter().any(|s| s.datetime_utc == datetime_utc) {
            return Err("Une planification existe déjà à cette date et heure exacte.".to_string());
        }

        list.push(ScheduledAction { datetime_utc, state });
        
        // Trier par ordre chronologique
        list.sort_by(|a, b| a.datetime_utc.cmp(&b.datetime_utc));

        Ok(())
    }
}

// --- Matériel Actionneurs ---

pub struct MotorPwmPin {
    driver: LedcDriver<'static>,
    active: bool,
    speed_pct: i32,
}

impl MotorPwmPin {
    pub fn get_speed(&self) -> i32 {
        self.speed_pct
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn new(driver: LedcDriver<'static>) -> Self {
        Self {
            driver,
            active: false,
            speed_pct: 20, // Par défaut 20%
        }
    }

    pub fn set_level(&mut self, level: Level) -> Result<(), esp_idf_hal::sys::EspError> {
        self.active = match level {
            Level::High => true,
            Level::Low => false,
        };
        self.update_duty()
    }

    pub fn set_speed(&mut self, speed_pct: i32) -> Result<(), esp_idf_hal::sys::EspError> {
        self.speed_pct = speed_pct;
        self.update_duty()
    }

    fn update_duty(&mut self) -> Result<(), esp_idf_hal::sys::EspError> {
        let max_duty = self.driver.get_max_duty();
        let target_duty = if self.active {
            (max_duty as u64 * self.speed_pct as u64 / 100) as u32
        } else {
            0
        };
        self.driver.set_duty(target_duty)
    }
}

pub struct Actuators {
    pub relay_a: PinDriver<'static, Output>,
    pub relay_b: PinDriver<'static, Output>,
    pub sw_pwr: PinDriver<'static, Output>,
    pub ina: MotorPwmPin,
    pub inb: MotorPwmPin,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActuatorsPresence {
    pub rla: bool,
    pub rlb: bool,
    pub swpwr: bool,
    pub ina: bool,
    pub inb: bool,
}

impl Actuators {
    pub fn init(
        gpio9: Gpio9<'static>,
        gpio47: Gpio47<'static>,
        gpio21: Gpio21<'static>,
        gpio36: Gpio36<'static>,
        gpio35: Gpio35<'static>,
    ) -> Result<Self, anyhow::Error> {
        log::info!("Initializing actuators (RLA, RLB, INA, INB, SWPWR)...");

        let mut relay_a = PinDriver::output(gpio9)?;
        relay_a.set_low()?;

        let mut relay_b = PinDriver::output(gpio47)?;
        relay_b.set_low()?;

        let mut sw_pwr = PinDriver::output(gpio21)?;
        sw_pwr.set_high()?; // Garder le système alimenté par défaut

        // Configurer le PWM à 10 kHz pour les moteurs
        let timer1 = unsafe { TIMER1::steal() };
        let timer2 = unsafe { TIMER2::steal() };
        let channel1 = unsafe { CHANNEL1::steal() };
        let channel2 = unsafe { CHANNEL2::steal() };

        let timer_config = config::TimerConfig::new().frequency(10.kHz().into());
        let timer_driver1 = LedcTimerDriver::new(timer1, &timer_config)?;
        let timer_driver2 = LedcTimerDriver::new(timer2, &timer_config)?;

        let ledc_ina = LedcDriver::new(channel1, timer_driver1, gpio36)?;
        let ledc_inb = LedcDriver::new(channel2, timer_driver2, gpio35)?;

        let mut ina = MotorPwmPin::new(ledc_ina);
        let mut inb = MotorPwmPin::new(ledc_inb);

        ina.set_level(Level::Low)?;
        inb.set_level(Level::Low)?;

        Ok(Self {
            relay_a,
            relay_b,
            sw_pwr,
            ina,
            inb,
        })
    }

    pub fn detect(&self, vsense_volts: Option<f32>) -> ActuatorsPresence {
        // RLA, RLB et SWPWR sont toujours considérés comme présents.
        // INA et INB ne sont détectés/présents que si vsense > 6.0V.
        let is_high_voltage = vsense_volts.map_or(false, |v| v > 6.0);
        ActuatorsPresence {
            rla: true,
            rlb: true,
            swpwr: true,
            ina: is_high_voltage,
            inb: is_high_voltage,
        }
    }

    pub fn write(&mut self, id: &str, state: bool) -> Result<(), esp_idf_hal::sys::EspError> {
        match id {
            "rla" => self.relay_a.set_level(state.into()),
            "rlb" => self.relay_b.set_level(state.into()),
            "swpwr" => self.sw_pwr.set_level(state.into()),
            "ina" => self.ina.set_level(state.into()),
            "inb" => self.inb.set_level(state.into()),
            _ => {
                log::warn!("Unknown actuator id: {}", id);
                Ok(())
            }
        }
    }
}
