## Objet

Développer un firmware de base minimaliste, ultra-fiable et modulaire en **Rust** pour microcontrôleurs ESP32 (cible initiale : ESP-WROOM-32 pour le POC, avec compatibilité ascendante pensée pour ESP32-S3 et ESP32-C6). 

Ce binaire est localisé sur la partition `factory` (taille max : 2 Mo). Il gère une logique de démarrage double (Station avec mise à jour automatique / Point d'Accès avec portail captif) et sert de garde-fou (bootstrapper) pour provisionner l'appareil, écrire dans la NVS, flasher le firmware de production.
L'architecture logicielle doit être pensée pour découpler le socle de communication (ce firmware de base `factory`) de la logique applicative finale (`production`), cette dernière ayant vocation à intégrer à terme des pilotes spécifiques (Écran SPI, RS485, Radio, Capteurs I2C/1-Wire, Actionneurs).

---

## 1. Environnement technique & Contraintes

* **Langage & Framework :** Rust avec l'écosystème `esp-idf-hal` et `esp-idf-svc` (environnement `std`). Target de référence pour le POC : `xtensa-esp32-espidf`.
* **Optimisation de la taille :** Le binaire final doit impérativement tenir dans **2 Mo max**. Aucune partition de système de fichiers (SPIFFS / LittleFS) ne doit être utilisée. Les ressources web doivent être embarquées sous forme de chaînes statiques (`&'static str`).
* **Gestion mémoire :** Traitement des fichiers d'upload et de téléchargement **par flux (streaming/chunks)** avec un tampon (buffer) de lecture maximal de 1 Ko à 4 Ko pour éviter toute saturation de la RAM (Heap allocation minimale).
* **Architecture Multi-Cibles & Modulaire :** Structurer le projet sous forme d'un espace de travail (**Cargo Workspace**) ou de modules découplés afin de pouvoir compiler :
  1. Le firmware de base (`factory`).
  2. Une version POC du firmware de `production` (qui partage l'infrastructure NVS/Wi-Fi/web server, mais possède son propre point d'entrée prêt à recevoir les futures tâches d'acquisition capteurs et de contrôle actionneurs).

---

## 2. Table des Partitions (`partitions.csv`)

```csv
# Name,   Type, SubType, Offset,  Size, Flags
nvs,      data, nvs,     ,        24K,
otadata,  data, ota,     ,        8K,
phy_init, data, phy,     ,        4K,
factory,  app,  factory, ,        2M,
production,app, ota_0,   ,        2M,

```

---

## 3. Clés de Configuration NVS (Namespace: `"whispereye"`)

Le firmware doit s'assurer du cycle de vie des variables clés suivantes. Si la NVS est vierge ou corrompue, elle doit être initialisée avec ces valeurs d'usine par défaut :

* `wifi_ssid` (String) : `"IoT"`
* `wifi_psk` (String) : `"Esp32&Cie2026"`
* `totp_secret` (String) : `"Salt-4-Hash-Between-Probe-&-WhisperEye"`
* `ntp_server` (String) : `"wrt.lan"`
* `fw_version` (String) : `"empty"`
* `last_download` (String) : `"1970-01-01T00:00:00Z"`
* `last_ota_success` (String) : `"1970-01-01T00:00:00Z"`
* `update_url` (String) : `"https://github.com/sctfic/WhisperEye/raw/main/boards/board_default/firmware.bin"`
* `ota_retry` (i32) : `-1`

---

## 4. Logique de Démarrage (Workflow du `main.rs`)

Au boot, le firmware applique strictement l'algorithme suivant :

1. **Tentative de connexion STA :** Lecture de `wifi_ssid` et `wifi_psk` depuis la NVS. L'ESP32 tente de s'y connecter en mode Station (STA) avec un timeout strict de 10 secondes.
2. **Scénario REUSSITE Connexion STA :**
* L'ESP32 reste connecté à ce réseau.
* Il initialise le client NTP via le serveur inscrit dans `ntp_server`.
* **Vérification & Exécution OTA :** Si la clé `update_url` n'est pas vide, l'ESP32 lance immédiatement un client HTTP, télécharge le binaire de production en tâche de fond et met à jour la partition `production` par morceaux (streaming). La variable `last_download` est mise à jour en NVS à la fin avec le timestamp récupéré via NTP, `last_ota_success` et `fw_version` sont réinitialisées à "empty" et "1970-01-01T00:00:00Z", elles seront mises à jour par le firmware de production après un premier reboot réussi.
* Le serveur HTTP reste démarré sur l'IP obtenue par DHCP pour permettre l'accès à l'interface de configuration.


3. **Scénario ECHEC Connexion STA :**
* L'ESP32 bascule immédiatement en **mode Access Point (AP)** ouvert, SSID : `"ESP32-Configuration"`.
* Un serveur DNS (UDP, Port 53) est démarré pour rediriger toutes les requêtes DNS vers `192.168.4.1` (**Portail Captif**).
* Le serveur HTTP est démarré pour servir le formulaire de secours.


---

## 5. Interface Web & Interface Utilisateur (`src/web_pages.rs`)

La page HTML5/CSS unique doit être moderne, responsive, et embarquer la logique JavaScript (Vanilla JS) nécessaire pour gérer l'affichage par onglets (Tabs) et les requêtes asynchrones.

### A. Interface du Firmware de Base (`factory_boot`)
* **Tab_1 : Statut ESP32**
  Affiche dynamiquement (via un fetch périodique sur `/api/status`) l'état du système :
  * SSID actuel et force du signal (RSSI)
  * Adresse IP, Passerelle (Gateway)
  * Serveur NTP configuré, Date/Heure courante de l'ESP32 et décalage NTP
  * Version actuelle du firmware de production et date de la dernière mise à jour
  * Mode réseau actif (Station ou Portail Captif DNS Actif)
* **Tab_2 : Configuration Système**
  * **Sélecteur SSID :** Un élément `<select id="ssid_select">`. La première option doit être `'Saisir un SSID'`. Les options suivantes sont peuplées par un `fetch` asynchrone pointant sur l'API de scan `/api/ssids`.
  * **Champ texte SSID masqué :** Un `<input type="text" id="ssid_custom" style="display:none;">`, affiché en JS uniquement si l'utilisateur choisit l'option 'Saisir un SSID'.
  * **Mot de passe :** Un `<input type="password" id="password">` avec une case à cocher pour basculer la visibilité du texte.
  * **URL de mise à jour :** Un `<input type="text" id="update_url">` pré-rempli.
  * **Bouton de validation :** Envoie l'intégralité de ces champs au format JSON via un `POST /api/config`.
* **Tab_3 : Upload Manuel Direct**
  * Un champ `<input type="file" accept=".bin">` pour flasher manuellement le binaire sur la partition de production depuis son PC, accompagné d'une barre de progression HTML5 alimentée par un objet `XMLHttpRequest` (gestion de l'avancement de l'upload).

### B. Interface du Firmware Applicatif (`production_app`)
Le firmware de production intègre **la même gestion Wi-Fi / DNS / Portail captif** et le même style moderne et premium, mais avec les spécificités suivantes :
* **PAS de formulaire d'upload manuel (Pas de Tab_3).** La mise à jour se fait exclusivement via le serveur d'OTA automatique.
* **Tab_2 (Configuration) modifiée :** Le changement d'URL de mise à jour (ou des identifiants Wi-Fi) est enregistré en NVS et **déclenche immédiatement un redémarrage de l'ESP32** (restart). Au boot suivant, le firmware de base (`factory_boot`) prendra le relais pour tenter la connexion et exécuter la mise à jour OTA ou valider la configuration.
* **Tab_3 : Capteurs & Actionneurs (Nouvel Onglet)**
  * Visualisation en temps réel des données des capteurs (ex. Température SHT45, humidité, CO2 SCD41, capteurs 1-Wire DS18B20)
  * Contrôle interactif des actionneurs (ex. boutons de bascule pour les relais, curseurs ou valeurs numériques pour la PWM / consignes).

---

## 6. Structure du Projet, Architecture Répertoire et Spécifications des API

Afin de prévoir les déclinaisons matérielles (S3, C6) et l'intégration des capteurs/actionneurs dans la version finale sans polluer le code de secours (`factory`), le projet adopte l'architecture monorepo suivante pour le POC :

```text
WhisperEye/
├── Cargo.toml                  # Workspace Cargo config
├── partitions.csv              # Table des partitions commune
├── common/                     # Bibliothèque partagée (NVS, HAL partagée, Modèles de données)
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       └── nvs_storage.rs      # Gestion unifiée de la NVS
├── factory_boot/               # Notre Firmware de secours minimaliste (2 Mo)
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs             # Routage HTTP, orchestrateur de boot
│       ├── wifi.rs             # Pilote Wi-Fi AP/STA et Serveur DNS Captif (Port 53)
│       ├── ota.rs              # Client HTTP d'auto-update (Streaming client -> Flash)
│       └── web_pages.rs        # UI HTML/CSS/JS injectée sous forme de chaînes statiques
└── production_app/             # Le Firmware Applicatif de Production (Avec gestion capteurs & actionneurs)
    ├── Cargo.toml
    └── src/
        ├── main.rs             # Point d'entrée de production, initialisation du HAL, boucle d'acquisition
        ├── wifi.rs             # Même pile Wi-Fi / DNS / Portail Captif partagée que factory
        ├── sensors.rs          # Simulation/Gestion des capteurs (SCD41/SHT45/DS18B20)
        ├── actuators.rs        # Gestion des sorties (Relais, PWM)
        └── web_pages.rs        # UI dédiée (Statut, Config avec reboot, Onglet Capteurs/Actionneurs)

```

### Spécifications des API REST

#### 1. API communes et implémentées dans `factory_boot`
* `GET /` : Renvoie la page HTML d'usine unique issue de `web_pages.rs`.
* `GET /api/status` : Renvoie un objet JSON contenant les statuts d'exécution et les valeurs courantes NVS.
* `GET /api/ssids` : Déclenche un scan asynchrone des réseaux Wi-Fi et renvoie la liste des SSID trouvés.
* `POST /api/config` : Récupère la charge utile JSON (SSID, mot de passe, URL). Enregistre dans la NVS et lance immédiatement la procédure OTA.
* `POST /api/upload-ota` : Reçoit le flux binaire brut (`application/octet-stream`) pour écrire directement dans la partition `production`.

#### 2. API spécifiques à `production_app`
* `GET /` : Renvoie la page HTML de production unique (avec l'onglet Capteurs/Actionneurs et sans l'onglet d'upload manuel).
* `GET /api/status` : Similaire à factory, mais remonte également la version courante active de production.
* `POST /api/config` : Enregistre les nouveaux paramètres Wi-Fi et l'URL en NVS, puis **déclenche un redémarrage immédiat de l'ESP32** (via `esp_restart()`) pour repasser par le chargeur du firmware de base si un url de maj est fourni et different de l'url actuel, sinon il ne redémarre pas.
* `GET /api/sensors` : Renvoie un objet JSON contenant les lectures en temps réel des capteurs :
  ```json
  {
    "temperature_sht45": 23.5,
    "humidity_sht45": 45.2,
    "co2_scd41": 850,
    "temperature_ds18b20": 22.8
  }
  ```
* `POST /api/actuators` : Reçoit une charge utile JSON pour piloter les actionneurs et renvoie l'état mis à jour :
  ```json
  {
    "relay_1": true,
    "pwm_intensity": 75
  }
  ```

---

## 7. Livrables Attendus pour le POC (Cible ESP-WROOM-32)

1. **`Cargo.toml` (Racine & Workspace) :** Configuré avec les drapeaux agressifs d'optimisation de taille pour le profil `release` (`lto = true`, `opt-level = "s"`, `panic = "abort"`, `codegen-units = 1`).
2. **`partitions.csv`** conforme aux tailles spécifiées.
3. **Code Source Rust Intégral :** L'ensemble des fichiers décrits dans l'arborescence (`common`, `factory_boot`, `production_app`). Le code doit compiler nativement avec la chaîne d'outils ESP-IDF (`esp-idf-sys`, `esp-idf-hal`, `esp-idf-svc`). Le code doit être exempt de placeholders vides ou de macro `todo!()` bloquantes.

```