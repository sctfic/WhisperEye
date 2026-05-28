use esp_idf_svc::nvs::{EspNvs, EspNvsPartition, NvsDefault};
use anyhow::{Result, Context};
use log::{info, error};

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
            self.set_str("updateUrl", "https://github.com/sctfic/WhisperEye/raw/main/boards/board_default/firmware.bin")?;
            self.set_i32("otaRetry", -1)?;
        }
        Ok(())
    }

    pub fn get_str(&self, key: &str) -> Result<Option<String>> {
        // EspNvs get_str writes to a buffer. We'll use a dynamic buffer.
        let mut buf = [0u8; 256];
        match self.nvs.get_str(key, &mut buf) {
            Ok(Some(s)) => Ok(Some(s.to_string())),
            Ok(None) => Ok(None),
            Err(e) => {
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
