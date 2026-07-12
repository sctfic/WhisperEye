use esp_idf_svc::nvs::{EspNvs, EspNvsPartition, NvsDefault};
use anyhow::{Result, Context};
use log::{info};

pub struct NvsStorage {
    nvs: EspNvs<NvsDefault>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct WifiKnownEntry {
    pub psk: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<bool>,
    /// Timestamp de dernière connexion réussie (unix_secs / 600, intervalles de 10 min)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<u32>,
}

impl NvsStorage {
    pub fn new(partition: EspNvsPartition<NvsDefault>) -> Result<Self> {
        let nvs = EspNvs::new(partition, "whispereye", true)
            .context("Failed to open NVS namespace 'whispereye'")?;
        let mut storage = Self { nvs };
        storage.ensure_defaults()?;
        Ok(storage)
    }

    pub fn ensure_defaults(&mut self) -> Result<()> {
        if self.get_str("wifiKnown")?.is_none() {
            self.set_str("wifiKnown", r#"{"IoT":{"psk":"Esp32&Cie2026","default":true}}"#)?;
        }
        if self.get_str("ntpServer")?.is_none() {
            self.set_str("ntpServer", "default")?;
        }
        if self.get_str("metricsUrl")?.is_none() {
            self.set_str("metricsUrl", "empty")?;
        }
        if self.get_str("fwVersion")?.is_none() {
            self.set_str("fwVersion", "empty")?;
        }
        if self.get_str("lastOtaDl")?.is_none() {
            self.set_str("lastOtaDl", "1970-01-01T00:00:00Z")?;
        }
        if self.get_str("lastOtaSuccess")?.is_none() {
            self.set_str("lastOtaSuccess", "1970-01-01T00:00:00Z")?;
        }
        if self.get_str("lastOtaWrite")?.is_none() {
            self.set_str("lastOtaWrite", "1970-01-01T00:00:00Z")?;
        }
        if self.get_str("updateRepoList")?.is_none() {
            // self.set_str("updateRepoList", "https://raw.githubusercontent.com/sctfic/WhisperEye/refs/heads/mesh/boards/board_default/firmware-s3.json")?;
            self.set_str("updateRepoList", "https://github.com/sctfic/WhisperEye/raw/main/boards/board_default/firmware-s3.json")?;
        }
        if self.get_str("updateDlUrl")?.is_none() {
            self.set_str("updateDlUrl", "empty")?;
        }
        if self.get_i32("otaRetry")?.is_none() {
            self.set_i32("otaRetry", -1)?;
        }
        if self.get_i32("autoUpdate")?.is_none() {
            self.set_i32("autoUpdate", 1)?;
        }
        if self.get_str("nextCheck")?.is_none() {
            self.set_str("nextCheck", "0")?;
        }
        if self.get_i32("deviceRenamable")?.is_none() {
            self.set_i32("deviceRenamable", 1)?;
        }

        if self.get_i32("wifiChannel")?.is_none() {
            self.set_i32("wifiChannel", 11)?;
        }
        // if self.get_str("meshId")?.is_none() {
        //     self.set_str("meshId", "WE-001")?;
        // }
        // if self.get_str("meshPmk")?.unwrap_or_default().is_empty() {
        //     self.set_str("meshPmk", "Mesh-IoT@Espressif!")?;
        // }
        // if self.get_str("meshSsid")?.unwrap_or_default().is_empty() {
        //     self.set_str("meshSsid", "Esp32MeshNetwork")?;
        // }
        Ok(())
    }

    pub fn get_known_networks(&self) -> Result<std::collections::HashMap<String, WifiKnownEntry>> {
        let known_str = self.get_str("wifiKnown")?.unwrap_or_else(|| "{}".to_string());
        let map: std::collections::HashMap<String, WifiKnownEntry> = serde_json::from_str(&known_str).unwrap_or_default();
        Ok(map)
    }

    pub fn save_known_networks(&mut self, map: &std::collections::HashMap<String, WifiKnownEntry>) -> Result<()> {
        let new_str = serde_json::to_string(map)?;
        self.set_str("wifiKnown", &new_str)?;
        Ok(())
    }

    pub fn set_default_network(&mut self, ssid: &str, psk: &str) -> Result<()> {
        if ssid.is_empty() {
            return Ok(());
        }
        let mut map = self.get_known_networks()?;
        for entry in map.values_mut() {
            entry.default = None;
        }
        let existing_last_seen = map.get(ssid).and_then(|e| e.last_seen);
        map.insert(ssid.to_string(), WifiKnownEntry {
            psk: psk.to_string(),
            default: Some(true),
            last_seen: existing_last_seen,
        });
        self.save_known_networks(&map)?;
        Ok(())
    }

    pub fn set_default_network_by_ssid(&mut self, ssid: &str) -> Result<()> {
        let mut map = self.get_known_networks()?;
        for (s, entry) in map.iter_mut() {
            if s == ssid {
                entry.default = Some(true);
            } else {
                entry.default = None;
            }
        }
        self.save_known_networks(&map)?;
        Ok(())
    }

    /// Enregistre le timestamp de dernière connexion réussie pour un SSID.
    /// Format : unix_secs / 600 (intervalles de 10 minutes).
    /// Ne met à jour que si le nouveau timestamp est strictement supérieur à l'ancien.
    pub fn update_wifi_last_seen(&mut self, ssid: &str) -> Result<()> {
        if ssid.is_empty() {
            return Ok(());
        }
        let now_10min = (std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            / 600) as u32;
        let mut map = self.get_known_networks()?;
        if let Some(entry) = map.get_mut(ssid) {
            if entry.last_seen.unwrap_or(0) < now_10min {
                entry.last_seen = Some(now_10min);
                self.save_known_networks(&map)?;
            }
        }
        Ok(())
    }

    /// Retourne le last_seen du réseau Wi-Fi par défaut, s'il existe.
    pub fn get_default_network_last_seen(&self) -> Result<Option<u32>> {
        let known = self.get_known_networks()?;
        Ok(known.iter()
            .find(|(_, e)| e.default.unwrap_or(false))
            .and_then(|(_, e)| e.last_seen))
    }

    pub fn get_str(&self, key: &str) -> Result<Option<String>> {
        // EspNvs get_str writes to a buffer. We'll use a larger dynamic buffer to support JSON arrays.
        let mut buf = vec![0u8; 4000];
        match self.nvs.get_str(key, &mut buf) {
            Ok(Some(s)) => Ok(Some(s.to_string())),
            Ok(None) => Ok(None),
            Err(_e) => {
                // If it is ESP_ERR_NVS_NOT_FOUND, we just return None
                // In esp-idf-sys, the error code might be checked, but for safety:
                Ok(None)
            }
        }
    }

    pub fn set_str(&mut self, key: &str, val: &str) -> Result<()> {
        self.nvs.set_str(key, val).context(format!("Failed to set NVS key {}", key))?;
        Ok(())
    }

    pub fn get_i32(&self, key: &str) -> Result<Option<i32>> {
        match self.nvs.get_i32(key) {
            Ok(Some(v)) => Ok(Some(v)),
            Ok(None) => Ok(None),
            Err(_) => Ok(None),
        }
    }

    pub fn set_i32(&mut self, key: &str, val: i32) -> Result<()> {
        self.nvs.set_i32(key, val).context(format!("Failed to set NVS i32 key {}", key))?;
        Ok(())
    }

    pub fn remove_key(&mut self, key: &str) -> Result<()> {
        let _ = self.nvs.remove(key);
        Ok(())
    }

    pub fn dump_to_log(&self) -> Result<()> {
        info!("=== NVS STORAGE DUMP ===");
        let keys_str = &[
            "totpSecret",
            "ntpServer",
            "fwVersion",
            "lastOtaDl",
            "lastOtaSuccess",
            "lastOtaWrite",
            "updateRepoList",
            "updateDlUrl",
            "wifiKnown",
            "nextCheck",
            "extName",
            "extDesc",
            "metricsUrl",
            // "meshId",
            // "meshPmk",
        ];
        for key in keys_str {
            match self.get_str(key) {
                Ok(Some(val)) => {
                    if *key == "totpSecret" {
                        let masked = if val.len() > 12 {
                            format!("{}......{}", &val[..6], &val[val.len() - 6..])
                        } else {
                            val.clone()
                        };
                        info!("  {} : \"{}\"", key, masked);
                    } else if *key == "wifiKnown" {
                        if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(&val) {
                            if let Ok(pretty) = serde_json::to_string_pretty(&json_val) {
                                info!("  {} :\n{}", key, pretty);
                            } else {
                                info!("  {} : \"{}\"", key, val);
                            }
                        } else {
                            info!("  {} : \"{}\"", key, val);
                        }
                    } else {
                        info!("  {} : \"{}\"", key, val);
                    }
                }
                Ok(None) => info!("  {} : [not set]", key),
                Err(e) => info!("  {} : Error({:?})", key, e),
            }
        }
        
        let keys_i32 = &["otaRetry", "autoUpdate", "deviceRenamable", "wifiChannel"];
        for key in keys_i32 {
            match self.get_i32(key) {
                Ok(Some(val)) => info!("  {} : {}", key, val),
                Ok(None) => info!("  {} : [not set]", key),
                Err(e) => info!("  {} : Error({:?})", key, e),
            }
        }
        // Dump device registry (devicesKnow) JSON if present
        match self.get_str("devicesKnow") {
            Ok(Some(reg_json)) => {
                if let Ok(json_val) = serde_json::from_str::<serde_json::Value>(&reg_json) {
                    if let Ok(pretty) = serde_json::to_string_pretty(&json_val) {
                        info!("  devicesKnow :\n{}", pretty);
                    } else {
                        info!("  devicesKnow : {}", reg_json);
                    }
                } else {
                    info!("  devicesKnow : {}", reg_json);
                }
            }
            Ok(None) => info!("  devicesKnow : [not set]"),
            Err(e) => info!("  devicesKnow : Error({:?})", e),
        }

        info!("========================");
        Ok(())
    }

    pub fn factory_reset(&mut self) -> Result<()> {
        unsafe {
            esp_idf_sys::nvs_flash_erase();
        }
        Ok(())
    }
}
