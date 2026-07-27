# Guide pas-à-pas : Ajouter un nouveau capteur dans WhisperEye (`HowTo_NewSensor.md`)

Ce document détaille l'ensemble des étapes nécessaires pour intégrer un nouveau capteur matériel (I2C, 1-Wire ou ADC) dans l'architecture modulaire du firmware WhisperEye.

---

## 📌 Vue d'ensemble de l'architecture de collecte

```text
[ Capteur Physique (ex: SCD41, BME280, SHT45) ]
                    │
                    ▼  (I2C Bus / Multiplexeur TCA9548A)
[ Driver spécifique : src/i2c/i2c_xxx.rs ]
                    │
                    ▼  (Aggrégation dans I2cBus)
[ Bus I2C principal : src/i2c.rs ]
                    │
                    ▼  (Lecture périodique dans Cron)
[ Tâche d'arrière-plan : src/cron.rs ]
                    │
                    ├──► [ Registre dynamique & Formules NVS : src/dynamic_devices.rs ]
                    │
                    ├──► [ API REST JSON : /api/sensors & /api/peripherals ]
                    │
                    └──► [ Interface IHM écran physique : src/screen_browse.rs ]
```

---

## 🚀 Étape 1 : Créer le module driver I2C (`src/i2c/i2c_votre_capteur.rs`)

Créez un nouveau fichier dans `production_app/src/i2c/i2c_votre_capteur.rs`.

### 1.1 Exemple de structure de code
```rust
use esp_idf_hal::i2c::I2cDriver;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8};

// Adresse(s) I2C par défaut du composant
pub const DETECT_ADDRESSES: &[u8] = &[0x62];

// Variables globales statiques pour l'accès rapide
pub static CAPTEUR_FOUND: AtomicBool = AtomicBool::new(false);
pub static CAPTEUR_VALEUR: Mutex<f32> = Mutex::new(-255.0);

// Structure des grandeurs physiques mesurées
#[derive(Debug, Clone, serde::Serialize)]
pub struct CapteurReadings {
    pub valeur_physique: f32,
}

pub struct I2cCapteur {
    pub channel: u8,
    pub address: u8,
    pub is_found: bool,
}

impl I2cCapteur {
    pub fn new(channel: u8, address: u8) -> Self {
        Self { channel, address, is_found: false }
    }

    pub fn init(&mut self, driver: &mut I2cDriver<'static>) -> Result<(), anyhow::Error> {
        log::info!("Initialisation du capteur à l'adresse 0x{:02x}...", self.address);
        // Envoi des commandes de configuration I2C initiales (ex: mode de mesure)
        self.is_found = true;
        CAPTEUR_FOUND.store(true, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    pub fn read_value(&mut self, driver: &mut I2cDriver<'static>) -> Option<CapteurReadings> {
        // 1. Écrire la commande de lecture ou vérifier si la donnée est prête (Data Ready)
        // 2. Lire les octets bruts sur le bus I2C
        // 3. Vérifier la somme de contrôle CRC (ex: Sensirion CRC-8)
        // 4. Convertir la valeur brute selon la formule du datasheet
        Some(CapteurReadings { valeur_physique: 25.0 })
    }
}
```

### ⚠️ Recommandation sur le mode de lecture (Consommation & Durabilité)
Pour les capteurs NDIR ou photoacoustiques (comme le **SCD40/SCD41**), faites attention au mode d'acquisition :
* **Mode Périodique continu (5s)** : Consomme ~15–17 mA continus et accélère l'usure de l'émetteur infrarouge.
* **Mode Low Power (30s)** : Réduit la consommation à ~3.2 mA (~80% d'économie).
* **Mode Mesure Unique (Single Shot)** : Idéal si le Cron ne lit le capteur que toutes les minutes ou toutes les 5 minutes. Le capteur reste en veille profonde (< 0.15 mA) entre chaque mesure.

---

## 🔌 Étape 2 : Déclarer le module dans le Bus I2C (`src/i2c.rs`)

Dans `production_app/src/i2c.rs` :

1. Déclarez le sous-module en haut du fichier :
   ```rust
   pub mod i2c_votre_capteur;
   ```
2. Ajoutez la liste des instances dans la structure `I2cBus` :
   ```rust
   pub struct I2cBus {
       pub vos_capteurs: Vec<i2c_votre_capteur::I2cCapteur>,
   }
   ```
3. Dans `detect_and_init()`, ajoutez l'adresse I2C à la boucle de scan :
   ```rust
   else if i2c_votre_capteur::DETECT_ADDRESSES.contains(&addr) {
       let mut dev = i2c_votre_capteur::I2cCapteur::new(channel, addr);
       if dev.init(&mut driver).is_ok() {
           self.vos_capteurs.push(dev);
       }
   }
   ```
4. Dans `read_value()`, appelez la méthode de lecture et retournez le résultat dans le tuple de mesures.

---

## 📊 Étape 3 : Ajouter les champs dans `SensorReadings` (`src/sensors.rs`)

Dans `production_app/src/sensors.rs` :

1. Ajoutez les champs de mesure dans la structure `SensorReadings` :
   ```rust
   pub struct SensorReadings {
       pub votre_mesure: f32,
   }
   ```
2. Ajoutez l'entrée correspondant dans la sérialisation JSON (`impl serde::Serialize`) :
   ```rust
   map.serialize_entry("i2c:0:0x62_CO2", &self.votre_mesure)?;
   ```

---

## ⚙️ Étape 4 : Déclarer le composant dans le Registre Dynamique (`src/dynamic_devices.rs`)

Dans `production_app/src/dynamic_devices.rs` :

1. Dans la fonction `detect_dynamic_peripherals()`, ajoutez la création automatique des entrées dynamiques lorsque le capteur est présent sur une adresse I2C :
   ```rust
   } else if addr == 0x62 {
       let id_c = format!("i2c:{}:0x{:02x}_CO2", channel, addr);
       let mut entry_c = saved.remove(&id_c).unwrap_or_else(|| make_default(format!("SCD41-CO2 (i2c:{}:0x{:02x})", channel, addr), false, true, Some(addr_str.clone())));
       entry_c.present = true;
       updated.insert(id_c, entry_c);
   }
   ```

### 🏷️ Règle de nommage standardisée des suffixes
Pour garantir une compatibilité automatique avec les unités et les graphiques IHM :
* `_T` : Température (°C)
* `_H` : Humidité (%RH)
* `_P` : Pression (hPa)
* `_CO2` : Dioxyde de Carbone (ppm)

---

## ⏱️ Étape 5 : Intégrer la lecture dans la boucle Cron (`src/cron.rs`)

Dans `production_app/src/cron.rs` :

1. Dans la boucle de collecte I2C, récupérez les valeurs mesurées et affectez-les à la structure `readings`.
2. Formattez une ligne de journalisation propre dans la console :
   ```rust
   lines.push(format!("  SCD41 : CO2={} ppm  Temp={:.1}C  Hum={:.1}%", readings.co2_scd41, readings.temp_scd41, readings.hum_scd41));
   ```

---

## 🖥️ Étape 6 : Rendu automatique Frontend Web & Écran IHM

Grâce au design dynamique de WhisperEye :
* **API REST (`/api/sensors` et `/api/peripherals`)** : Les nouvelles clés `i2c:*` sont automatiquement exposées au format JSON.
* **Écran IHM Physique (`src/screen_browse.rs`)** : La boucle d'affichage du menu **"Capteurs"** parcourt automatiquement le registre d'entrées `i2c:` enregistrées. Tout nouveau capteur ajouté dans `dynamic_devices.rs` apparaît immédiatement à l'écran sans aucun code supplémentaire !
