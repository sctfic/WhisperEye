use esp_idf_svc::nvs::{EspNvs, EspNvsPartition, NvsDefault};
use anyhow::{Result, Context};
use log::{info};

pub struct NvsStorage {
    nvs: EspNvs<NvsDefault>,
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
        // If a key doesn't exist, we set the default value.
        // We use some flag or check if a vital key is present.
        if self.get_str("wifiSsid")?.is_none() {
            info!("NVS is empty or uninitialized. Writing defaults...");
            self.set_str("wifiSsid", "IoT")?;
            self.set_str("wifiPsk", "Esp32&Cie2026")?;
            self.set_str("totpSecret", "Salt-4-Hash-Between-Probe-&-WhisperEye")?;
            self.set_str("ntpServer", "wrt.lan")?;
            self.set_str("fwVersion", "empty")?;
            self.set_str("lastOtaDl", "1970-01-01T00:00:00Z")?;
            self.set_str("lastOtaSuccess", "1970-01-01T00:00:00Z")?;
            self.set_str("updateAvailable", "https://github.com/sctfic/WhisperEye/raw/main/boards/board_default/firmware.json")?;
            self.set_str("updateDlUrl", "https://github.com/sctfic/WhisperEye/raw/main/boards/board_default/firmware.bin")?;
            self.set_i32("otaRetry", -1)?;
            self.set_str("wifiKnown", "[]")?;
        } else if self.get_str("wifiKnown")?.is_none() {
            self.set_str("wifiKnown", "[]")?;
        }
        Ok(())
    }

    pub fn get_known_networks(&self) -> Result<Vec<(String, String)>> {
        let known_str = self.get_str("wifiKnown")?.unwrap_or_else(|| "[]".to_string());
        #[derive(serde::Deserialize)]
        struct Net {
            ssid: String,
            psk: String,
        }
        let list: Vec<Net> = serde_json::from_str(&known_str).unwrap_or_default();
        Ok(list.into_iter().map(|n| (n.ssid, n.psk)).collect())
    }

    pub fn add_known_network(&mut self, ssid: &str, psk: &str) -> Result<()> {
        if ssid.is_empty() {
            return Ok(());
        }
        let mut list = self.get_known_networks()?;
        // Check if already exists, if so update the psk, else append
        if let Some(pos) = list.iter().position(|(s, _)| s == ssid) {
            list[pos].1 = psk.to_string();
        } else {
            list.push((ssid.to_string(), psk.to_string()));
        }
        
        #[derive(serde::Serialize)]
        struct Net {
            ssid: String,
            psk: String,
        }
        let serialized_list: Vec<Net> = list.into_iter().map(|(ssid, psk)| Net { ssid, psk }).collect();
        let new_str = serde_json::to_string(&serialized_list)?;
        self.set_str("wifiKnown", &new_str)?;
        Ok(())
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
}
