# Documentation API — Mode Production (production_app)

Ce document détaille l'ensemble des points d'accès (endpoints) HTTP exposés par le firmware de **Production** applicatif (`production_app`) de la carte WhisperEye.

---

## 🌐 Ressources Statiques et Portail Captif

### Tableau de Bord Principal
* **URL** : `/`
* **Méthode** : `GET`
* **Description** : Renvoie l'interface web riche de monitoring et de configuration applicative (`production.html`).

### Icône Favicon
* **URL** : `/favicon.ico`
* **Méthode** : `GET`
* **Description** : Renvoie le favicon de l'application.

### Portail Captif (Redirections)
* **URLs** : `/generate_204`, `/hotspot-detect.html`, `/ncsi.txt`, `/connecttest.txt`
* **Méthode** : `GET`
* **Description** : Redirige la détection des OS mobiles vers `http://192.168.71.1/` (HTTP 302 Found) pour forcer l'affichage du portail en cas de connexion en direct à l'AP Mesh locale.

---

## 📊 Endpoints API (Lecture / GET)

### État Complet du Système
* **URL** : `/api/status`
* **Méthode** : `GET`
* **Type de retour** : `application/json`
* **Description** : Retourne l'état complet de la carte (mode réseau, Wi-Fi, IP, configurations avancées, et état détaillé du Mesh : mode, distance, nœuds connectés).
* **Exemple de réponse** :
```json
{
  "network_mode": "Station",
  "wifi_ssid": "Maison_WiFi",
  "wifi_rssi": -58,
  "ip_addr": "192.168.1.120",
  "gateway_addr": "192.168.1.254",
  "sys_time": "2026-06-14 11:15:32",
  "ntp_server": "pool.ntp.org",
  "metrics_url": "http://192.168.1.50/telemetry",
  "fw_version": "1.0.19-0013",
  "last_ota_success": "2026-06-14 10:05:00",
  "last_ota_dl": "2026-06-14 10:04:12",
  "last_ota_write": "2026-06-14 10:04:55",
  "update_url": "https://example.com/manifest.json",
  "update_interval": "7j",
  "whispereye_board": "1.0",
  "board_type": "1.0",
  "chip_type": "ESP32-S3",
  "wifi_known": {
    "Maison_WiFi": { "psk": "MaCleWiFi123", "default": true }
  },
  "auto_update": true,
  "rename_enabled": true,
  "has_totp": true,
  "partial_totp": "123456......789012",
  "ext_name": "WE-F3C2",
  "ext_desc": "WhisperEye Extender Salon",
  "mesh_enabled": true,
  "mesh_root": true,
  "mesh_distance": 0,
  "mesh_nodes_count": 2,
  "mesh_channel": 11,
  "mesh_id": "WhisperMesh",
  "mesh_pmk": "",
  "author": {
    "email": "alban.lopez+whisperEye@gmail.com",
    "name": "LOPEZ Alban",
    "link": "https://github.com/sctfic/WhisperEye/blob/main/README.md"
  }
}
```

### Capacités Matérielles (Périphériques)
* **URL** : `/api/capacity`
* **Méthode** : `GET`
* **Type de retour** : `application/json`
* **Description** : Renvoie les listes des capteurs et actionneurs physiquement présents sur la carte électronique, avec leurs types et unités de mesure.
* **Exemple de réponse** :
```json
{
  "mac": "24:EC:4A:81:F3:C2",
  "name": "WE-F3C2",
  "description": "WhisperEye Extender Salon",
  "version": "1.0.19-0013",
  "rename_enabled": true,
  "sensors": [
    { "Name": "touch", "description": "Touche Tactile", "Type": "Touch", "Unit": "-" },
    { "Name": "onewr:28ff641e8315029c", "description": "Sonde DS18B20", "Type": "Temperature", "Unit": "°C" }
  ],
  "actuators": [
    { "Name": "rla", "description": "Relais de puissance A", "Type": "tout ou rien", "range": "bool:0 1" }
  ]
}
```

### Proxy de Recherche des Mises à Jour
* **URL** : `/api/check_updates`
* **Méthode** : `GET`
* **Type de retour** : `application/json`
* **Description** : Identique à celle du mode Recovery. Proxy de secours local pour charger le manifest JSON sans blocage CORS.

### Historique des Mesures
* **URL** : `/api/history`
* **Méthode** : `GET`
* **Type de retour** : `application/json`
* **Description** : Renvoie les 10 dernières entrées de mesures capteurs enregistrées en mémoire glissante (SHT45, SCD41, sondes 1-Wire).
* **Exemple de réponse** :
```json
[
  {
    "timestamp": 1781426450,
    "readings": {
      "temperature_sht45": 22.4,
      "humidity_sht45": 48.2,
      "co2_scd41": 650,
      "ds18b20_temperatures": {
        "28ff641e8315029c": 21.8
      }
    }
  }
]
```

### Scan des réseaux Wi-Fi
* **URL** : `/api/ssids`
* **Méthode** : `GET`
* **Type de retour** : `application/json`
* **Description** : Déclenche un scan matériel et met en cache la liste des réseaux Wi-Fi environnants détectés (identique à Recovery).

### Mesures en Temps Réel (Live)
* **URL** : `/api/sensors`
* **Méthode** : `GET`
* **Type de retour** : `application/json`
* **Description** : Interroge directement les capteurs connectés et retourne les dernières valeurs physiques lues.
* **Exemple de réponse** :
```json
{
  "temperature_sht45": 22.5,
  "humidity_sht45": 48.0,
  "co2_scd41": 645,
  "ds18b20_temperatures": {
    "28ff641e8315029c": 21.9
  }
}
```

### Liste des Périphériques Configurés
* **URL** : `/api/peripherals`
* **Méthode** : `GET`
* **Type de retour** : `application/json`
* **Description** : Liste l'ensemble des modules physiques gérés (statiques ou dynamiques découverts), leur nom d'affichage personnalisé, leur présence physique et leur valeur courante.
* **Exemple de réponse** :
```json
[
  { "id": "rla", "name": "Relais A", "is_static": true, "present": true, "value": "OFF" },
  { "id": "onewr:28ff641e8315029c", "name": "Sonde DS18B20", "is_static": false, "present": true, "value": "21.9" }
]
```

### Synchronisation du Mesh Parent/Enfant
* **URL** : `/api/mesh/sync`
* **Méthode** : `GET`
* **Description** : Endpoint appelé par les nœuds enfants pour s'enregistrer auprès de leur parent Mesh, récupérer les identifiants de connexion par défaut et calculer leur propre distance au nœud racine.

---

## 🛠️ Endpoints API (Configuration & Mutation / POST)

### Renommer un Périphérique
* **URL** : `/api/peripherals`
* **Méthode** : `POST`
* **Format Payload** : `application/json`
* **Description** : Modifie le nom d'affichage personnalisé d'un capteur ou actionneur en NVS (ex: renommer `rla` en "Radiateur").
* **Exemple de payload** :
```json
{
  "id": "rla",
  "name": "Chauffage Salon"
}
```

### Contrôle des Actionneurs (Relais / Alimentations)
* **URL** : `/api/actuators`
* **Méthode** : `POST`
* **Format Payload** : `application/json`
* **Description** : Active ou désactive les sorties physiques de la carte (relais RLA/RLB, alimentation SWPWR, pont en H INA/INB).
* **Exemple de payload** :
```json
{
  "rla": true,
  "rlb": false,
  "swpwr": true,
  "ina": false,
  "inb": false
}
```

### Supprimer la clé TOTP de sécurité
* **URL** : `/api/clear-totp`
* **Méthode** : `POST`
* **Format Payload** : `application/json`
* **Description** : Supprime la clé TOTP configurée en NVS. Nécessite l'envoi de la clé secrète actuelle brute sous le champ `token` pour s'authentifier.
* **Exemple de payload** :
```json
{
  "token": "MaCléSecrèteActuelle"
}
```

### Réinitialisation d'Usine (Factory Reset)
* **URL** : `/api/reset`
* **Méthode** : `POST`
* **Format Payload** : `application/json`
* **Description** : Efface l'ensemble de la configuration stockée en NVS (Réseaux Wi-Fi, clés TOTP, paramètres Mesh) et redémarre la carte. Nécessite la confirmation explicite du mot `"RESET"`.
* **Exemple de payload** :
```json
{
  "confirm": "RESET"
}
```

### Enregistrement de la Configuration Système
* **URL** : `/api/config`
* **Méthode** : `POST`
* **Format Payload** : `application/json`
* **Description** : Modifie et enregistre les variables globales de l'appareil (Wi-Fi client, URL des mises à jour, URL de télémétrie, paramètres Mesh, nom et description de l'extendeur, activation du renommage).
* **Exemple de payload complet** :
```json
{
  "wifi_ssid": "MonRéseauWiFi",
  "wifi_psk": "MotDePasseWi-Fi",
  "update_url": "https://example.com/catalog.json",
  "update_interval": "7j",
  "apply_only": false,
  "auto_update": true,
  "totp_secret": "MaNouvelleCléSecrète",
  "ext_name": "NouveauNomWE",
  "ext_desc": "NouvelleDescription",
  "ntp_server": "pool.ntp.org",
  "metrics_url": "http://192.168.1.50/telemetry",
  "rename_enabled": true,
  "mesh_enabled": true,
  "mesh_channel": 11,
  "mesh_id": "WhisperMesh",
  "mesh_pmk": ""
}
```
*Note : Tous les champs du payload JSON sont optionnels.*
