#![recursion_limit = "256"]

use esp_idf_sys as _; // Mandatory for linking ESP-IDF
use esp_idf_hal::peripherals::Peripherals;
use esp_idf_hal::gpio::{PinDriver, Pull};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::http::server::{EspHttpServer, Configuration as ServerConfig};
use esp_idf_svc::sntp::EspSntp;
use anyhow::{Result, Context};
use log::info;
use std::thread;
use std::sync::{Arc, Mutex};

pub const WHISPEREYE_BOARD:  &str = "1.0";
pub const CHIP_TYPE:  &str = "ESP32-S3";
pub const FW_VERSION: &str = "1.2.124";

#[allow(dead_code)]
pub const TOTP_SECRET: &str = "Salt-4-Hash-Between-Probe-&-WhisperEye";

pub const AUTHOR_EMAIL: &str = "alban.lopez+whisperEye@gmail.com";
pub const AUTHOR_NAME: &str = "LOPEZ Alban";
pub const AUTHOR_LINK: &str = "https://github.com/sctfic/WhisperEye/blob/main/README.md";



mod wifi;
mod sensors;
mod actuators;
mod web_pages;
mod cron;
mod dynamic_devices;
mod one_wire;
mod i2c;
mod screen;
mod screen_display;
mod screen_browse;
mod radio;
mod board;
mod route;
mod web_handlers;
pub mod touch;

use wifi::{NetManager, NetState};
use common::nvs_storage::NvsStorage;
use actuators::{Actuators, ActuatorsState};
use board::Board;
use i2c::I2c;
use one_wire::OneWire;

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct ConfigPayload {
    wifi_ssid: Option<String>,
    wifi_psk: Option<String>,
    update_url: Option<String>,
    update_interval: Option<String>,
    apply_only: Option<bool>,
    auto_update: Option<bool>,
    totp_secret: Option<String>,
    current_totp_secret: Option<String>,
    ext_name: Option<String>,
    ext_desc: Option<String>,
    ntp_server: Option<String>,
    metrics_url: Option<String>,
    rename_enabled: Option<bool>,
    mesh_channel: Option<i32>,
    mesh_id: Option<String>,
    mesh_pmk: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum UrlValidationState {
    NotChecked,
    Checking,
    Checked(bool),
}

struct CustomLogger;

impl log::Log for CustomLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Debug
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            let marker = match record.level() {
                log::Level::Error => "E",
                log::Level::Warn => "W",
                log::Level::Info => "I",
                log::Level::Debug => "D",
                log::Level::Trace => "V",
            };
            let timestamp = unsafe { esp_idf_sys::esp_log_timestamp() };
            println!("\x1b[35;1m{}\x1b[0m ({}) {}: {}", marker, timestamp, record.target(), record.args());
        }
    }

    fn flush(&self) {}
}

static LOGGER: CustomLogger = CustomLogger;

fn main() -> Result<()> {
    log::set_logger(&LOGGER)
        .map(|()| log::set_max_level(log::LevelFilter::Debug))
        .expect("Failed to initialize custom logger");
    
    unsafe {
        esp_idf_sys::esp_task_wdt_deinit();
    }
    println!("DEBUG: Task Watchdog Timer deinitialized");
    
    let peripherals = Peripherals::take().context("Failed to take ESP32 Peripherals")?;
    println!("DEBUG: Peripherals taken");
    
    let sys_loop = EspSystemEventLoop::take().context("Failed to take System Event Loop")?;
    let nvs_default = EspDefaultNvsPartition::take().context("Failed to take NVS Partition")?;
    
    // Initialize NVS Storage helper
    let nvs_storage = Arc::new(Mutex::new(NvsStorage::new(nvs_default.clone())?));
    println!("DEBUG: NVS Storage initialized");
    let _ = nvs_storage.lock().unwrap().dump_to_log();


    // 1. Initialiser le module de carte mère Board (Touch, VSENSE, ISENSE)
    let board = Arc::new(Mutex::new(Board::init(
        peripherals.adc1,
        peripherals.pins.gpio1,
        peripherals.pins.gpio2,
        peripherals.pins.gpio14,
    )?));
    println!("DEBUG: Board (touch, vsense, isense) initialized");

    // 2. Initialiser les Actuators (RLA, RLB, INA, INB, SWPWR)
    let actuators = Arc::new(Mutex::new(Actuators::init(
        peripherals.pins.gpio48,
        peripherals.pins.gpio47,
        peripherals.pins.gpio21,
        peripherals.pins.gpio36,
        peripherals.pins.gpio35,
    )?));
    println!("DEBUG: Actuators initialized");

    let actuators_state = Arc::new(Mutex::new(ActuatorsState::default()));

    // 2b. Initialiser la LED RMT (GPIO48 est partagé avec RLA)
    #[allow(deprecated)]
    let rmt_channel = unsafe { esp_idf_hal::rmt::CHANNEL0::steal() };
    #[allow(deprecated)]
    let led_pin = unsafe { esp_idf_hal::gpio::Gpio48::steal() };
    if let Err(e) = common::led::init_led(rmt_channel, led_pin) {
        println!("WARNING: Failed to init LED: {:?}", e);
    }

    // Restauration des états/valeurs PWM sauvegardés dans devicesKnow
    {
        let mut acts = actuators.lock().unwrap();
        let mut state = actuators_state.lock().unwrap();
        let registry = crate::dynamic_devices::DeviceRegistry::new(Arc::clone(&nvs_storage));
        let map = registry.load_registry();

        // Restaurer le Pont H (H0)
        if let Some(entry) = map.get("H0") {
            let inv = entry.inverseur.unwrap_or(0);
            let speed_a = entry.ina.as_ref().map(|i| i.pwm_val).unwrap_or(30);
            let speed_b = entry.inb.as_ref().map(|i| i.pwm_val).unwrap_or(30);
            state.H0.inverseur = inv;
            state.H0.speed_a = speed_a;
            state.H0.speed_b = speed_b;
            let _ = acts.write_h0(&state.H0);
            log::info!("[BOOT] Restored H0: inverseur={}, speed_a={}%, speed_b={}%", inv, speed_a, speed_b);
        }
        // Restaurer RLA
        if let Some(entry) = map.get("rla") {
            if let Some(pwm) = entry.pwm_val {
                let is_active = pwm > 0;
                let _ = acts.write("rla", is_active);
                if is_active {
                    let _ = acts.relay_a.set_speed(pwm as i32);
                }
                state.rla = is_active;
                log::info!("[BOOT] Restored RLA state: {}, speed: {}%", is_active, acts.relay_a.get_speed());
            }
        }
        // Restaurer RLB
        if let Some(entry) = map.get("rlb") {
            if let Some(pwm) = entry.pwm_val {
                let is_active = pwm > 0;
                let _ = acts.write("rlb", is_active);
                if is_active {
                    let _ = acts.relay_b.set_speed(pwm as i32);
                }
                state.rlb = is_active;
                log::info!("[BOOT] Restored RLB state: {}, speed: {}%", is_active, acts.relay_b.get_speed());
            }
        }
        // Restaurer SCREEN
        if let Some(entry) = map.get("screen") {
            if let Some(pwm) = entry.pwm_val {
                let mut storage = nvs_storage.lock().unwrap();
                let _ = storage.set_i32("scrBrightness", pwm as i32);
                log::info!("[BOOT] Restored SCREEN brightness to: {}%", pwm);
            }
        }
    }

    // 3. Initialiser le bus I2C (TCA9548A et scan dynamique)
    let i2c = Arc::new(Mutex::new(I2c::init()?));
    println!("DEBUG: I2C initialized");

    info!("\x1b[35mWhisperEye Production Application Starting Up (Version {})...\x1b[0m", FW_VERSION);

    // Wi-Fi Event Loop subscription
    let _wifi_event_sub = sys_loop.subscribe::<esp_idf_svc::wifi::WifiEvent, _>(|event| {
        if let esp_idf_svc::wifi::WifiEvent::ApStaConnected(info) = event {
            let mac = info.mac();
            info!("[WIFI AP] Station connectée -> MAC: {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
                mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);
            wifi::AP_CLIENT_CONNECTED.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    })?;

    let url_validation_state = Arc::new(Mutex::new(UrlValidationState::NotChecked));
    
    // Configurer la version et otaRetry
    {
        let mut storage = nvs_storage.lock().unwrap();
        let saved_version = storage.get_str("fwVersion")?.unwrap_or_else(|| "empty".to_string());
        if saved_version != FW_VERSION {
            let _ = storage.set_str("fwVersion", FW_VERSION);
            let now_str = web_handlers::get_formatted_time();
            let _ = storage.set_str("lastOtaSuccess", &now_str);
        }
        let _ = storage.set_i32("otaRetry", -1);
    }

    let boot_pin_gpio = peripherals.pins.gpio0;
    let modem = peripherals.modem;

    // Initialisation du bus OneWire
    let onewr_pin = peripherals.pins.gpio39;
    println!("DEBUG: Starting 1-Wire bus initialization...");
    let (discovered_probes, onewire_bus) = if let Ok(mut ow) = OneWire::new(onewr_pin) {
        let probes = ow.search_roms();
        for probe in &probes {
            let _ = ow.verify_authenticity(probe);
            let _ = ow.configure_resolution_10bit(probe); // 0.25
        }
        (probes, Some(Arc::new(Mutex::new(ow))))
    } else {
        (Vec::new(), None)
    };
    let discovered_probes = Arc::new(discovered_probes);
    one_wire::ONEWIRE_DEVICES_COUNT.store(discovered_probes.len() as u8, std::sync::atomic::Ordering::Relaxed);
    info!("[1-Wire] Scan terminé : {} sonde(s) DS18B20 détectée(s). Bus initialisé: {}", discovered_probes.len(), onewire_bus.is_some());
    if !discovered_probes.is_empty() {
        for (i, addr) in discovered_probes.iter().enumerate() {
            info!("[1-Wire]   Sonde #{}: 0x{}", i + 1, addr.to_uppercase());
        }
    }

    // Enregistrer les périphériques détectés
    {
        let mut registry = dynamic_devices::DeviceRegistry::new(Arc::clone(&nvs_storage));
        let _ = registry.scan_and_register((*discovered_probes).clone(), &i2c);
    }

    let (mesh_channel, mesh_ssid, mesh_pmk) = {
        let storage = nvs_storage.lock().unwrap();
        let channel = storage.get_i32("wifiChannel")?.unwrap_or(11) as u8;
        let ssid = "Esp32MeshNetwork".to_string();
        let pmk = "Mesh-IoT@Espressif!".to_string();
        (channel, ssid, pmk)
    };

    let wifi_manager = NetManager::new(modem, sys_loop.clone(), nvs_default, mesh_ssid, mesh_pmk, mesh_channel)?;
    let wifi_manager = Arc::new(Mutex::new(wifi_manager));

    // 4. Initialisation et démarrage de l'IHM / Écran ST7789
    println!("DEBUG: Initializing screen...");
    match screen::Screen::init(
        peripherals.spi2,
        peripherals.pins.gpio7,
        peripherals.pins.gpio15,
        peripherals.pins.gpio16,
        peripherals.pins.gpio4,
        peripherals.pins.gpio5,
        peripherals.pins.gpio6,
        peripherals.pins.gpio17,
        peripherals.pins.gpio18,
        peripherals.pins.gpio8,
        peripherals.pins.gpio3,
    ) {
        Ok((screen, display)) => {
            println!("DEBUG: Screen initialized successfully, preparing IHM thread...");
            let board_clone = Arc::clone(&board);
            let actuators_clone = Arc::clone(&actuators);
            let state_clone = Arc::clone(&actuators_state);
            let nvs_clone = Arc::clone(&nvs_storage);
            let wifi_clone = Arc::clone(&wifi_manager);
            println!("DEBUG: Spawning screen_ihm thread (32KB stack)...");
            match thread::Builder::new()
                .name("screen_ihm".to_string())
                .stack_size(32768)
                .spawn(move || {
                    if let Err(e) = screen_display::run_ihm(screen, display, board_clone, actuators_clone, state_clone, nvs_clone, wifi_clone) {
                        log::error!("Erreur fatale dans le thread IHM : {:?}", e);
                    }
                })
            {
                Ok(_handle) => {
                    println!("DEBUG: screen_ihm thread spawned successfully.");
                    info!("Écran et thread IHM démarrés avec succès !");
                }
                Err(e) => {
                    log::error!("Échec du spawn du thread IHM : {:?}. Main continue.", e);
                }
            }
        }
        Err(e) => {
            log::error!("Échec de l'initialisation de l'écran : {:?}", e);
        }
    }

    // Bouton de boot pour le pairing et le reset d'usine
    let boot_pin = PinDriver::input(boot_pin_gpio, Pull::Up)?;
    let wifi_manager_boot = Arc::clone(&wifi_manager);
    let nvs_boot = Arc::clone(&nvs_storage);

    thread::Builder::new()
        .name("boot_button_worker".to_string())
        .stack_size(4096)
        .spawn(move || {
            let mut pressed_ticks = 0;
            loop {
                if boot_pin.is_low() {
                    pressed_ticks += 1;
                    if pressed_ticks == 20 {
                        info!("BOOT button held for 2 seconds! Triggering pairing mode.");
                        let mut net = wifi_manager_boot.lock().unwrap();
                        net.state = NetState::ApPairing;
                        net.pairing_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(120));
                        let _ = net.setup_provisioning_ap();
                    } else if pressed_ticks == 40 {
                        info!("BOOT button held for 4 seconds! Performing factory reset.");
                        common::led::set_reset_flashing(true);
                        thread::sleep(std::time::Duration::from_secs(1));
                        if boot_pin.is_low() {
                            let mut storage = nvs_boot.lock().unwrap();
                            let _ = web_handlers::perform_factory_reset(&mut storage);
                            unsafe { esp_idf_sys::esp_restart(); }
                        }
                        common::led::set_reset_flashing(false);
                    }
                } else {
                    pressed_ticks = 0;
                }
                thread::sleep(std::time::Duration::from_millis(100));
            }
        })?;

    {
        let mut net = wifi_manager.lock().unwrap();
        net.state = NetState::WifiPreferred;
        net.last_state_change = std::time::Instant::now();
    }

    // [Junior Dev Note] : L'initialisation SNTP est désormais déclenchée UNIQUEMENT après
    // connexion WiFi réussie, dans le thread net_controller (wifi.rs).
    // Cela évite de tenter une synchronisation NTP sans réseau disponible.

    NetManager::start_controller_thread(
        Arc::clone(&wifi_manager),
        Arc::clone(&nvs_storage),
    )?;

    let scheduled_actions = Arc::new(Mutex::new(actuators::ScheduledActions::default()));

    // Lancer le cron scheduler (on passe le nouvel actuators, one_wire et le board)
    let cron_handle = cron::spawn_cron_scheduler(
        Arc::clone(&nvs_storage),
        Arc::clone(&wifi_manager),
        Arc::clone(&actuators_state),
        Arc::clone(&actuators),
        Arc::clone(&scheduled_actions),
        onewire_bus.clone(),
        Arc::clone(&i2c),
        Arc::clone(&board),
    )?;

    // Serveur Web
    let mut server = EspHttpServer::new(&ServerConfig::default())
        .context("Failed to start HTTP server")?;

    route::register_routes(
        &mut server,
        Arc::clone(&nvs_storage),
        Arc::clone(&wifi_manager),
        Arc::clone(&url_validation_state),
        Arc::clone(&discovered_probes),
        Arc::clone(&actuators_state),
        Arc::clone(&actuators),
        Arc::clone(&scheduled_actions),
        Arc::clone(&board),
        Arc::clone(&i2c),
        cron_handle,
    )?;

    loop {
        thread::sleep(std::time::Duration::from_secs(60));
    }
}




















































































































































































































