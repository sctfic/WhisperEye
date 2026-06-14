# Documentation API — Mode Recovery (recovery_boot)

Ce document détaille l'ensemble des points d'accès (endpoints) HTTP exposés par le firmware de secours **Recovery** (`recovery_boot`) de la carte WhisperEye. 

L'IP par défaut en mode Point d'Accès est **`192.168.71.1`**.

---

## 🌐 Pages et Portail Captif

### Page d'accueil de Secours
* **URL** : `/`
* **Méthode** : `GET`
* **Description** : Renvoie le tableau de bord minimaliste embarqué de secours (`recovery.html`).

### Portail Captif (Redirections)
* **URLs** : `/generate_204`, `/hotspot-detect.html`, `/ncsi.txt`, `/connecttest.txt`, `/*` (catch-all)
* **Méthode** : `GET`
* **Description** : Redirige les requêtes de détection de connectivité des OS (Android, iOS, Windows) vers `http://192.168.71.1/` (HTTP 302 Found) pour faire apparaître automatiquement la page de secours.

---

## 📊 Endpoints API (Lecture / GET)

### État du Système
* **URL** : `/api/status`
* **Méthode** : `GET`
* **Type de retour** : `application/json`
* **Description** : Renvoie les informations d'état système spécifiques à la partition Recovery.
* **Exemple de réponse** :
```json
{
  "network_mode": "AccessPoint",
  "wifi_ssid": "ESP32-Configuration",
  "wifi_rssi": null,
  "ip_addr": "192.168.71.1",
  "gateway_addr": "0.0.0.0",
  "sys_time": "1970-01-01 00:00:23",
  "ntp_server": "pool.ntp.org",
  "metrics_url": "",
  "fw_version": "v1.0.0-poc",
  "last_ota_success": "Jamais",
  "last_ota_dl": "Jamais",
  "last_ota_write": "Jamais",
  "update_url": "https://raw.githubusercontent.com/.../firmware.json",
  "update_interval": "7j",
  "board_type": "1.0",
  "chip_type": "ESP32-S3",
  "recovery_version": "1.0.0-recovery-0061",
  "wifi_known": {},
  "auto_update": true
}
```

### État d'avancement de la Mise à Jour (OTA)
* **URL** : `/api/updateStatus`
* **Méthode** : `GET`
* **Type de retour** : `application/json`
* **Description** : Retourne l'état actuel et la progression de l'écriture en mémoire flash du firmware de production. Utilisé par le frontend en mode *polling* (toutes les 500 ms).
* **Exemple de réponse** :
```json
{
  "percentage": 45,
  "size": 1450201,
  "written": 652590,
  "status": "Téléchargement et flashage de la production..."
}
```

### Proxy de Recherche des Mises à Jour
* **URL** : `/api/check_updates`
* **Méthode** : `GET`
* **Type de retour** : `application/json`
* **Description** : Lit l'URL du catalogue enregistrée en NVS (`updateAvailable`), télécharge le fichier de configuration et extrait l'entrée correspondante au type de puce `ESP32-S3`. Sert de proxy local pour contourner les blocages de CORS.

### Scan des réseaux Wi-Fi
* **URL** : `/api/ssids`
* **Méthode** : `GET`
* **Type de retour** : `application/json`
* **Description** : Déclenche un scan matériel actif des réseaux Wi-Fi environnants et retourne la liste des SSID détectés ainsi que le SSID actif actuel. En mode AP, si le scan échoue, il renvoie une liste par défaut.
* **Exemple de réponse** :
```json
{
  "ssids": ["Freebox-WiFi", "Maison-2.4G", "IoT-Net"],
  "active": ""
}
```

---

## 🛠️ Endpoints API (Configuration & Mutation / POST)

### Enregistrement de la Configuration
* **URL** : `/api/config`
* **Méthode** : `POST`
* **Format Payload** : `application/json`
* **Description** : Enregistre les nouveaux paramètres Wi-Fi et OTA dans la NVS et lance optionnellement la procédure d'OTA en tâche de fond (si `apply_only` est `false`).
* **Format JSON du payload** :
```json
{
  "wifi_ssid": "MonNouveauRéseau",
  "wifi_psk": "MaCléSecrète",
  "update_url": "https://example.com/production.bin",
  "apply_only": false,
  "auto_update": true,
  "ntp_server": "pool.ntp.org",
  "metrics_url": "http://192.168.1.50/api/telemetry"
}
```

### Flashage manuel (Upload de Fichier)
* **URL** : `/api/upload-ota`
* **Méthode** : `POST`
* **Format Payload** : `application/octet-stream`
* **Description** : Permet de téléverser et flasher directement un fichier binaire (`.bin`) de production envoyé par morceaux (chuncks) depuis le navigateur. Une fois le flashage validé, la partition de démarrage est orientée vers `production` et la carte redémarre automatiquement.
