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
    let r = ((color.0 as u32 * intensity as u32) / 255).min(255) as u8;
    let g = ((color.1 as u32 * intensity as u32) / 255).min(255) as u8;
    let b = ((color.2 as u32 * intensity as u32) / 255).min(255) as u8;

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

// ─── Pattern LED RGB 2-impulsions (STA + AP), intensité 2/255 ───

static LED_STA_STATUS: AtomicU8 = AtomicU8::new(0);
// 0 = None (rouge), 1 = WifiAttempting (vert clignotant), 2 = WifiOk (vert),
// 3 = MeshAttempting (bleu clignotant), 4 = MeshOk (bleu)
static LED_AP_STATUS: AtomicU8 = AtomicU8::new(0);
// 0 = Off, 1 = MeshSsid (magenta), 2 = ApPairing (orange clignotant)
static START_LED_PATTERN: Once = Once::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedStaStatus {
    None = 0,
    WifiAttempting = 1,
    WifiOk = 2,
    MeshAttempting = 3,
    MeshOk = 4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedApStatus {
    Off = 0,
    MeshSsid = 1,
    ApPairing = 2,
}

pub fn set_sta_status(status: LedStaStatus) {
    LED_STA_STATUS.store(status as u8, Ordering::SeqCst);
    ensure_pattern_running();
}

pub fn set_ap_status(status: LedApStatus) {
    LED_AP_STATUS.store(status as u8, Ordering::SeqCst);
    ensure_pattern_running();
}

fn ensure_pattern_running() {
    START_LED_PATTERN.call_once(|| {
        thread::Builder::new()
            .name("led_pattern".to_string())
            .stack_size(4096)
            .spawn(move || {
                let intensity: u8 = 2; // 2/255
                let tick = Duration::from_millis(10);
                let pulse_duration_ticks: u32 = 20;  // 200ms
                let off_ticks: u32 = 40;              // 400ms
                let cycle_ticks: u32 = pulse_duration_ticks + off_ticks + pulse_duration_ticks + off_ticks; // 1200ms
                let mut phase: u32 = 0;

                loop {
                    let sta = LED_STA_STATUS.load(Ordering::SeqCst);
                    let ap = LED_AP_STATUS.load(Ordering::SeqCst);

                    let (r, g, b) = if phase < pulse_duration_ticks {
                        // --- Pulse 1 : STA ---
                        let sub = phase; // 0..19
                        match sta {
                            1 => { // WifiAttempting: green 50/50/50/50
                                let m = sub % 10; // 0..9
                                if m < 5 { (0, intensity, 0) } else { (0, 0, 0) }
                            }
                            2 => { // WifiOk: green 200ms
                                (0, intensity, 0)
                            }
                            3 => { // MeshAttempting: blue 30/50/30/50/30
                                let m = sub;
                                if m < 3 || (m >= 8 && m < 11) || (m >= 16 && m < 19) {
                                    (0, 0, intensity)
                                } else {
                                    (0, 0, 0)
                                }
                            }
                            4 => { // MeshOk: blue 200ms
                                (0, 0, intensity)
                            }
                            _ => { // None: red 200ms
                                (intensity, 0, 0)
                            }
                        }
                    } else if phase < pulse_duration_ticks + off_ticks {
                        // --- 400ms off ---
                        (0, 0, 0)
                    } else if phase < pulse_duration_ticks + off_ticks + pulse_duration_ticks {
                        // --- Pulse 2 : AP ---
                        let sub = phase - pulse_duration_ticks - off_ticks; // 0..19
                        match ap {
                            1 => { // MeshSsid: magenta 200ms
                                (intensity, 0, intensity)
                            }
                            2 => { // ApPairing: orange 30/50/30/50/30
                                let m = sub;
                                if m < 3 || (m >= 8 && m < 11) || (m >= 16 && m < 19) {
                                    (intensity, intensity / 2, 0) // orange ≈ (R=2, G=1, B=0)
                                } else {
                                    (0, 0, 0)
                                }
                            }
                            _ => { (0, 0, 0) }
                        }
                    } else {
                        // --- 400ms off ---
                        (0, 0, 0)
                    };

                    // Écrire sur le WS2812
                    let mut guard = LED_DRIVER.lock().unwrap();
                    if let Some(ref mut driver) = *guard {
                        let led_color = LedPixelColorGrb24::new_with_rgb(r, g, b);
                        let _ = driver.write_blocking(led_color.as_ref().iter().copied());
                    }
                    drop(guard);

                    thread::sleep(tick);
                    phase = (phase + 1) % cycle_ticks;
                }
            })
            .expect("Failed to spawn LED pattern thread");
    });
}
