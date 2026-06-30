use esp_idf_hal::gpio::*;
use esp_idf_hal::ledc::*;
use esp_idf_hal::spi::*;
use esp_idf_hal::delay::Ets;
use esp_idf_hal::units::FromValueType;
use display_interface_spi::SPIInterfaceNoCS;
use mipidsi::Builder;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

pub type ST7789Display = mipidsi::Display<
    SPIInterfaceNoCS<SpiDeviceDriver<'static, SpiDriver<'static>>, PinDriver<'static, Output>>,
    mipidsi::models::ST7789,
    PinDriver<'static, Output>,
>;

/// Wrapper safe autour du PCNT hardware avec cleanup propre
pub struct PcntEncoder {
    unit: esp_idf_sys::pcnt_unit_handle_t,
    chan_a: esp_idf_sys::pcnt_channel_handle_t,
    chan_b: esp_idf_sys::pcnt_channel_handle_t,
}

impl PcntEncoder {
    pub fn new(pin_a: i32, pin_b: i32) -> Result<Self, anyhow::Error> {
        let mut unit: esp_idf_sys::pcnt_unit_handle_t = std::ptr::null_mut();

        // Limites larges pour éviter la saturation entre deux lectures
        let unit_config = esp_idf_sys::pcnt_unit_config_t {
            low_limit: -32768,
            high_limit: 32767,
            flags: Default::default(),
            intr_priority: 0,
        };

        let err = unsafe { esp_idf_sys::pcnt_new_unit(&unit_config, &mut unit) };
        if err != esp_idf_sys::ESP_OK {
            anyhow::bail!("pcnt_new_unit failed: {}", err);
        }

        // Filtre anti-rebond hardware (~10µs, à ajuster selon ton EC11)
        let filter_config = esp_idf_sys::pcnt_glitch_filter_config_t {
            max_glitch_ns: 10000,
        };
        let err = unsafe { esp_idf_sys::pcnt_unit_set_glitch_filter(unit, &filter_config) };
        if err != esp_idf_sys::ESP_OK {
            unsafe { esp_idf_sys::pcnt_del_unit(unit) };
            anyhow::bail!("pcnt_unit_set_glitch_filter failed: {}", err);
        }

        // Channel A : edge sur pin_a, level contrôlé par pin_b
        let mut chan_a: esp_idf_sys::pcnt_channel_handle_t = std::ptr::null_mut();
        let chan_a_config = esp_idf_sys::pcnt_chan_config_t {
            edge_gpio_num: pin_a,
            level_gpio_num: pin_b,
            flags: Default::default(),
        };
        let err = unsafe { esp_idf_sys::pcnt_new_channel(unit, &chan_a_config, &mut chan_a) };
        if err != esp_idf_sys::ESP_OK {
            unsafe { esp_idf_sys::pcnt_del_unit(unit) };
            anyhow::bail!("pcnt_new_channel A failed: {}", err);
        }

        // Channel B : edge sur pin_b, level contrôlé par pin_a
        let mut chan_b: esp_idf_sys::pcnt_channel_handle_t = std::ptr::null_mut();
        let chan_b_config = esp_idf_sys::pcnt_chan_config_t {
            edge_gpio_num: pin_b,
            level_gpio_num: pin_a,
            flags: Default::default(),
        };
        let err = unsafe { esp_idf_sys::pcnt_new_channel(unit, &chan_b_config, &mut chan_b) };
        if err != esp_idf_sys::ESP_OK {
            unsafe {
                esp_idf_sys::pcnt_del_channel(chan_a);
                esp_idf_sys::pcnt_del_unit(unit);
            }
            anyhow::bail!("pcnt_new_channel B failed: {}", err);
        }

        unsafe {
            // Canal A
            esp_idf_sys::pcnt_channel_set_edge_action(
                chan_a,
                esp_idf_sys::pcnt_channel_edge_action_t_PCNT_CHANNEL_EDGE_ACTION_DECREASE,
                esp_idf_sys::pcnt_channel_edge_action_t_PCNT_CHANNEL_EDGE_ACTION_INCREASE,
            );
            esp_idf_sys::pcnt_channel_set_level_action(
                chan_a,
                esp_idf_sys::pcnt_channel_level_action_t_PCNT_CHANNEL_LEVEL_ACTION_KEEP,
                esp_idf_sys::pcnt_channel_level_action_t_PCNT_CHANNEL_LEVEL_ACTION_INVERSE,
            );

            // Canal B
            esp_idf_sys::pcnt_channel_set_edge_action(
                chan_b,
                esp_idf_sys::pcnt_channel_edge_action_t_PCNT_CHANNEL_EDGE_ACTION_INCREASE,
                esp_idf_sys::pcnt_channel_edge_action_t_PCNT_CHANNEL_EDGE_ACTION_DECREASE,
            );
            esp_idf_sys::pcnt_channel_set_level_action(
                chan_b,
                esp_idf_sys::pcnt_channel_level_action_t_PCNT_CHANNEL_LEVEL_ACTION_KEEP,
                esp_idf_sys::pcnt_channel_level_action_t_PCNT_CHANNEL_LEVEL_ACTION_INVERSE,
            );

            esp_idf_sys::pcnt_unit_enable(unit);
            esp_idf_sys::pcnt_unit_clear_count(unit);
            esp_idf_sys::pcnt_unit_start(unit);
        }

        Ok(Self { unit, chan_a, chan_b })
    }

    pub fn count(&self) -> Result<i32, anyhow::Error> {
        let mut count: std::os::raw::c_int = 0;
        let err = unsafe { esp_idf_sys::pcnt_unit_get_count(self.unit, &mut count) };
        if err != esp_idf_sys::ESP_OK {
            anyhow::bail!("pcnt_unit_get_count failed: {}", err);
        }
        Ok(count as i32)
    }

    /// Réinitialise le compteur à 0 (utile pour éviter la saturation)
    pub fn clear(&self) {
        unsafe { esp_idf_sys::pcnt_unit_clear_count(self.unit) };
    }
}

impl Drop for PcntEncoder {
    fn drop(&mut self) {
        unsafe {
            esp_idf_sys::pcnt_unit_stop(self.unit);
            esp_idf_sys::pcnt_unit_disable(self.unit);
            esp_idf_sys::pcnt_del_channel(self.chan_a);
            esp_idf_sys::pcnt_del_channel(self.chan_b);
            esp_idf_sys::pcnt_del_unit(self.unit);
        }
    }
}

// Le handle ESP-IDF est thread-safe natif
unsafe impl Send for PcntEncoder {}
unsafe impl Sync for PcntEncoder {}

pub struct Screen {
    pub blk_pwm: LedcDriver<'static>,
    pub max_duty: u32,
    pub btn2_driver: PinDriver<'static, Input>,
    pub btn3_driver: PinDriver<'static, Input>,
    pub brightness: Arc<AtomicI32>,
    // Garder l'encodeur en vie aussi longtemps que Screen
    _encoder: Arc<PcntEncoder>,
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
        // GPIO17 et 18 sont réservés pour le PCNT, on ne crée PAS de PinDriver dessus
        _btn0: Gpio17<'static>,
        _btn1: Gpio18<'static>,
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
        // ❌ SUPPRIMÉ : Pas de PinDriver sur GPIO17/18 — réservés PCNT
        let btn2_driver = PinDriver::input(btn2, Pull::Up)?;
        let btn3_driver = PinDriver::input(btn3, Pull::Up)?;
        log::info!("Encoder buttons configured.");

        let brightness = Arc::new(AtomicI32::new(20));
        let brightness_clone = Arc::clone(&brightness);

        // PCNT sur GPIO17 et 18
        log::info!("Configuring PCNT hardware for encoder...");
        let encoder = Arc::new(PcntEncoder::new(17, 18)?);
        let encoder_thread = Arc::clone(&encoder);

        std::thread::Builder::new()
            .name("encoder_pcnt".to_string())
            .stack_size(4096)
            .spawn(move || {
                let mut last_count = 0;
                let mut acc_steps = 0;

                loop {
                    match encoder_thread.count() {
                        Ok(current_count) => {
                            let diff = current_count - last_count;
                            if diff != 0 {
                                acc_steps += diff;
                                last_count = current_count;

                                // Gestion de la saturation : reset périodique
                                if current_count.abs() > 30000 {
                                    encoder_thread.clear();
                                    last_count = 0;
                                }

                                // 4 counts = 1 cran d'encodeur EC11
                                while acc_steps >= 4 {
                                    let mut val = brightness_clone.load(Ordering::Relaxed);
                                    if val <= 95 {
                                        val += 5;
                                        brightness_clone.store(val, Ordering::Relaxed);
                                    }
                                    acc_steps -= 4;
                                }
                                while acc_steps <= -4 {
                                    let mut val = brightness_clone.load(Ordering::Relaxed);
                                    if val >= 10 {
                                        val -= 5;
                                        brightness_clone.store(val, Ordering::Relaxed);
                                    }
                                    acc_steps += 4;
                                }
                            }
                        }
                        Err(e) => {
                            log::error!("PCNT read error: {:?}", e);
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10)); // 10ms, pas 100ms
                }
            })
            .expect("Failed to spawn PCNT encoder thread");

        let screen = Self {
            blk_pwm,
            max_duty,
            btn2_driver,
            btn3_driver,
            brightness,
            _encoder: encoder, // Garde l'encodeur en vie
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