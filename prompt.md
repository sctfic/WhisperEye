## Objet

Développer un firmware de base minimaliste et ultra-fiable en **Rust** pour ESP32, localisé sur la partition `factory` (taille max : 2 Mo). Ce binaire gère une logique de démarrage double (Station avec mise à jour automatique / Point d'Accès avec portail captif) et sert de garde-fou pour provisionner l'appareil, écrire dans la NVS, flasher le firmware de production, et valider son état pour éviter le mécanisme de rollback automatique d'ESP-IDF.

---

## 1. Environnement technique & Contraintes

* **Langage & Framework :** Rust avec l'écosystème `esp-idf-hal` et `esp-idf-svc` (environnement `std`).
* **Optimisation de la taille :** Le binaire final doit impérativement tenir dans **1,5 Mo à 2 Mo**. Aucune partition de système de fichiers (SPIFFS / LittleFS) ne doit être utilisée. Les ressources web doivent être embarquées sous forme de chaînes statiques (`&'static str`).
* **Gestion mémoire :** Traitement des fichiers d'upload **par flux (streaming/chunks)** avec un tampon (buffer) de lecture maximal de 1 Ko à 4 Ko pour éviter toute saturation de la RAM.

---

## 2. Table des Partitions (`partitions.csv`)

```csv
# Name,   Type, SubType, Offset,  Size, Flags
nvs,      data, nvs,     ,        24K,
otadata,  data, ota,     ,        8K,
phy_init, data, phy,     ,        4K,
factory,  app,  factory, ,        2M,
production,app, ota_0,   ,        ,

```

---

## 3. Clés de Configuration NVS (Namespace: `"whispereye"`)

Le firmware doit s'assurer du cycle de vie des variables clés suivantes. Si la NVS est vierge, elle doit être initialisée avec ces valeurs d'usine par défaut :

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

## 4. Logique de Démarrage & Gestion Anti-Rollback (Workflow du `main.rs`)

Au boot, le firmware applique strictement l'algorithme suivant :

1. **Tentative de connexion STA :** Lecture de `wifi_ssid` et `wifi_psk` depuis la NVS. Si présents, l'ESP32 tente de s'y connecter en mode Station (STA) avec un timeout de 10 secondes.
2. **Scénario REUSSITE Connexion STA :**
* L'ESP32 reste connecté à ce réseau.
* Il initialise le client NTP via le serveur inscrit dans `ntp_server`.
* **Vérification OTA :** Si la clé `update_url` n'est pas vide, l'ESP32 lance immédiatement un client HTTP, télécharge le binaire de production en tâche de fond et met à jour la partition `production` par morceaux (streaming). Les variables `last_download` et `last_ota_success` sont mises à jour à la fin avec le timestamp récupéré via NTP.
* Le serveur HTTP reste démarré sur l'IP obtenue par DHCP pour permettre l'accès au formulaire.


3. **Scénario ECHEC Connexion STA :**
* L'ESP32 bascule immédiatement en **mode Access Point (AP)** ouvert, SSID : `"ESP32-Configuration"`.
* Un serveur DNS (UDP, Port 53) est démarré pour rediriger toutes les requêtes vers `192.168.4.1` (**Portail Captif**).
* Le serveur HTTP est démarré pour servir le formulaire de secours.



---

## 5. Interface Web & Interface Utilisateur (`src/web_pages.rs`)

La page HTML5/CSS unique doit embarquer la logique JavaScript nécessaire pour gérer les nouveaux champs dynamiques :

### Tab_1 : Status ESP32`
affiche le statut :
-ssid
- ip/gateway
- rssi
- server (ntp)
- Date NTP
- version Firmware
- DNS (oui ou non? mode captif?)
- ...

### Tab_2 : Configuration Système

* **Sélecteur SSID :** Un élément `<select id="ssid_select">`. La première option doit être `'Saisir un SSID'`. Les options suivantes sont peuplées par un `fetch` asynchrone pointant sur l'API de scan de l'ESP32.
* **Champ texte SSID masqué :** Un `<input type="text" id="ssid_custom" style="display:none;">`. Si l'utilisateur choisit l'option 'Saisir un SSID' dans le select, un script JS affiche ce champ texte pour permettre une saisie manuelle.
* **Mot de passe :** Un `<input type="password" id="password">` avec une case à cocher pour afficher/masquer les caractères en clair.
* **URL de mise à jour :** Un `<input type="text" id="update_url">` pré-rempli avec la valeur lue depuis la NVS.
* **Bouton Submit :** Envoie l'intégralité de ces champs au format JSON au point d'accès `POST /api/config`.

### Tab_3 : Upload Manuel Direct

* Un champ `<input type="file" accept=".bin">` pour flasher manuellement la partition de production depuis son PC, accompagné d'une barre de progression dynamique en JS (`XMLHttpRequest`).

---

## 6. Architecture des Fichiers et Spécifications des API REST

### `src/main.rs`

Gère le routage du serveur HTTP embarqué :

* `GET /` : Renvoie la page HTML de configuration (injecte au passage les valeurs actuelles de la NVS dans les inputs ou via une route dédiée `/api/values`).
* `GET /api/status` : Renvoie les status et les valeurs de la NVS.
* `GET /api/ssids` : Lance un scan réseau et renvoie la liste au format JSON.
* `POST /api/config` : Récupère le JSON, extrait le SSID (sélectionné ou saisi manuellement), la clé, et l'URL de mise à jour, teste le SSID et la clé (connexion), puis enregistre ces variables dans la NVS, si la connexion reussi, il lance la MAJ par telechargement.
* `POST /api/upload-ota` : Reçoit le flux binaire brut par paquets de taille contrôlée, écrit dans la partition `production` à l'aide de l'API `EspOta`.

### `src/wifi.rs`

Contient la logique d'initialisation du pilote Wi-Fi, la gestion de la bascule d'état (STA vers AP), et la routine de traitement des requêtes UDP 53 pour le serveur DNS captif.

### `src/ota.rs`

Fournit l'implémentation client HTTP pour le téléchargement automatique depuis `update_url` et le streaming vers la flash.


---

## 7. Livrables attendus

1. Le fichier `Cargo.toml` complet avec les drapeaux d'optimisation de taille pour la release (`lto = true`, `opt-level = "s"`, `panic = "abort"`).
2. Le fichier `partitions.csv`.
3. Un code source Rust modulaire, propre et intégralement écrit (`main.rs`, `wifi.rs`, `web_pages.rs`, `ota.rs`). Toutes les dépendances NVS, les manipulations de structures d'options Wi-Fi doivent compiler de manière native sous l'écosystème `esp-idf-sys`. Aucun squelette vide n'est accepté.