use esp_idf_hal::gpio::*;
use esp_idf_hal::spi::*;
use esp_idf_hal::units::FromValueType;
use esp_idf_hal::delay::Ets;
use display_interface_spi::SPIInterfaceNoCS;
use mipidsi::{Builder, ColorInversion};
use embedded_graphics::{
    prelude::*,
    mono_font::{ascii::{FONT_10X20, FONT_6X10}, MonoTextStyleBuilder},
    text::Text,
    pixelcolor::Rgb565,
    primitives::{Rectangle, PrimitiveStyle},
};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

/// Check screen presence.
pub fn is_present() -> bool {
    // Screen is always connected on production board
    true
}

pub fn init_screen(
    spi: esp_idf_hal::spi::SPI2<'static>,
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
    adc1: esp_idf_hal::adc::ADC1<'static>,
    gpio1: Gpio1<'static>,
    gpio2: Gpio2<'static>,
    static_devs: Arc<Mutex<crate::static_devices::StaticDevices>>,
    actuators_state: Arc<Mutex<crate::actuators::ActuatorsState>>,
) -> Result<(), anyhow::Error> {
    std::thread::Builder::new()
        .name("ihm_thread".to_string())
        .stack_size(8192)
        .spawn(move || {
            if let Err(e) = run_ihm(spi, sclk, sda, rst, dc, cs, blk, btn0, btn1, btn2, btn3, adc1, gpio1, gpio2, static_devs, actuators_state) {
                log::error!("Erreur fatale dans le thread IHM : {:?}", e);
            }
        })?;
    Ok(())
}

fn run_ihm(
    spi: esp_idf_hal::spi::SPI2<'static>,
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
    adc1: esp_idf_hal::adc::ADC1<'static>,
    gpio1: Gpio1<'static>,
    gpio2: Gpio2<'static>,
    static_devs: Arc<Mutex<crate::static_devices::StaticDevices>>,
    actuators_state: Arc<Mutex<crate::actuators::ActuatorsState>>,
) -> Result<(), anyhow::Error> {
    // 1. Activer le rétroéclairage avec LEDC PWM (luminosité réduite à 20%)
    let timer0 = unsafe { esp_idf_hal::ledc::TIMER0::steal() };
    let channel0 = unsafe { esp_idf_hal::ledc::CHANNEL0::steal() };
    let mut blk_pwm = esp_idf_hal::ledc::LedcDriver::new(
        channel0,
        esp_idf_hal::ledc::LedcTimerDriver::new(
            timer0,
            &esp_idf_hal::ledc::config::TimerConfig::new().frequency(esp_idf_hal::units::FromValueType::kHz(5).into()),
        )?,
        blk,
    )?;
    let max_duty = blk_pwm.get_max_duty();
    blk_pwm.set_duty(max_duty * 20 / 100)?;

    // 2. Configurer le bus SPI
    let bus_config = SpiDriverConfig::default();
    let config = SpiConfig::new()
        .baudrate(26.MHz().into());
    let device = SpiDeviceDriver::new_single(
        spi,
        sclk,
        sda,
        Option::<Gpio0>::None,
        Some(cs),
        &bus_config,
        &config,
    )?;

    // 3. Interface d'affichage
    let dc_driver = PinDriver::output(dc)?;
    let di = SPIInterfaceNoCS::new(device, dc_driver);

    // 4. Reset physique du ST7789
    let mut rst_driver = PinDriver::output(rst)?;
    rst_driver.set_high()?;
    Ets::delay_us(5000);
    rst_driver.set_low()?;
    Ets::delay_us(15000);
    rst_driver.set_high()?;
    Ets::delay_us(15000);

    // 5. Initialiser le driver ST7789 via mipidsi
    let mut delay = Ets;
    let mut display = Builder::st7789(di)
        .with_display_size(240, 320)
        .with_orientation(mipidsi::Orientation::Landscape(true))
        .with_invert_colors(ColorInversion::Inverted)
        .init(&mut delay, Some(rst_driver))
        .map_err(|e| anyhow::anyhow!("Display init error: {:?}", e))?;

    // 6. Configurer les boutons et la roue codeuse en entrée avec pull-up interne
    let btn0_driver = PinDriver::input(btn0, Pull::Up)?;
    let btn1_driver = PinDriver::input(btn1, Pull::Up)?;
    let btn2_driver = PinDriver::input(btn2, Pull::Up)?;
    let btn3_driver = PinDriver::input(btn3, Pull::Up)?;

    // 6.bis Configurer l'ADC pour VSENSE (GPIO1) et ISENSE (GPIO2)
    let adc1_driver = esp_idf_hal::adc::oneshot::AdcDriver::new(adc1)?;
    let adc_config_V = esp_idf_hal::adc::oneshot::config::AdcChannelConfig {
        attenuation: esp_idf_hal::adc::attenuation::DB_2_5,
        calibration: esp_idf_hal::adc::oneshot::config::Calibration::Curve,
        ..Default::default()
    };
    let adc_config_I = esp_idf_hal::adc::oneshot::config::AdcChannelConfig {
        attenuation: esp_idf_hal::adc::attenuation::DB_6,
        calibration: esp_idf_hal::adc::oneshot::config::Calibration::Curve,
        ..Default::default()
    };
    let mut vsense_channel = esp_idf_hal::adc::oneshot::AdcChannelDriver::new(&adc1_driver, gpio1, &adc_config_V)?;
    let mut isense_channel = esp_idf_hal::adc::oneshot::AdcChannelDriver::new(&adc1_driver, gpio2, &adc_config_I)?;

    // Partager la valeur de luminosité (0-100) de manière atomique entre le thread de l'encodeur et le rendu
    let brightness_atomic = Arc::new(AtomicI32::new(20));
    let brightness_atomic_clone = Arc::clone(&brightness_atomic);

    // Thread dédié à la lecture de la roue codeuse avec machine d'état à quadrature complète
    std::thread::Builder::new()
        .name("encoder_thread".to_string())
        .stack_size(4096)
        .spawn(move || {
            // Initialiser l'état de l'encodeur
            let a = btn0_driver.is_high();
            let b = btn1_driver.is_high();
            let mut last_state = ((a as u8) << 1) | (b as u8);
            let mut acc_steps = 0i8;

            // Table de décodage à quadrature complète de l'encodeur Gray code.
            // Index : (état_précédent << 2) | état_actuel
            const ENCODER_STATES: [i8; 16] = [
                0,   // 00 -> 00
                1,   // 00 -> 01 (CCW)
                -1,  // 00 -> 10 (CW)
                0,   // 00 -> 11
                -1,  // 01 -> 00 (CW)
                0,   // 01 -> 01
                0,   // 01 -> 10
                1,   // 01 -> 11 (CCW)
                1,   // 10 -> 00 (CCW)
                0,   // 10 -> 01
                0,   // 10 -> 10
                -1,  // 10 -> 11 (CW)
                0,   // 11 -> 00
                -1,  // 11 -> 01 (CW)
                1,   // 11 -> 10 (CCW)
                0,   // 11 -> 11
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
                        let mut val = brightness_atomic_clone.load(Ordering::Relaxed);
                        if val <= 95 {
                            val += 5;
                            brightness_atomic_clone.store(val, Ordering::Relaxed);
                        }
                        acc_steps -= 4;
                    } else if acc_steps <= -4 {
                        let mut val = brightness_atomic_clone.load(Ordering::Relaxed);
                        if val >= 10 {
                            val -= 5;
                            brightness_atomic_clone.store(val, Ordering::Relaxed);
                        }
                        acc_steps += 4;
                    }
                    last_state = current_state;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        })?;

    // 7. Dessin initial
    let mut last_rendered_brightness = 20i32;
    let mut last_rendered_ina = false;
    let mut last_rendered_inb = false;
    let mut last_touch_state = false;
    let mut last_rendered_ver = crate::i2c_bus::BME280_VERSION.load(Ordering::Relaxed);
    let mut raw_update_ticks = 0;

    let mut vsense_volts = 0.0f32;
    let mut isense_amps = 0.0f32;
    let mut vsense_mv = 0u16;
    let mut isense_mv = 0u16;
    let mut vsense_raw = 0u16;
    let mut isense_raw = 0u16;

    display.clear(Rgb565::BLACK)
        .map_err(|e| anyhow::anyhow!("Clear display error: {:?}", e))?;

    let text_style = MonoTextStyleBuilder::new()
        .font(&FONT_10X20)
        .text_color(Rgb565::WHITE)
        .background_color(Rgb565::BLACK)
        .build();

    let small_text_style = MonoTextStyleBuilder::new()
        .font(&FONT_6X10)
        .text_color(Rgb565::new(20, 40, 20)) // Gris-vert discret
        .background_color(Rgb565::BLACK)
        .build();

    let green_style = MonoTextStyleBuilder::new()
        .font(&FONT_10X20)
        .text_color(Rgb565::GREEN)
        .background_color(Rgb565::BLACK)
        .build();

    let red_style = MonoTextStyleBuilder::new()
        .font(&FONT_10X20)
        .text_color(Rgb565::RED)
        .background_color(Rgb565::BLACK)
        .build();
    
    // 1ère ligne : Affichage BME280 si présent
    let is_bme_found = crate::i2c_bus::BME280_FOUND.load(Ordering::Relaxed);
    let bme_text = if is_bme_found {
        let t = *crate::i2c_bus::BME280_TEMP.lock().unwrap();
        let h = *crate::i2c_bus::BME280_HUM.lock().unwrap();
        let p = *crate::i2c_bus::BME280_PRESS.lock().unwrap();
        format!("BME:{:.1}C {:.1}% {:.0}hPa", t, h, p)
    } else {
        "BME280: Absent".to_string()
    };
    let bme_text_padded = format!("{:<24}", bme_text);
    let bme_style = if is_bme_found { green_style } else { red_style };
    Text::new(&bme_text_padded, Point::new(10, 20), bme_style)
        .draw(&mut display)
        .map_err(|e| anyhow::anyhow!("Draw BME error: {:?}", e))?;

    let text = format!("Luminosite: {}%", last_rendered_brightness);
    Text::new(&text, Point::new(45, 100), text_style)
        .draw(&mut display)
        .map_err(|e| anyhow::anyhow!("Draw text error: {:?}", e))?;

    let state_text = format!("INA: {}   INB: {}", if last_rendered_ina { 1 } else { 0 }, if last_rendered_inb { 1 } else { 0 });
    Text::new(&state_text, Point::new(45, 130), text_style)
        .draw(&mut display)
        .map_err(|e| anyhow::anyhow!("Draw state error: {:?}", e))?;

    let touch_str = if last_touch_state { "Touch: TOUCHE" } else { "Touch: RELACHE" };
    Text::new(touch_str, Point::new(45, 160), text_style)
        .draw(&mut display)
        .map_err(|e| anyhow::anyhow!("Draw touch error: {:?}", e))?;

    let adc_text = format!("V:{:.1}V({}mV) I:{:.2}A({}mV)", vsense_volts, vsense_mv, isense_amps, isense_mv);
    let adc_text_padded = format!("{:<30}", adc_text);
    Text::new(&adc_text_padded, Point::new(10, 190), text_style)
        .draw(&mut display)
        .map_err(|e| anyhow::anyhow!("Draw ADC error: {:?}", e))?;

    let raw_adc_text = format!("VSENSE RAW: {:<4}  ISENSE RAW: {:<4}", vsense_raw, isense_raw);
    let raw_adc_padded = format!("{:<40}", raw_adc_text);
    Text::new(&raw_adc_padded, Point::new(10, 203), small_text_style)
        .draw(&mut display)
        .map_err(|e| anyhow::anyhow!("Draw RAW ADC error: {:?}", e))?;

    let mut last_btn2 = true;
    let mut last_btn3 = true;
    let mut btn3_pressed_ticks = 0u32;
    let mut last_brightness = 20i32;

    loop {
        // Échantillonner les boutons
        let btn2_val = btn2_driver.is_high();
        let btn3_val = btn3_driver.is_high();

        // 0. Appliquer la luminosité si elle change
        let current_brightness = brightness_atomic.load(Ordering::Relaxed);
        if current_brightness != last_brightness {
            let _ = blk_pwm.set_duty(max_duty * current_brightness as u32 / 100);
            {
                let mut devs = static_devs.lock().unwrap();
                let _ = devs.ina.set_speed(current_brightness);
                let _ = devs.inb.set_speed(current_brightness);
            }
            last_brightness = current_brightness;
        }

        // 1. Appui sur BTN2 réinitialise la luminosité à 20% (front descendant)
        if last_btn2 && !btn2_val {
            brightness_atomic.store(20, Ordering::Relaxed);
        }
        last_btn2 = btn2_val;

        // 2. Chaque appui sur BTN3 changera l'état de INA/INB
        if last_btn3 && !btn3_val {
            let (next_ina, next_inb) = {
                let act = actuators_state.lock().unwrap();
                match (act.ina, act.inb) {
                    (false, false) => (true, false),
                    (true, false) => (false, true),
                    (false, true) => (true, true),
                    (true, true) => (false, false),
                }
            };

            // Appliquer au hardware
            {
                let mut devs = static_devs.lock().unwrap();
                let _ = devs.ina.set_level(next_ina.into());
                let _ = devs.inb.set_level(next_inb.into());
            }

            // Mettre à jour l'état partagé
            {
                let mut act = actuators_state.lock().unwrap();
                act.ina = next_ina;
                act.inb = next_inb;
            }

            log::info!(
                "Etat change par BTN3 : INA = {}, INB = {}",
                if next_ina { 1 } else { 0 },
                if next_inb { 1 } else { 0 }
            );
        }

        // 2.bis Appui long sur BTN3 (2 secondes = 200 ticks de 10ms) coupe SWPWR
        if !btn3_val {
            btn3_pressed_ticks += 1;
            if btn3_pressed_ticks == 200 {
                let mut devs = static_devs.lock().unwrap();
                let _ = devs.sw_pwr.set_low();
                let mut act = actuators_state.lock().unwrap();
                act.swpwr = false;
            }
        } else {
            btn3_pressed_ticks = 0;
        }
        last_btn3 = btn3_val;

        raw_update_ticks += 1;
        let should_update_raw = raw_update_ticks >= 10;
        if should_update_raw {
            raw_update_ticks = 0;
            if let Ok(mv) = adc1_driver.read(&mut vsense_channel) {
                // vsense, sur un pont diviseur  47 kohms / (1000 kohms + 47 kohms) = 0.04489
                // donc notre mesure est de 4.49% de la tension reele. On multiplie donc par 22.27 / 1000.0
                // l'adc lit de 0 a 1.25V sur l'echelle de 0 a 4095
                vsense_mv = mv;
                vsense_volts = ((mv as f32) * 20.74 / 1000.0) -1.2;
            }
            if let Ok(raw) = adc1_driver.read_raw(&mut vsense_channel) {
                vsense_raw = raw;
            }
            if let Ok(mv) = adc1_driver.read(&mut isense_channel) {
                isense_mv = mv;
                isense_amps = ((mv as f32) * 3.08 / 1000.0) -0.21;
            }
            if let Ok(raw) = adc1_driver.read_raw(&mut isense_channel) {
                isense_raw = raw;
            }
        }

        // 3. Lire l'état du capteur tactile (TOUCH - GPIO14)
        let current_touch = {
            let devs = static_devs.lock().unwrap();
            devs.touch.is_pressed().unwrap_or(false)
        };

        // 4. Si TOUCH est appuyé (high), rallumer SWPWR
        if current_touch && !last_touch_state {
            let mut devs = static_devs.lock().unwrap();
            let _ = devs.sw_pwr.set_high();
            let mut act = actuators_state.lock().unwrap();
            act.swpwr = true;
        }

        // 5. Redessiner l'écran
        let (current_ina, current_inb) = {
            let act = actuators_state.lock().unwrap();
            (act.ina, act.inb)
        };
        let current_ver = crate::i2c_bus::BME280_VERSION.load(Ordering::Relaxed);
        
        let should_redraw_main = current_brightness != last_rendered_brightness 
            || current_touch != last_touch_state 
            || current_ver != last_rendered_ver
            || current_ina != last_rendered_ina
            || current_inb != last_rendered_inb;
        
        if should_redraw_main || should_update_raw {
            if should_redraw_main {
                let info_changed = current_brightness != last_rendered_brightness
                    || current_touch != last_touch_state
                    || current_ina != last_rendered_ina
                    || current_inb != last_rendered_inb;

                last_rendered_brightness = current_brightness;
                last_touch_state = current_touch;
                last_rendered_ver = current_ver;
                last_rendered_ina = current_ina;
                last_rendered_inb = current_inb;

                // Si les infos changent, on efface uniquement la zone des textes mobiles (sous Y=35 à Y=195)
                if info_changed {
                    Rectangle::new(Point::new(0, 35), Size::new(320, 160))
                        .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
                        .draw(&mut display)
                        .map_err(|e| anyhow::anyhow!("Clear text area error: {:?}", e))?;
                }

                // 1ère ligne : Affichage BME280 si présent
                let is_bme_found = crate::i2c_bus::BME280_FOUND.load(Ordering::Relaxed);
                let bme_text = if is_bme_found {
                    let t = *crate::i2c_bus::BME280_TEMP.lock().unwrap();
                    let h = *crate::i2c_bus::BME280_HUM.lock().unwrap();
                    let p = *crate::i2c_bus::BME280_PRESS.lock().unwrap();
                    format!("BME:{:.1}C {:.1}% {:.0}hPa", t, h, p)
                } else {
                    "BME280: Absent".to_string()
                };
                let bme_text_padded = format!("{:<24}", bme_text);
                let bme_style = if is_bme_found { green_style } else { red_style };
                Text::new(&bme_text_padded, Point::new(10, 20), bme_style)
                    .draw(&mut display)
                    .map_err(|e| anyhow::anyhow!("Draw BME error: {:?}", e))?;

                let text = format!("Luminosite: {}%", last_rendered_brightness);
                Text::new(&text, Point::new(45, 100), text_style)
                    .draw(&mut display)
                    .map_err(|e| anyhow::anyhow!("Draw text error: {:?}", e))?;

                let state_text = format!("INA: {}   INB: {}", if last_rendered_ina { 1 } else { 0 }, if last_rendered_inb { 1 } else { 0 });
                Text::new(&state_text, Point::new(45, 130), text_style)
                    .draw(&mut display)
                    .map_err(|e| anyhow::anyhow!("Draw state error: {:?}", e))?;

                let touch_str = if last_touch_state { "Touch: TOUCHE" } else { "Touch: RELACHE" };
                Text::new(touch_str, Point::new(45, 160), text_style)
                    .draw(&mut display)
                    .map_err(|e| anyhow::anyhow!("Draw touch error: {:?}", e))?;
            }

            // Toujours redessiner la ligne ADC à Y=190 et RAW à Y=203
            let adc_text = format!("V:{:.1}V({}mV) I:{:.2}A({}mV)", vsense_volts, vsense_mv, isense_amps, isense_mv);
            let adc_text_padded = format!("{:<30}", adc_text);
            Text::new(&adc_text_padded, Point::new(10, 190), text_style)
                .draw(&mut display)
                .map_err(|e| anyhow::anyhow!("Draw ADC error: {:?}", e))?;

            let raw_adc_text = format!("VSENSE RAW: {:<4}  ISENSE RAW: {:<4}", vsense_raw, isense_raw);
            let raw_adc_padded = format!("{:<40}", raw_adc_text);
            Text::new(&raw_adc_padded, Point::new(10, 203), small_text_style)
                .draw(&mut display)
                .map_err(|e| anyhow::anyhow!("Draw RAW ADC error: {:?}", e))?;

            // Toujours redessiner la ligne de valeur brute à Y=223
            let (raw_val, thresh) = {
                let devs = static_devs.lock().unwrap();
                (
                    devs.touch.get_raw_value().unwrap_or(0),
                    devs.touch.get_threshold(),
                )
            };
            
            let raw_text = format!("Raw:{:<5} Thresh:{:<5}", raw_val, thresh);
            let raw_text_padded = format!("{:<24}", raw_text);
            let style = if current_touch { green_style } else { red_style };
            Text::new(&raw_text_padded, Point::new(10, 223), style)
                .draw(&mut display)
                .map_err(|e| anyhow::anyhow!("Draw raw touch error: {:?}", e))?;
        }

        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
