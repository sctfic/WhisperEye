use esp_idf_hal::adc::oneshot::{AdcDriver, AdcChannelDriver, config::AdcChannelConfig};
use esp_idf_hal::adc::{ADC1, attenuation, oneshot::config::Calibration, ADCCH0, ADCCH1, ADCU1};
use esp_idf_hal::gpio::{Gpio1, Gpio2, Gpio14};
use crate::touch::TouchSensor;

#[derive(Debug, Clone, serde::Serialize)]
pub struct BoardReadings {
    pub touch: bool,
    pub vsense_volts: Option<f32>,
    pub isense_amps: Option<f32>,
    pub vsense_raw: u32,
    pub isense_raw: u32,
}

pub struct Board {
    adc_driver: &'static AdcDriver<'static, ADCU1>,
    vsense_channel: AdcChannelDriver<'static, ADCCH0<ADCU1>, &'static AdcDriver<'static, ADCU1>>,
    isense_channel: AdcChannelDriver<'static, ADCCH1<ADCU1>, &'static AdcDriver<'static, ADCU1>>,
    pub touch: TouchSensor,
}

impl Board {
    pub fn init(
        adc1: ADC1<'static>,
        gpio1: Gpio1<'static>,
        gpio2: Gpio2<'static>,
        gpio14: Gpio14<'static>,
    ) -> Result<Self, anyhow::Error> {
        log::info!("Initializing Board hardware (Touch, VSENSE, ISENSE)...");

        // 1. Initialiser le pilote ADC avec Box::leak pour obtenir un &'static AdcDriver<'static>
        let adc_driver: &'static AdcDriver<'static, ADCU1> = Box::leak(Box::new(AdcDriver::new(adc1)?));

        // 2. Configurer les canaux ADC pour VSENSE (GPIO1) et ISENSE (GPIO2)
        let adc_config_v = AdcChannelConfig {
            attenuation: attenuation::DB_2_5,
            calibration: Calibration::Curve,
            ..Default::default()
        };
        let adc_config_i = AdcChannelConfig {
            attenuation: attenuation::DB_6,
            calibration: Calibration::Curve,
            ..Default::default()
        };

        let vsense_channel = AdcChannelDriver::new(adc_driver, gpio1, &adc_config_v)?;
        let isense_channel = AdcChannelDriver::new(adc_driver, gpio2, &adc_config_i)?;

        // 3. Initialiser le Touch Sensor sur GPIO14
        let touch = TouchSensor::new(gpio14, 50000)?;
        let _ = touch.calibrate(50);

        Ok(Self {
            adc_driver,
            vsense_channel,
            isense_channel,
            touch,
        })
    }

    pub fn detect(&self) -> bool {
        // La carte mère principale est toujours considérée comme présente
        true
    }

    pub fn read_value(&mut self, ina_active: bool, inb_active: bool) -> BoardReadings {
        let touch = self.touch.is_pressed().unwrap_or(false);

        // Lecture de VSENSE (Tension)
        let mut vsense_mv = 0;
        let mut vsense_raw = 0;
        if let Ok(mv) = self.adc_driver.read(&mut self.vsense_channel) {
            vsense_mv = mv;
        }
        if let Ok(raw) = self.adc_driver.read_raw(&mut self.vsense_channel) {
            vsense_raw = raw;
        }

        // Calcul de la tension réelle en Volts
        let volts = ((vsense_mv as f32) * 20.74 / 1000.0) - 1.2;
        let vsense_volts = if volts < 1.0 {
            None
        } else {
            Some(volts)
        };

        // Lecture de ISENSE (Courant) avec intégration sur 128 échantillons (10 cycles PWM 10kHz complets)
        let mut total_mv = 0u32;
        let mut total_raw = 0u32;
        let samples = 128;
        for _ in 0..samples {
            if let Ok(mv) = self.adc_driver.read(&mut self.isense_channel) {
                total_mv += mv as u32;
            }
            if let Ok(raw) = self.adc_driver.read_raw(&mut self.isense_channel) {
                total_raw += raw as u32;
            }
        }
        let isense_mv = total_mv / samples;
        let isense_raw = total_raw / samples;

        // Calcul de l'intensité en Ampères
        let raw_amps = ((isense_mv as f32) * 3.08 / 1000.0) - 0.21;
        let amps = if !ina_active && !inb_active {
            0.0
        } else if raw_amps < 0.0 {
            0.0
        } else {
            raw_amps
        };
        let mut isense_amps = Some(amps);

        // Application des règles métier :
        // 1. isense présent si vsense > 6V
        if let Some(v) = vsense_volts {
            if v <= 6.0 {
                isense_amps = None;
            }
        } else {
            isense_amps = None;
        }

        // 2. si isense_raw = 4095 et ina = inb = 0, retourner null (None)
        if isense_raw == 4095 && !ina_active && !inb_active {
            isense_amps = None;
        }

        BoardReadings {
            touch,
            vsense_volts,
            isense_amps,
            vsense_raw: vsense_raw as u32,
            isense_raw: isense_raw as u32,
        }
    }

    pub fn is_touch_pressed(&self) -> bool {
        self.touch.is_pressed().unwrap_or(false)
    }
}

unsafe impl Send for Board {}
unsafe impl Sync for Board {}
