# Guide d'implémentation du matériel WhisperEye

Ce document explique comment utiliser et configurer les différents périphériques matériels de la carte **WhisperEye**, sur la base de notre implémentation logicielle actuelle (Rust / ESP-IDF). Il est destiné à l'équipe de développement pour leur fournir les principes d'implémentation et des extraits de code associés.

---

## 1. Scan I2C à travers le multiplexeur (TCA954 / TCA9548A)

Le bus I2C principal de l'ESP32-S3 est connecté à un multiplexeur de canaux I2C (TCA954 ou TCA9548A).

### Brochage (Pins) et Adressage
* **Périphérique I2C principal** : `I2C0`
* **Broches I2C (SDA/SCL)** :
  * **Polarité Standard (Directe)** : SDA = `GPIO38`, SCL = `GPIO37`
  * **Polarité Inversée** : SDA = `GPIO37`, SCL = `GPIO38`
* **Adresse I2C du multiplexeur** : `0x74`
* **Canaux utilisés** : `0`, `1`, `2`, `3`, `4`, `7`

### Principe et extrait de code
Pour interroger un canal spécifique, on écrit le bit correspondant sur l'adresse `0x74` du multiplexeur avant d'effectuer les écritures/lectures classiques.

```rust
// 1. Sélectionner le canal sur le multiplexeur
fn select_channel(driver: &mut I2cDriver<'static>, channel: u8) -> Result<(), esp_idf_sys::EspError> {
    let control_byte = 1 << channel;
    driver.write(0x74, &[control_byte], 50)
}

// 2. Prober un périphérique à une adresse spécifique
let mut has_device = false;
if select_channel(&mut driver, channel).is_ok() {
    // Écriture bidon pour tester la présence (ACK)
    if driver.write(device_addr, &[0x00], 50).is_ok() {
        has_device = true; // ACK reçu, périphérique présent !
    }
}
```

---

## 2. Écran d'affichage (ST7789)

L'écran ST7789 (240x320 pixels) est configuré en mode paysage inversé avec rétroéclairage dynamique.

### Brochage (Pins)
* **Périphérique SPI** : `SPI2`
* **Horloge (SCLK)** : `GPIO7`
* **Données (SDA/MOSI)** : `GPIO15`
* **RST (Reset physique)** : `GPIO16`
* **DC (Data/Command)** : `GPIO4`
* **CS (Chip Select)** : `GPIO5`
* **Rétroéclairage (BLK)** : `GPIO6`
* **Roue codeuse** : A = `GPIO17`, B = `GPIO18`
* **Bouton BTN2** : `GPIO8`
* **Bouton BTN3** : `GPIO3`

### Principe et extrait de code

#### A. Rétroéclairage PWM (Luminosité)
Le signal de rétroéclairage est modulé en PWM à 5 kHz :
```rust
let timer0 = unsafe { ledc::TIMER0::steal() };
let channel0 = unsafe { ledc::CHANNEL0::steal() };
let mut blk_pwm = LedcDriver::new(
    channel0,
    LedcTimerDriver::new(timer0, &TimerConfig::new().frequency(5.kHz().into()))?,
    blk_pin,
)?;

// Appliquer une luminosité de 0 à 100%
let max_duty = blk_pwm.get_max_duty();
blk_pwm.set_duty(max_duty * duty_pct / 100)?;
```

#### B. Décodage de la roue codeuse (Quadrature)
La rotation est échantillonnée à 1 ms à l'aide d'une table d'état à quadrature complète :
```rust
// Table de décodage Gray code (Index : état_précédent << 2 | état_actuel)
const ENCODER_STATES: [i8; 16] = [
    0, 1, -1, 0, -1, 0, 0, 1, 1, 0, 0, -1, 0, -1, 1, 0
];

// Boucle de lecture
let a = btn0_driver.is_high();
let b = btn1_driver.is_high();
let current_state = ((a as u8) << 1) | (b as u8);

if current_state != last_state {
    let change = ENCODER_STATES[((last_state << 2) | current_state) as usize];
    acc_steps += change;
    
    if acc_steps >= 4 {
        // Incrémenter la luminosité de +5%
        acc_steps -= 4;
    } else if acc_steps <= -4 {
        // Décrémenter la luminosité de -5%
        acc_steps += 4;
    }
    last_state = current_state;
}
```

#### C. Gestion des Boutons (BTN2 et BTN3)
* **BTN2** réinitialise la luminosité de l'écran à `20%` :
```rust
if last_btn2 && !btn2_val {
    brightness_atomic.store(20, Ordering::Relaxed);
}
```
* **BTN3** gère deux actions selon la durée :
  * *Appui court (front descendant)* : Cycle l'état des sorties moteur INA/INB.
  * *Appui long (2 secondes)* : Coupe le commutateur `SWPWR` (GPIO21).

---

## 3. Contrôle moteur / Pont en H (INA et INB)

### Brochage (Pins)
* **INA** : `GPIO36` (Sortie PWM - LEDC Channel 1)
* **INB** : `GPIO35` (Sortie PWM - LEDC Channel 2)

### Principe et extrait de code
Les entrées du pont en H (DRV8701) sont pilotées en PWM à 10 kHz à l'aide du périphérique LEDC (timers 1 et 2). 

Pour simplifier l'intégration sans modifier les appels existants, un wrapper `MotorPwmPin` est implémenté. Il conserve l'état logique (`High`/`Low`) et reçoit dynamiquement les changements de vitesse (pourcentage basé sur la luminosité de l'écran, réglé via l'encodeur rotatif).

```rust
use esp_idf_hal::ledc::*;
use esp_idf_hal::units::*;

pub struct MotorPwmPin {
    driver: LedcDriver<'static>,
    active: bool,
    speed_pct: i32,
}

impl MotorPwmPin {
    pub fn new(driver: LedcDriver<'static>) -> Self {
        Self { driver, active: false, speed_pct: 20 }
    }

    pub fn set_level(&mut self, level: Level) -> Result<(), EspError> {
        self.active = match level {
            Level::High => true,
            Level::Low => false,
        };
        self.update_duty()
    }

    pub fn set_speed(&mut self, speed_pct: i32) -> Result<(), EspError> {
        self.speed_pct = speed_pct;
        self.update_duty()
    }

    fn update_duty(&mut self) -> Result<(), EspError> {
        let max_duty = self.driver.get_max_duty();
        let target_duty = if self.active {
            (max_duty as u64 * self.speed_pct as u64 / 100) as u32
        } else {
            0
        };
        self.driver.set_duty(target_duty)
    }
}

// Initialisation (à 10 kHz) :
let timer_config = config::TimerConfig::new().frequency(10.kHz().into());
let timer_driver1 = LedcTimerDriver::new(timer1, &timer_config)?;
let ledc_ina = LedcDriver::new(channel1, timer_driver1, gpio36)?;
let mut ina = MotorPwmPin::new(ledc_ina);

// Changement de vitesse en direct (par exemple lorsque la luminosité change) :
devs.ina.set_speed(brightness_pct)?;
```

---

## 4. Capteur tactile (TOUCH)

### Brochage (Pins)
* **TOUCH** : `GPIO14` (entrée capacitive)

### Principe et extrait de code
Utilise le capteur capacitif interne de l'ESP32-S3. Une calibration au démarrage permet de définir un seuil.

```rust
// 1. Calibration au démarrage (Calcul de la moyenne)
let mut sum = 0u32;
for _ in 0..50 {
    sum += touch_pad_read(touch_channel)?;
}
let threshold = (sum / 50) - 2000; // Marge de sensibilité

// 2. Détection
let raw_value = touch_pad_read(touch_channel)?;
let is_pressed = raw_value < threshold; // La capacité augmente, réduisant la valeur lue
```

---

## 5. Commutateur d'alimentation générale (SWPWR)

### Brochage (Pins)
* **SWPWR** : `GPIO21` (Sortie numérique)

### Principe et extrait de code
Commande l'alimentation des lignes de puissance esclaves (5V/3.3V) des capteurs, 1Wire, radio et écran.

```rust
// Couper l'alimentation (Mise en sommeil de la carte)
devs.sw_pwr.set_low()?;

// Réactiver l'alimentation (Sur détection tactile TOUCH)
if current_touch && !last_touch_state {
    devs.sw_pwr.set_high()?;
}
```

---

## 6. Mesures analogiques VSENSE et ISENSE (ADC)

Surveille la tension générale de la carte et le courant consommé par le pont en H.

### Brochage (Pins)
* **VSENSE** : `GPIO1` (Entrée analogique ADC1 Channel 0)
* **ISENSE** : `GPIO2` (Entrée analogique ADC1 Channel 1)

### Principe et extrait de code
Utilise le pilote monocoup d'ADC1 (`oneshot::AdcDriver`) avec une atténuation de 12 dB (qui remplace l'ancienne notation 11 dB) pour étendre la dynamique de mesure jusqu'à ~3.1V.

```rust
use esp_idf_hal::adc::oneshot::*;

// 1. Initialiser le pilote ADC1 et ses canaux avec atténuation de 12 dB et calibration Curve
let adc1_driver = AdcDriver::new(peripherals.adc1)?;
let adc_config = esp_idf_hal::adc::oneshot::config::AdcChannelConfig {
    attenuation: esp_idf_hal::adc::attenuation::DB_2_5,
    calibration: esp_idf_hal::adc::oneshot::config::Calibration::Curve,
    ..Default::default()
};
let mut vsense_channel = AdcChannelDriver::new(&adc1_driver, gpio1, &adc_config)?;
let mut isense_channel = AdcChannelDriver::new(&adc1_driver, gpio2, &adc_config)?;

// 2. Lecture périodique (toutes les 100 ms)
// Lecture calibrée en millivolts convertie en volts réels
if let Ok(mv) = adc1_driver.read(&mut vsense_channel) {
    // VSENSE sur pont diviseur : 47 kOhms / (1000 kOhms + 47 kOhms) = 0.04489 (4.49%)
    // La tension mesurée (millivolts) est divisée par 1000 (V) puis multipliée par 22.27
    let vsense_volts = (mv as f32) * 22.27 / 1000.0;
}

// Lecture de la valeur brute (0..4095 pour 12 bits)
if let Ok(raw) = adc1_driver.read_raw(&mut vsense_channel) {
    let vsense_raw = raw;
}
```

---

## 7. Bus 1-Wire (DS18B20)

### Brochage (Pins)
* **Bus 1-Wire (Data)** : `GPIO39`
* **Résistance de pull-up** : Externe physique de `1 kΩ` raccordée au 3.3V (alimentation 3 fils : VDD=3.3V, GND, Data=GPIO39).

### Problème spécifique du GPIO 39 (JTAG)
Sur l'ESP32-S3, le **GPIO 39** est configuré par défaut au démarrage comme broche JTAG (`MTCK`). Tant que la fonction JTAG est active sur cette broche, le périphérique JTAG matériel garde le contrôle exclusif de la pin, l'empêchant de descendre à 0V (elle reste bloquée à 3.3V) même si l'application appelle `.set_low()`.

Pour résoudre ce conflit et libérer la broche afin de l'utiliser en GPIO standard, il est indispensable d'appeler la fonction de bas niveau d'initialisation de l'ESP-IDF `gpio_reset_pin(39)` avant d'initialiser le pilote :

```rust
use esp_idf_hal::gpio::{PinDriver, InputOutput, Gpio39, Pull};
use esp_idf_sys as sys;

pub fn init_onewire(pin: Gpio39<'static>) -> Result<PinDriver<'static, InputOutput>, anyhow::Error> {
    // 1. Libérer le GPIO 39 du JTAG matériel
    unsafe {
        sys::gpio_reset_pin(39);
    }
    
    // 2. Initialiser la broche en Open-Drain standard
    let mut pin_driver = PinDriver::input_output_od(pin, Pull::Up)?;
    pin_driver.set_high()?;
    Ok(pin_driver)
}
```

### Bonnes pratiques et clés de succès pour la recherche multi-capteurs (Search ROM)
Pour que la recherche de plusieurs périphériques (Search ROM 0xF0) fonctionne de façon fiable en bit-banging pure Rust, trois facteurs critiques ont été identifiés et implémentés :

1. **Mémoire du chemin de collision (`previous_rom`)** :
   La variable mémorisant la ROM du capteur précédent doit être définie **en dehors** de la boucle `while !last_device_flag`. Si le tableau de ROM est réinitialisé à chaque itération du `while`, l'algorithme perd l'historique du chemin exploré. Lors des collisions situées avant `last_discrepancy`, il choisira toujours le bit `0` par défaut au lieu de suivre la structure de la ROM précédente, bloquant la détection en boucle sur le tout premier capteur trouvé.

2. **Ajustement du timing de lecture (`read_bit`)** :
   Pour fiabiliser la détection de collisions (lorsque le bit brut et son complément sont tous les deux lus à `0`), le timing d'échantillonnage doit se situer précisément au milieu de la fenêtre de validité (qui dure 15 µs).
   * **Timing optimisé** : Tirer le bus à bas pendant **2 µs** (pour signaler le slot), libérer la ligne, puis attendre **8 µs** (total 10 µs du slot) avant de lire l'état de la broche. Enfin, attendre **60 µs** pour laisser le slot se terminer. Ce timing à 10 µs évite de lire le bus trop tard lorsque la ligne est déjà remontée, ce qui masquerait les collisions.

3. **Garde-fou anti-doublon** :
   Si pour une raison de bruit physique ou de glissement temporel l'algorithme lit à nouveau une adresse ROM déjà enregistrée, la recherche doit immédiatement s'interrompre (`break` ou `last_device_flag = true`) pour éviter de boucler indéfiniment.