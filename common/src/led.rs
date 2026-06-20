#![allow(deprecated)]
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::sync::Mutex;
use std::sync::Once;
use std::thread;
use std::time::Duration;

use ws2812_esp32_rmt_driver::driver::color::{LedPixelColor, LedPixelColorGrb24};
use ws2812_esp32_rmt_driver::driver::Ws2812Esp32RmtDriver;

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
// 3 = ProvisioningAttempting (bleu clignotant), 4 = ProvisioningOk (bleu)
static LED_AP_STATUS: AtomicU8 = AtomicU8::new(0);
// 0 = Off, 1 = ProvisioningSsid (magenta), 2 = ApPairing (orange clignotant)
pub static MESH_RETRIES_EXHAUSTED: AtomicBool = AtomicBool::new(false);
static START_LED_PATTERN: Once = Once::new();

/// Timestamp (Instant) jusqu'auquel le mode "identify" (blanc rapide) est actif.
/// Stocké comme secondes depuis le boot (u32, suffisant pour ~136 ans).
// Timestamp (UNIX seconds) jusqu'auquel le mode "identify" (blanc rapide) est actif.
// Stocké comme secondes UNIX (u32, suffisant jusqu'en 2106).
static IDENTIFY_SECS: AtomicU32 = AtomicU32::new(0);
static RESET_FLASHING: AtomicBool = AtomicBool::new(false);

pub fn set_reset_flashing(flashing: bool) {
    RESET_FLASHING.store(flashing, Ordering::SeqCst);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedStaStatus {
    None = 0,
    WifiAttempting = 1,
    WifiOk = 2,
    ProvisioningAttempting = 3,
    ProvisioningOk = 4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedApStatus {
    Off = 0,
    ProvisioningSsid = 1,
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

/// Démarre ou réinitialise le mode "identify" (LED blanche clignotement rapide).
/// Remet le compteur à `duration_secs` (pas d'accumulation).
/// Retourne le timestamp UTC (SystemTime) de fin.
pub fn extend_identify(duration_secs: u64) -> std::time::SystemTime {
    let now_unix = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let new_target = (now_unix + duration_secs) as u32;
    IDENTIFY_SECS.store(new_target, Ordering::SeqCst);
    let unix_end = std::time::UNIX_EPOCH + std::time::Duration::from_secs(now_unix + duration_secs);
    log::info!("LED identify set, ends in {}s (UTC: {:?})", duration_secs, unix_end);
    std::time::UNIX_EPOCH + std::time::Duration::from_secs(now_unix + duration_secs)
}

/// Annule immédiatement le mode identify.
pub fn cancel_identify() {
    IDENTIFY_SECS.store(0, Ordering::SeqCst);
    log::info!("LED identify cancelled");
}

/// Retourne le temps restant en secondes pour le mode identify, 0 si inactif.
pub fn identify_remaining_secs() -> u64 {
    let now_unix = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let target = IDENTIFY_SECS.load(Ordering::SeqCst) as u64;
    if target > now_unix { target - now_unix } else { 0 }
}
fn identify_active() -> bool {
    let now_unix = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let target = IDENTIFY_SECS.load(Ordering::SeqCst) as u64;
    target > now_unix
}

fn ensure_pattern_running() {
    START_LED_PATTERN.call_once(|| {
        thread::Builder::new()
            .name("led_pattern".to_string())
            .stack_size(4096)
            .spawn(move || {
                let intensity: u8 = 2; // 2/255
                let identify_intensity: u8 = 200; // ~78% pour le blanc rapide
                let tick = Duration::from_millis(10);
                let pulse_duration_ticks: u32 = 20; // 200ms
                let off_ticks: u32 = 40; // 400ms
                let cycle_ticks: u32 =
                    pulse_duration_ticks + off_ticks + pulse_duration_ticks + off_ticks; // 1200ms
                let mut phase: u32 = 0;

                loop {
                    if RESET_FLASHING.load(Ordering::SeqCst) {
                        thread::sleep(tick);
                        continue;
                    }

                    // Le mode identify (blanc rapide) prend priorité sur tout
                    if identify_active() {
                        // Blanc clignotant rapide : 50ms ON, 50ms OFF
                        let fast_phase = phase % 10; // 10 ticks = 100ms cycle
                        let (r, g, b) = if fast_phase < 5 {
                            (identify_intensity, identify_intensity, identify_intensity)
                        } else {
                            (0, 0, 0)
                        };
                        let mut guard = LED_DRIVER.lock().unwrap();
                        if let Some(ref mut driver) = *guard {
                            let led_color = LedPixelColorGrb24::new_with_rgb(r, g, b);
                            let _ = driver.write_blocking(led_color.as_ref().iter().copied());
                        }
                        drop(guard);
                        thread::sleep(tick);
                        phase = (phase + 1) % cycle_ticks;
                        continue;
                    }

                    let sta = LED_STA_STATUS.load(Ordering::SeqCst);
                    let ap = LED_AP_STATUS.load(Ordering::SeqCst);

                    let (r, g, b) = if phase < pulse_duration_ticks {
                        // --- Pulse 1 : STA ---
                        let sub = phase; // 0..19
                        match sta {
                            1 => {
                                // WifiAttempting: green 50/50/50/50
                                let m = sub % 10; // 0..9
                                if m < 5 {
                                    (0, intensity, 0)
                                } else {
                                    (0, 0, 0)
                                }
                            }
                            2 => {
                                // WifiOk: green 200ms
                                (0, intensity, 0)
                            }
                            3 => {
                                // ProvisioningAttempting: blue 30/50/30/50/30
                                let m = sub;
                                if m < 3 || (m >= 8 && m < 11) || (m >= 16 && m < 19) {
                                    (0, 0, intensity)
                                } else {
                                    (0, 0, 0)
                                }
                            }
                            4 => {
                                // ProvisioningOk: blue 200ms
                                (0, 0, intensity)
                            }
                            _ => {
                                // None: 3 flashs rouges
                                let m = sub; // 0..19
                                if (m < 3) || (m >= 5 && m < 8) || (m >= 10 && m < 13) {
                                    (intensity, 0, 0)
                                } else {
                                    (0, 0, 0)
                                }
                            }
                        }
                    } else if phase < pulse_duration_ticks + off_ticks {
                        // --- 400ms off ---
                        (0, 0, 0)
                    } else if phase < pulse_duration_ticks + off_ticks + pulse_duration_ticks {
                        // --- Pulse 2 : AP ---
                        let sub = phase - pulse_duration_ticks - off_ticks; // 0..19
                        match ap {
                            1 => {
                                // ProvisioningSsid: magenta 200ms
                                (intensity, 0, intensity)
                            }
                            2 => {
                                // ApPairing: orange 30/50/30/50/30
                                let m = sub;
                                if m < 3 || (m >= 8 && m < 11) || (m >= 16 && m < 19) {
                                    (intensity, intensity / 2, 0) // orange ≈ (R=2, G=1, B=0)
                                } else {
                                    (0, 0, 0)
                                }
                            }
                            _ => (0, 0, 0),
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
