use esp_idf_sys as _; // Nécessaire pour initialiser l'écosystème d'exécution ESP-IDF
use esp_idf_hal::peripherals::Peripherals;
use esp_idf_hal::gpio::*;
use esp_idf_hal::i2c::*;
use esp_idf_hal::spi::*;
use esp_idf_hal::ledc::*; // Pour le contrôle PWM (moteur, LEDs, etc.)
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use log::{info, error, warn};

// Importations depuis notre bibliothèque commune
use common::wifi::WifiManager;
use common::ntp::NtpManager;
use common::http_server::HttpServerManager;
use common::ota::OtaManager;
use common::BaseConfig;

fn main() -> anyhow::Result<()> {
    // 1. Initialisation système de base d'ESP-IDF
    esp_idf_svc::log::EspLogger::initialize_default();
    info!("Démarrage du Firmware WhisperEye sur ESP32-S3...");

    let peripherals = Peripherals::take()?;
    let sys_loop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    // 2. Chargement de la configuration de base (Secrets & identifiants)
    let config = BaseConfig::default();

    // 3. Initialisation de la pile réseau (Wifi & Bluetooth BLE)
    let mut wifi_manager = WifiManager::new(peripherals.modem, sys_loop.clone(), nvs.clone())?;
    
    // Tentative de connexion Wifi avec les valeurs par défaut
    // (Dans la vraie vie, l'utilisateur peut configurer le SSID/Mot de passe à la volée via AP)
    if let Err(_e) = wifi_manager.connect(&config.wifi_ssid, &config.wifi_psk) {
        warn!("Impossible de se connecter en mode Station Wifi. Démarrage en mode Point d'Accès (AP)...");
        wifi_manager.start_ap("WhisperEye-AP-Config", "WhisperEye123!")?;
    }
    
    wifi_manager.init_bluetooth()?;

    // 4. Initialisation du client NTP
    let _ntp_manager = NtpManager::new(&config.ntp_server)?;

    // 5. Initialisation du serveur HTTP sécurisé par TOTP
    let _http_server = HttpServerManager::new(80, &config.totp_secret)?;

    // 6. Initialisation du gestionnaire OTA
    let _ota_manager = OtaManager::new()?;

    // ==========================================
    // INITIALISATION DES CAPTEURS & ACTIONNEURS (SPÉCIFIQUE CARTE)
    // ==========================================
    info!("Initialisation des périphériques spécifiques de la carte WhisperEye...");

    // --- A. ÉCRAN SUR PORT SPI ---
    info!("Initialisation du port SPI pour l'écran...");
    let spi_config = SpiConfig::new().baudrate(10.MHz().into());
    let _spi_driver = SpiDeviceDriver::new_single(
        peripherals.spi2,
        peripherals.pins.gpio6, // SCLK
        peripherals.pins.gpio7, // MOSI
        Option::<Gpio5>::None,  // MISO (Non connecté pour l'écran)
        Some(peripherals.pins.gpio18), // CS / Chip Select
        &spi_config,
    )?;

    // --- B. PORT DE COMMUNICATION RS485 ---
    info!("Initialisation du port RS485 (UART)...");
    // Configuration UART avec broche directionnelle de contrôle (RTS / DE) pour RS485 half-duplex
    // ex: peripherals.uart1, peripherals.pins.gpio16 (TX), peripherals.pins.gpio17 (RX), peripherals.pins.gpio15 (DE)

    // --- C. PORT RADIO OPTIONNEL ---
    info!("Initialisation du port Radio optionnel...");
    // ex: Module radio (LoRa / RFM95) connecté sur un bus SPI secondaire ou port UART

    // --- D. CAPTEURS (METRICS) ---
    info!("Initialisation des capteurs (metrics)...");

    // 1. Roue codeuse et poussoir de l'écran
    info!("Configuration de la roue codeuse et du bouton poussoir...");
    let _encoder_a = PinDriver::input(peripherals.pins.gpio8)?;
    let _encoder_b = PinDriver::input(peripherals.pins.gpio9)?;
    let _encoder_push = PinDriver::input(peripherals.pins.gpio12)?;

    // 2. Sensitif périphérique (Entrée tactile capacitive)
    info!("Configuration de l'entrée sensitive périphérique...");
    let _touch_sensor = PinDriver::input(peripherals.pins.gpio13)?;

    // 3. Capteur de tension d'alimentation (Mesure analogique / ADC)
    info!("Configuration du capteur de tension d'alimentation...");
    let _voltage_sensor = PinDriver::input(peripherals.pins.gpio14)?;

    // 4. DS18B20 en 1-Wire
    info!("Initialisation du bus 1-Wire pour sonde DS18B20...");
    let _ds18b20_pin = PinDriver::input_output(peripherals.pins.gpio4)?;

    // 5. I2C (via multiplexeur TCA9548) pour SCD41 & SHT45
    info!("Initialisation du bus I2C principal...");
    let i2c_config = I2cConfig::new().baudrate(400.kHz().into());
    let _i2c_driver = I2cDriver::new(
        peripherals.i2c0,
        peripherals.pins.gpio0, // SDA
        peripherals.pins.gpio1, // SCL
        &i2c_config,
    )?;

    // Note conceptuelle pour la sélection de canaux du multiplexeur TCA9548
    // Canal 0: Capteur de CO2 SCD41
    // Canal 1: Capteur de température et d'humidité SHT45
    let _select_tca9548_channel = |channel: u8| -> Result<(), esp_idf_hal::sys::EspError> {
        let _address = 0x70; // Adresse I2C typique du TCA9548
        let _control_byte = 1 << channel;
        // i2c_driver.write(address, &[control_byte], 1000)?;
        Ok(())
    };

    // --- E. ACTIONNEURS (CMD) ---
    info!("Initialisation des actionneurs...");

    // 1. 2 Relais
    info!("Initialisation des 2 relais...");
    let _relais_1 = PinDriver::output(peripherals.pins.gpio10)?;
    let _relais_2 = PinDriver::output(peripherals.pins.gpio11)?;

    // 2. 1 Moteur double sens ou 2 PWM
    info!("Initialisation du contrôle moteur / sorties PWM...");
    let _pwm_1 = PinDriver::output(peripherals.pins.gpio2)?;
    let _pwm_2 = PinDriver::output(peripherals.pins.gpio3)?;

    // 3. 2 LEDs d'état
    info!("Initialisation des 2 LEDs d'état...");
    let mut led_1 = PinDriver::output(peripherals.pins.gpio19)?;
    let mut led_2 = PinDriver::output(peripherals.pins.gpio20)?;

    // 4. Pin sectionneur d'alimentation
    info!("Initialisation de la broche du sectionneur d'alimentation...");
    let mut _power_disconnect = PinDriver::output(peripherals.pins.gpio21)?;
    _power_disconnect.set_low()?; // Par défaut maintenu actif/connecté

    // --- Boucle Principale Infinie ---
    info!("WhisperEye initialisé avec succès ! Entrée dans la boucle principale...");
    
    let mut state = false;
    loop {
        // Clignotement alterné des deux LEDs comme battement de cœur
        state = !state;
        if state {
            led_1.set_high()?;
            led_2.set_low()?;
        } else {
            led_1.set_low()?;
            led_2.set_high()?;
        }

        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}
