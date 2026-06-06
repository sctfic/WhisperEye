# 👁️ WhisperEye — Système de Partitionnement & Mise à Jour OTA (Rust / ESP32)

> **WhisperEye** est un écosystème de firmware modulaire écrit en **Rust** pour microcontrôleurs Espressif (**ESP32-S3**). Il s'appuie sur une séparation stricte entre une partition de secours stable (**Recovery**) et une partition applicative métier (**Production**). Ce modèle garantit une résilience industrielle avec mise à jour automatisée Over-The-Air (OTA) et un mécanisme anti-bootloop.

---

## 💾 Architecture de Partitionnement (`partitions.csv`)

Pour éviter tout risque de blocage ou d'inaccessibilité en production, la mémoire flash de 16 Mo de la carte WhisperEye est organisée selon un schéma de partitionnement spécifique défini dans le fichier [partitions.csv](file:///c:/Users/Alban/Desktop/Dev/www/WhisperEye/partitions.csv) :

```text
+---------------------------------------------------------------------------------+
|                                 ESP32-S3 Flash (16 Mo)                          |
+------------+---------------+--------------+----------------------+--------------+
| nvs (24K)  | otadata (8K)  | phy_init (4K)| recovery (2M)        | production   |
| Config/WiFi| Table de Boot | Phys RF Init | Secours (Sub:factory)| (13.8M) App  |
+------------+---------------+--------------+----------------------+--------------+
```

### Rôle détaillé des partitions :
* **`nvs` (24 Ko)** : Stockage non volatile. Elle contient les identifiants réseaux mémorisés dans le dictionnaire `wifiKnown`, le drapeau `autoUpdate` (activant/désactivant la mise à jour automatique), l'URL du dépôt de mise à jour `updateAvailable`, le compteur de tentatives de boot `otaRetry` (permettant de se prémunir des mises à jour infinies en boucle en cas de binaire défaillant), et la date cible du prochain contrôle `nextCheck`.
* **`otadata` (8 Ko)** : Coordonne la table de démarrage (boot slot active). L'ESP32 l'utilise pour décider de démarrer sur la partition de production ou de secours.
* **`phy_init` (4 Ko)** : Contient les données d'initialisation pour la couche physique radio (Wi-Fi et Bluetooth).
* **`recovery` (2 Mo)** : Contient le firmware de secours autonome `recovery_boot`.
  > [!IMPORTANT]
  > Bien que nommée **`recovery`** dans la configuration, son sous-type (subtype) dans la table reste défini sur `factory`. Cela force le bootloader de l'ESP32 à l'utiliser comme fallback automatique si aucune donnée d'OTA valide n'est initialisée ou si une défaillance de démarrage est détectée. Ce binaire de secours n'est jamais écrasé par OTA.
* **`production` (13.8 Mo)** : Dédiée à l'application métier principale `production_app`. Elle dispose de l'espace de stockage maximal pour héberger la logique des capteurs, l'ordonnanceur Cron, l'historique glissant des mesures et le dashboard web.

---

## 🔄 Principe de Fonctionnement (Production vs Recovery)

WhisperEye utilise un cycle de démarrage bidirectionnel sécurisé pour déployer de nouvelles versions de firmware sans intervention physique sur l'appareil.

```mermaid
graph TD
    A[Démarrage de la Carte] --> B{Bootloader ESP32}
    B --> C{otaRetry > 0 ?}
    C -- "Oui (MAJ en cours)" --> D[Boot sur RECOVERY]
    C -- "Non / -1 (Normal)" --> E[Boot sur PRODUCTION]

    D --> D1[Tentative de connexion Wi-Fi]
    D1 --> D2[Décrémente otaRetry en NVS]
    D2 --> D3[Télécharge et écrit le nouveau .bin sur Production]
    D3 --> D4{Flash OK ?}
    D4 -- Oui --> D5[Drapeau otaRetry = -1]
    D4 -- Non --> D6["Restart (nouvelle tentative)"]
    D5 --> D7[Restart]
    D7 --> B

    E --> E1[Exécution application métier]
    E1 --> E2[Cron: auto_update automatique]
    E2 --> E3{MAJ disponible ?}
    E3 -- Oui --> E4[Écrit l'URL en NVS et fixe otaRetry = 3]
    E4 --> E5[Restart de la carte]
    E5 --> B
```

### 1. Mode Normal (Production)
Par défaut, la carte démarre et s'exécute sur le firmware `production_app`. Elle récolte les données de capteurs, gère l'historique en mémoire, et expose un dashboard moderne de monitoring. Elle exécute aussi un cron en tâche de fond pour vérifier la présence de nouvelles versions sur le dépôt distant.

### 2. Mode Mise à Jour & Secours (Recovery)
Si une mise à jour est initiée (ou si la partition de production ne parvient plus à démarrer) :
1. La partition de production écrit l'URL du fichier binaire dans la NVS, configure la variable de tentatives `otaRetry` (généralement à `3`) et demande un redémarrage.
2. L'ESP32-S3 redémarre instantanément en partition de secours `recovery_boot`.
3. Dès son chargement, le firmware de secours se connecte au Wi-Fi. Avant de commencer tout téléchargement réseau, **il décrémente immédiatement de 1 le compteur `otaRetry` en NVS** pour se prémunir d'une coupure électrique ou d'un bug système durant l'installation.
4. Il télécharge le nouveau firmware applicatif et le flashe sur la partition `production`.
5. Si l'installation réussit, `otaRetry` est réinitialisé à `-1` et la carte redémarre sur son nouveau firmware de production.
6. Si l'installation échoue à plusieurs reprises (jusqu'à ce que `otaRetry` tombe à `0`), la carte reste indéfiniment en mode Recovery. L'utilisateur peut alors se connecter au portail captif généré par la partition Recovery pour diagnostiquer le système, uploader directement un fichier binaire depuis son navigateur, ou corriger la configuration.

---

## 🛠️ Architecture Logicielle (Workspace)

Le projet utilise un espace de travail Rust (Cargo Workspace) divisé de manière modulaire :

```text
WhisperEye/
├── Cargo.toml                  # Fichier de configuration du Workspace Cargo
├── partitions.csv              # Table des partitions de flashage (ESP32-S3)
├── run.ps1                     # Script de compilation & téléversement (PowerShell)
├── common/                     # [Crate commune] Partagée par Recovery et Production
│   └── src/
│       ├── lib.rs              # Modèles et clés NVS
│       └── nvs_storage.rs      # Wrapper d'accès en lecture/écriture à la NVS
├── recovery_boot/              # [Crate de secours] Firmware Recovery autonome (2 Mo)
│   └── src/
│       ├── main.rs             # Bootloader de secours, serveur HTTP & flashage OTA
│       ├── recovery.html       # Tableau de bord minimaliste de secours
│       ├── web_pages.rs        # Embarquement des ressources HTML
│       └── wifi.rs             # Gestion Wi-Fi et DNS Captif de secours
├── production_app/             # [Crate applicative] Firmware Production principal (13.8 Mo)
│   ├── Cargo.toml
│   ├── src/
│   │   ├── main.rs             # Dashboard, API REST locale & Initialisations
│   │   ├── production.html     # Tableau de bord dynamique moderne
│   │   ├── web_pages.rs        # Embarquement des ressources HTML de production
│   │   ├── wifi.rs             # Gestion Wi-Fi de production (Wi-Fi connus)
│   │   ├── cron.rs             # Tâches périodiques (Mises à jour, SNTP, Télémétrie)
│   │   ├── sensors.rs          # Routines de lecture des capteurs (I2C, 1-Wire, ADC)
│   │   └── actuators.rs        # Contrôle des sorties (Relais, PWM, LEDs)
│   └── schema.png              # Schéma électronique de principe
└── boards/                     # [Configurations de cartes cibles]
    ├── board_default/
    │   └── firmware.json       # Catalogue JSON décrivant les versions de firmware disponibles
    └── ota_base/
```

---

## 🔌 Carte Électronique WhisperEye

Le système tourne sur une carte matérielle propriétaire basée sur la puce **ESP32-S3**.

### Aperçus de la carte :

| Visuel Physique | Plan Mécanique |
|:---:|:---:|
| ![Visuel de la carte](file:///c:/Users/Alban/Desktop/Dev/www/WhisperEye/production_app/carte.png) | ![Plan mécanique](file:///c:/Users/Alban/Desktop/Dev/www/WhisperEye/production_app/plan.png) |

* **Schéma Électronique** :
  ![Schéma de principe électronique](file:///c:/Users/Alban/Desktop/Dev/www/WhisperEye/production_app/schema.png)
* **Brochage complet de la carte** (détail du fichier [S3-pin.tsv](file:///c:/Users/Alban/Desktop/Dev/www/WhisperEye/production_app/S3-pin.tsv)) :

| Signal / Label | GPIO (ESP32-S3) | Description |
|:---|:---:|:---|
| **ONEWR** | 39 | Bus 1-Wire (connecteurs 2x5) |
| **ISENSE** | 2 | Mesure de courant du pont en H |
| **SCR0** | 4 | Écran SLC (ST7789) |
| **SCR1** | 5 | Écran SDA (ST7789) |
| **SCR2** | 6 | Écran RES (ST7789) |
| **SCR3** | 7 | Écran DC (ST7789) |
| **SCR4** | 15 | Écran CS (ST7789) |
| **SCR5** | 16 | Écran BLK (ST7789) - Rétroéclairage |
| **BTN0** | 17 | Codeur rotatif - Roue sens A (EC11) |
| **BTN1** | 18 | Codeur rotatif - Roue sens B (EC11) |
| **BTN2** | 8 | Codeur rotatif - Bouton poussoir (EC11) |
| **BTN3** | 3 | Bouton de secours (Bouton KO) |
| **RSRO** | 41 | Liaison RS485 (SP3485EN) - RO (Receive Output) |
| **RSDE** | 42 | Liaison RS485 (SP3485EN) - DE (Driver Enable / RTS) |
| **RSDI** | 40 | Liaison RS485 (SP3485EN) - DI (Driver Input) |
| **SDA** | 38 | Bus I2C - SDA (Multiplexeur TCA9548A) |
| **SCL** | 37 | Bus I2C - SCL (Multiplexeur TCA9548A) |
| **INA** | 36 | Contrôle moteur - Entrée A du pont en H (DRV8701) |
| **INB** | 35 | Contrôle moteur - Entrée B du pont en H (DRV8701) |
| **RLA** | 48 | Relais de puissance A |
| **RLB** | 47 | Relais de puissance B |
| **VSENSE** | 1 | Mesure de la tension d'alimentation générale |
| **TOUCH** | 14 | Touche tactile capacitive périphérique |
| **SWPWR** | 21 | Sectionneur d'alimentation (coupure des rails 5V et 3.3V, sauf ESP) |
| **RF0** | 46 | Module radio (connecteur RF) |
| **RF1** | 9 | Module radio (connecteur RF) |
| **RF2** | 10 | Module radio (connecteur RF) |
| **RF3** | 11 | Module radio (connecteur RF) |
| **RF4** | 12 | Module radio (connecteur RF) |
| **RF5** | 13 | Module radio (connecteur RF) |


### Caractéristiques principales de la carte :
* **Microcontrôleur** : ESP32-S3 (Xtensa LX7 double cœur à 240 MHz).
* **Bus d'affichage (SPI)** : Support d'écran graphique haute résolution piloté par bus SPI rapide.
* **Bus Capteurs (I2C & 1-Wire)** :
  - Capteur de CO2, température et humidité **SCD41** et sonde haute précision **SHT45** multiplexés sur bus I2C via le circuit **TCA9548A**.
  - Sonde de température déportée **DS18B20** via bus 1-Wire.
  - Entrées tactiles (touches capacitives sensitives).
* **Communication industrielle** : Ligne série **RS485 half-duplex** avec contrôle matériel de direction (DE/RTS).
* **Alimentation & Actionneurs** :
  - Entrée d'alimentation de mesure (ADC) pour surveiller le niveau d'alimentation global.
  - Interrupteur/sectionneur d'alimentation général géré par pin GPIO pour les phases d'économie d'énergie.
  - Relais de puissance et sorties PWM moteur de précision.

---

## 🖥️ Interfaces Web (Dashboard)

Les firmware Production et Recovery intègrent tous deux des interfaces web embarquées très performantes écrites en HTML / CSS / Vanilla JS.

### Démonstration vidéo :
Voici un aperçu en vidéo du tableau de bord de production et des transitions de configuration de la carte :
![Démonstration Vidéo WhisperEye](file:///c:/Users/Alban/Desktop/Dev/www/WhisperEye/production_app/FwProd.mp4)

* **Interface Production (`production.html`)** : Dashboard de monitoring premium avec affichage dynamique des métriques (CO2, Température, Humidité), graphique d'historique glissant, état des relais et moteurs avec commande en temps réel, panneau de configuration réseau WiFi (SSID, mot de passe) et module d'installation de firmware OTA.
* **Interface Recovery (`recovery.html`)** : Interface simplifiée de secours. Elle permet d'uploader directement un binaire de production compilé localement en glisser-déposer (Drag & Drop), de configurer les réseaux connus, et de forcer manuellement le téléchargement d'un binaire depuis une URL cible.

---

## 🚀 Guide de Personnalisation pour Développeur

Pour adapter le firmware WhisperEye à votre propre projet ou carte électronique, suivez ces étapes :

### 1. Conserver Recovery et Personnaliser Production
Le firmware de secours `recovery_boot` est conçu pour rester générique et ne doit pas être modifié afin de préserver votre filet de sécurité (rollback). Vous devez implémenter vos fonctionnalités dans `production_app`.

### 2. Configurer le BoardType et le ChipType
Ouvrez le fichier `production_app/src/main.rs` et mettez à jour les constantes suivantes en haut du fichier (Elles devront correspondre au `firmware.json`. )
```rust
const WHISPEREYE_BOARD:  &str = "2.0";       // Version ou type de votre carte électronique boardType
const CHIP_TYPE:  &str = "ESP32-S3";         // Type du microcontrôleur cible (ex: ESP32-S3, ESP32-C6) ChipType
```

### 3. Configurer le Serveur de Mise à Jour OTA
Le firmware de production interroge régulièrement l'URL stockée dans la NVS sous la clé `updateAvailable`. Le fichier ciblé par cette URL doit respecter la structure JSON attendue.

Pour déployer vos firmwares, créez un fichier JSON similaire à [firmware.json](file:///c:/Users/Alban/Desktop/Dev/www/WhisperEye/boards/board_default/firmware.json) sur votre serveur d'hébergement :
```json
[
  {
    "boardType": "2.0",
    "ChipType": "ESP32-S3",
    "peripheriques": [],
    "stable": [
      {
        "version": "1.0.0",
        "url": "https://votre-domaine.com/assets/firmware-s3-1.0.0.bin"
      }
    ],
    "unstable": [
      {
        "version": "1.0.1-0002",
        "url": "https://votre-domaine.com/assets/firmware-s3-1.0.1-0002.bin"
      }
    ]
  }
]
```
Il s'agit d'un tableau qui peut contenir plusieurs `boardType` et `chipType` selon vos besoins.

### 4. Enregistrer la Clé dans la NVS
Au premier démarrage ou via l'interface réseau, assurez-vous d'inscrire l'URL de votre catalogue de mise à jour dans le paramètre `updateAvailable` de la NVS.

---

## 🛠️ Compilation et Téléversement

La chaîne de compilation Rust pour ESP32 nécessite des outils spécifiques d'Espressif.

### 1. Prérequis Système Généraux
1. Installez le compilateur Rust : [rustup](https://rustup.rs/).
2. Installez la toolchain ESP32 et les compilateurs croisés :
   ```bash
   cargo install ldproxy
   cargo install cargo-espflash
   ```
3. Installez `espup` pour gérer automatiquement les dépendances et compilateurs C++ Xtensa nécessaires à `esp-idf-sys` :
   ```bash
   # Installation globale de espup
   cargo install espup
   espup install
   ```

---

### 2. Déploiement sur Windows (Recommandé)

Sous Windows, le script d'automatisation [run.ps1](file:///c:/Users/Alban/Desktop/Dev/www/WhisperEye/run.ps1) gère l'export de l'environnement, l'incrémentation des versions, la compilation et le flashage.

1. Ouvrez une console **PowerShell** standard.
2. Lancez la compilation et le flashage d'un composant spécifique :

```powershell
# compiler et flasher l'intégralité du système (Recovery + Production)
.\run.ps1 -Target all

# compiler et flasher uniquement le firmware applicatif de Production
.\run.ps1 -Target production

# compiler et flasher uniquement le firmware de secours Recovery
.\run.ps1 -Target recovery

# effacer complètement la partition NVS de la carte
.\run.ps1 -Target nvs

# spécifier un port COM spécifique
.\run.ps1 -Target production -Port COM4
```

---

### 3. Déploiement sur Linux (Manuel / Ligne de Commande)

> [!NOTE]
> Les scripts de compilation automatique ne sont pas encore testés sous Linux. Vous pouvez compiler et flasher manuellement à l'aide des commandes standard de Cargo.

#### Prérequis Linux (Debian/Ubuntu) :
Installez les bibliothèques système nécessaires pour la communication USB série et la compilation croisée :
```bash
sudo apt-get update
sudo apt-get install -y libudev-dev pkg-config libusb-1.0-0-dev python3 python3-pip
```

#### Commandes de compilation :
```bash
# Charger les variables d'environnement espup (typiquement générées par espup install)
. $HOME/export-esp.sh

# Compiler le binaire Recovery (Secours)
cargo +esp build --package recovery_boot --release

# Compiler le binaire Production (Applicatif)
cargo +esp build --package production_app --release
```

#### Commandes de flashage manuel :
```bash
# Flasher le firmware de secours Recovery (sans rebooter après flash)
cargo +esp espflash flash --flash-size 16mb --package recovery_boot --partition-table partitions.csv --target-app-partition recovery --release --after no-reset

# Flasher le firmware de Production et ouvrir le moniteur série
cargo +esp espflash flash --flash-size 16mb --package production_app --partition-table partitions.csv --target-app-partition production --release --monitor
```

---

## 📜 Licence

Ce projet est la propriété de **Sctfic**. Tous droits réservés.
