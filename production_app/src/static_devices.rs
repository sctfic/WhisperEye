use esp_idf_hal::gpio::*;
use log::info;

pub struct StaticDevices {
    pub relay_a: PinDriver<'static, Output>,
    pub relay_b: PinDriver<'static, Output>,
    pub sw_pwr: PinDriver<'static, Output>,
    #[allow(dead_code)]
    pub touch: PinDriver<'static, Input>,
    pub ina: PinDriver<'static, Output>,
    pub inb: PinDriver<'static, Output>,
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
        
        let touch = PinDriver::input(gpio14, Pull::Floating)?;

        let mut ina = PinDriver::output(gpio36)?;
        ina.set_low()?;

        let mut inb = PinDriver::output(gpio35)?;
        inb.set_low()?;
        
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
