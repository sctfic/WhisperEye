#![allow(deprecated)]
use std::sync::Mutex;
use std::thread;
use std::time::Duration;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Once;

use ws2812_esp32_rmt_driver::driver::Ws2812Esp32RmtDriver;
use ws2812_esp32_rmt_driver::driver::color::{LedPixelColor, LedPixelColorGrb24};

// Instance globale du pilote RMT WS2812, initialisée une seule fois par init_led()
static LED_DRIVER: Mutex<Option<Ws2812Esp32RmtDriver<'static>>> = Mutex::new(None);

/// Initialise le pilote RMT pour la LED WS2812 sur le GPIO 48 (canal RMT 0).
/// Doit être appelé une seule fois au démarrage, avant tout appel à set_led_color().
pub fn init_led(
    channel: esp_idf_hal::rmt::CHANNEL0,
    pin: esp_idf_hal::gpio::Gpio48,
) -> anyhow::Result<()> {
    let driver = Ws2812Esp32RmtDriver::new(channel, pin)?;
    // SAFETY: Les périphériques ESP32 sont des singletons (Peripherals::take()) qui vivent
    // pour toute la durée du programme et ne sont jamais libérés. Le driver RMT est stocké
    // dans un Mutex statique global, ce qui est sûr tant que les périphériques ne sont pas
    // réutilisés ailleurs (garanti par le système d'ownership de Peripherals).
    let driver: Ws2812Esp32RmtDriver<'static> = unsafe { std::mem::transmute(driver) };
    let mut guard = LED_DRIVER.lock().unwrap();
    *guard = Some(driver);
    log::info!("LED RMT initialisée sur GPIO 48 (canal 0)");
    Ok(())
}

pub type Color = (u8, u8, u8);
pub const RED: Color = (255, 0, 0);
pub const GREEN: Color = (0, 255, 0);
pub const BLUE: Color = (0, 0, 255);
pub const YELLOW: Color = (255, 255, 0);
pub const CYAN: Color = (0, 255, 255);
pub const MAGENTA: Color = (255, 0, 255);
pub const WHITE: Color = (255, 255, 255);

pub fn set_led_color(color: Color, intensity: u8) {
    // Limiter la puissance à 1 (max 1 sur 255)
    let r = ((color.0 as u32 * intensity as u32) / 255).min(1) as u8;
    let g = ((color.1 as u32 * intensity as u32) / 255).min(1) as u8;
    let b = ((color.2 as u32 * intensity as u32) / 255).min(1) as u8;

    log::info!("LED set_color: requested {:?} intensity {}, applied: ({}, {}, {})", color, intensity, r, g, b);

    let mut guard = LED_DRIVER.lock().unwrap();
    if let Some(ref mut driver) = *guard {
        let led_color = LedPixelColorGrb24::new_with_rgb(r, g, b);
        if let Err(e) = driver.write_blocking(led_color.as_ref().iter().copied()) {
            log::error!("Erreur d'écriture RMT LED: {:?}", e);
        }
    } else {
        log::warn!("LED RMT non initialisée ! Appeler init_led() d'abord.");
    }
}

// ─── Heartbeat Wi-Fi sur GPIO 44 (PWM logicielle, inchangé) ───

static WIFI_STATUS: AtomicU8 = AtomicU8::new(0); // 0 = Connecting, 1 = Connected
static START_HEARTBEAT: Once = Once::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiStatus {
    Connecting,  // Fast blink (600ms)
    Connected,   // Slow pulse (2.5s)
    Off,         // LED off
    Pairing,     // Heartbeat double-pulse
}

pub fn set_wifi_status(status: WifiStatus) {
    let val = match status {
        WifiStatus::Connecting => 0,
        WifiStatus::Connected => 1,
        WifiStatus::Off => 2,
        WifiStatus::Pairing => 3,
    };
    WIFI_STATUS.store(val, Ordering::SeqCst);
    
    START_HEARTBEAT.call_once(|| {
        thread::spawn(move || {
            // Configurer le GPIO 44 en output
            let pin_mask = 1u64 << 44;
            let config = esp_idf_sys::gpio_config_t {
                pin_bit_mask: pin_mask,
                mode: esp_idf_sys::gpio_mode_t_GPIO_MODE_OUTPUT,
                pull_up_en: esp_idf_sys::gpio_pullup_t_GPIO_PULLUP_DISABLE,
                pull_down_en: esp_idf_sys::gpio_pulldown_t_GPIO_PULLDOWN_DISABLE,
                intr_type: esp_idf_sys::gpio_int_type_t_GPIO_INTR_DISABLE,
            };
            unsafe {
                esp_idf_sys::gpio_config(&config);
            }

            let mut t: f64 = 0.0;
            loop {
                let status = WIFI_STATUS.load(Ordering::SeqCst);
                if status == 2 {
                    // Éteindre la LED
                    unsafe {
                        core::ptr::write_volatile(0x6000_401c as *mut u32, 1 << (44 - 32));
                    }
                    thread::sleep(Duration::from_millis(100));
                    continue;
                }

                if status == 3 {
                    // Heartbeat double-pulse (Pairing) — cycle de 1.5s
                    let cycle_duration: f64 = 1.5;
                    let intensity: u8 = if t < 0.08 {
                        // Premier flash court (montée)
                        ((t / 0.08 * std::f64::consts::PI).sin() * 255.0) as u8
                    } else if t < 0.12 {
                        0 // pause courte
                    } else if t < 0.22 {
                        // Second flash plus long
                        (((t - 0.12) / 0.10 * std::f64::consts::PI).sin() * 255.0) as u8
                    } else {
                        0 // pause longue jusqu'à la fin du cycle
                    };

                    let threshold = (intensity as f64 / 255.0 * 10.0).round() as i32;
                    for step in 0..10 {
                        unsafe {
                            if step < threshold {
                                core::ptr::write_volatile(0x6000_4010 as *mut u32, 1 << (44 - 32));
                            } else {
                                core::ptr::write_volatile(0x6000_401c as *mut u32, 1 << (44 - 32));
                            }
                        }
                        thread::sleep(Duration::from_millis(1));
                    }

                    t += 0.010 / cycle_duration;
                    if t >= 1.0 {
                        t -= 1.0;
                    }
                    continue;
                }

                let cycle_duration = if status == 0 {
                    0.6 // Connecting (rapide) -> 600 ms
                } else {
                    2.5 // Connected (lent) -> 2.5 secondes
                };

                let intensity = if t < 0.15 {
                    let angle = (t / 0.15) * std::f64::consts::PI;
                    (angle.sin() * 80.0) as u8
                } else if t < 0.22 {
                    0
                } else if t < 0.40 {
                    let angle = ((t - 0.22) / 0.18) * std::f64::consts::PI;
                    (angle.sin() * 255.0) as u8
                } else {
                    0
                };

                // PWM logicielle : boucle rapide sur 10ms (10 pas de 1ms)
                let threshold = (intensity as f64 / 255.0 * 10.0).round() as i32;
                for step in 0..10 {
                    unsafe {
                        if step < threshold {
                            core::ptr::write_volatile(0x6000_4010 as *mut u32, 1 << (44 - 32));
                        } else {
                            core::ptr::write_volatile(0x6000_401c as *mut u32, 1 << (44 - 32));
                        }
                    }
                    thread::sleep(Duration::from_millis(1));
                }

                t += 0.010 / cycle_duration;
                if t >= 1.0 {
                    t -= 1.0;
                }
            }
        });
    });
}
