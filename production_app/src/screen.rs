use esp_idf_hal::gpio::*;
use esp_idf_hal::ledc::*;
use esp_idf_hal::spi::*;
use esp_idf_hal::delay::Ets;
use esp_idf_hal::units::FromValueType;
use display_interface_spi::SPIInterfaceNoCS;
use mipidsi::Builder;
use mipidsi::options::ColorInversion;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

pub type ST7789Display = mipidsi::Display<
    SPIInterfaceNoCS<SpiDeviceDriver<'static, SpiDriver<'static>>, PinDriver<'static, Output>>,
    mipidsi::models::ST7789,
    PinDriver<'static, Output>,
>;

pub struct Screen {
    pub blk_pwm: LedcDriver<'static>,
    pub max_duty: u32,
    pub btn2_driver: PinDriver<'static, Input>,
    pub btn3_driver: PinDriver<'static, Input>,
    pub brightness: Arc<AtomicI32>,
}

impl Screen {
    pub fn init(
        spi: SPI2<'static>,
        sclk: Gpio7<'static>,
        sda: Gpio15<'static>,
        rst: Gpio16<'static>,
        dc: Gpio4<'static>,
        cs: Gpio5<'static>,
        blk: Gpio6<'static>,
        btn0: Gpio17<'static>,
        btn1: Gpio18<'static>,
        btn2: Gpio8<'static>,
        btn3: Gpio3<'static>,
    ) -> Result<(Self, ST7789Display), anyhow::Error> {
        log::info!("Initializing screen backlight (LEDC PWM)...");
        let timer0 = unsafe { TIMER0::steal() };
        let channel0 = unsafe { CHANNEL0::steal() };
        let mut blk_pwm = LedcDriver::new(
            channel0,
            LedcTimerDriver::new(
                timer0,
                &config::TimerConfig::new().frequency(5.kHz().into()),
            )?,
            blk,
        )?;
        let max_duty = blk_pwm.get_max_duty();
        blk_pwm.set_duty(max_duty * 20 / 100)?; // 20% par défaut

        log::info!("Configuring SPI bus for display...");
        let bus_config = SpiDriverConfig::default();
        let config = SpiConfig::new().baudrate(26.MHz().into());
        let device = SpiDeviceDriver::new_single(
            spi,
            sclk,
            sda,
            Option::<Gpio0>::None,
            Some(cs),
            &bus_config,
            &config,
        )?;

        let dc_driver = PinDriver::output(dc)?;
        let di = SPIInterfaceNoCS::new(device, dc_driver);

        log::info!("Physical reset of ST7789...");
        let mut rst_driver = PinDriver::output(rst)?;
        rst_driver.set_high()?;
        Ets::delay_us(5000);
        rst_driver.set_low()?;
        Ets::delay_us(15000);
        rst_driver.set_high()?;
        Ets::delay_us(15000);

        log::info!("Initializing ST7789 controller via mipidsi...");
        let mut delay = Ets;
        let display = Builder::st7789(di)
            .with_display_size(240, 320)
            .with_orientation(mipidsi::Orientation::Landscape(true))
            .with_invert_colors(mipidsi::ColorInversion::Normal)
            .init(&mut delay, Some(rst_driver))
            .map_err(|e| anyhow::anyhow!("Display init error: {:?}", e))?;

        log::info!("Configuring encoder buttons...");
        let btn0_driver = PinDriver::input(btn0, Pull::Up)?;
        let btn1_driver = PinDriver::input(btn1, Pull::Up)?;
        let btn2_driver = PinDriver::input(btn2, Pull::Up)?;
        let btn3_driver = PinDriver::input(btn3, Pull::Up)?;
        log::info!("Encoder buttons configured.");

        let brightness = Arc::new(AtomicI32::new(20));
        let brightness_clone = Arc::clone(&brightness);

        // Lancement du thread de gestion de l'encodeur Gray Code (machine d'état à quadrature complète)
        log::info!("Spawning encoder_thread...");
        std::thread::Builder::new()
            .name("encoder_thread".to_string())
            .stack_size(16384)
            .spawn(move || {
                // Petit délai pour laisser l'init se terminer
                std::thread::sleep(std::time::Duration::from_millis(50));

                // Initialiser l'état de l'encodeur
                let a = btn0_driver.is_high();
                let b = btn1_driver.is_high();
                let mut last_state = ((a as u8) << 1) | (b as u8);
                let mut acc_steps: i8 = 0;

                // Table de décodage à quadrature complète de l'encodeur Gray code.
                // Index : (état_précédent << 2) | état_actuel
                const ENCODER_STATES: [i8; 16] = [
                     0,  // 00 -> 00
                     1,  // 00 -> 01 (CCW)
                    -1,  // 00 -> 10 (CW)
                     0,  // 00 -> 11
                    -1,  // 01 -> 00 (CW)
                     0,  // 01 -> 01
                     0,  // 01 -> 10
                     1,  // 01 -> 11 (CCW)
                     1,  // 10 -> 00 (CCW)
                     0,  // 10 -> 01
                     0,  // 10 -> 10
                    -1,  // 10 -> 11 (CW)
                     0,  // 11 -> 00
                    -1,  // 11 -> 01 (CW)
                     1,  // 11 -> 10 (CCW)
                     0,  // 11 -> 11
                ];

                loop {
                    let a = btn0_driver.is_high();
                    let b = btn1_driver.is_high();
                    let current_state = ((a as u8) << 1) | (b as u8);

                    if current_state != last_state {
                        let index = ((last_state << 2) | current_state) as usize;
                        let change = ENCODER_STATES[index];
                        acc_steps += change;

                        if acc_steps >= 4 {
                            let mut val = brightness_clone.load(Ordering::Relaxed);
                            if val <= 95 {
                                val += 5;
                                brightness_clone.store(val, Ordering::Relaxed);
                            }
                            acc_steps -= 4;
                        } else if acc_steps <= -4 {
                            let mut val = brightness_clone.load(Ordering::Relaxed);
                            if val >= 10 {
                                val -= 5;
                                brightness_clone.store(val, Ordering::Relaxed);
                            }
                            acc_steps += 4;
                        }
                        last_state = current_state;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            })?;
        log::info!("encoder_thread spawned successfully.");

        let screen = Self {
            blk_pwm,
            max_duty,
            btn2_driver,
            btn3_driver,
            brightness,
        };

        Ok((screen, display))
    }

    pub fn detect(&self) -> bool {
        true
    }

    pub fn set_backlight(&mut self, brightness_pct: u32) -> Result<(), anyhow::Error> {
        let duty = self.max_duty * brightness_pct / 100;
        self.blk_pwm.set_duty(duty)?;
        Ok(())
    }

    pub fn read_brightness(&self) -> i32 {
        self.brightness.load(Ordering::Relaxed)
    }
}

pub fn is_present() -> bool {
    true
}
