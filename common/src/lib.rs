//! # WhisperEye Common Firmware Library
//!
//! Cette bibliothèque contient la base logicielle partagée pour l'ensemble des
//! déclinaisons de cartes WhisperEye à base d'ESP32.
//! Elle fournit les abstractions nécessaires pour :
//! - La connexion Wifi et Bluetooth (`wifi`)
//! - La synchronisation de l'heure via client NTP (`ntp`)
//! - Les mises à jour de firmware à distance (`ota`)
//! - Le serveur HTTP embarqué avec authentification TOTP (`http_server`)

pub mod wifi;
pub mod ntp;
pub mod ota;
pub mod http_server;

/// Configuration générale de base
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BaseConfig {
    pub wifi_ssid: String,
    pub wifi_psk: String,
    pub totp_secret: String,
    pub ntp_server: String,
}

impl Default for BaseConfig {
    fn default() -> Self {
        Self {
            wifi_ssid: String::from("IoT"),
            wifi_psk: String::from("Esp32&Cie2026"),
            totp_secret: String::from("Totp-Salt-4-Hash-Between-Probe-&-WhisperEye"), // Secret TOTP par défaut
            ntp_server: String::from("wrt.lan"),
        }
    }
}
