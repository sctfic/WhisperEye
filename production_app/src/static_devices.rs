use esp_idf_hal::gpio::*;
use esp_idf_hal::ledc::*;
use esp_idf_hal::units::*;
use log::info;

pub struct MotorPwmPin {
    driver: LedcDriver<'static>,
    active: bool,
    speed_pct: i32,
}

impl MotorPwmPin {
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

pub struct StaticDevices {
    pub relay_a: PinDriver<'static, Output>,
    pub relay_b: PinDriver<'static, Output>,
    pub sw_pwr: PinDriver<'static, Output>,
    pub touch: crate::touch::TouchSensor,
    pub ina: MotorPwmPin,
    pub inb: MotorPwmPin,
}

impl StaticDevices {
    pub fn init(
        gpio9: Gpio9<'static>,
        gpio47: Gpio47<'static>,
        gpio21: Gpio21<'static>,
        gpio14: Gpio14<'static>,
        gpio36: Gpio36<'static>,
        gpio35: Gpio35<'static>,
    ) -> Result<Self, anyhow::Error> {
        info!("Initializing static board devices...");
        
        let mut relay_a = PinDriver::output(gpio9)?;
        relay_a.set_low()?;

        let mut relay_b = PinDriver::output(gpio47)?;
        relay_b.set_low()?;
        
        let mut sw_pwr = PinDriver::output(gpio21)?;
        sw_pwr.set_high()?; // Keep system 5V/3V powered
        
        let touch = crate::touch::TouchSensor::new(gpio14, 50000)?;
        let _ = touch.calibrate(50);

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
            touch,
            ina,
            inb,
        })
    }
}
