use esp_idf_svc::ota::EspOta;
use esp_idf_svc::http::client::{EspHttpConnection, Configuration};
use embedded_svc::http::Headers;
use anyhow::{Result, Context, anyhow};
use log::{info, error};
use std::io::Read;

pub fn perform_ota(update_url: &str) -> Result<()> {
    info!("Starting automatic OTA from URL: {}", update_url);
    
    // 1. Initialize HTTP Connection
    let config = Configuration {
        buffer_size: Some(2048), // Keep memory footprint small
        ..Default::default()
    };
    
    let mut connection = EspHttpConnection::new(&config)
        .context("Failed to create HTTP connection")?;
    
    connection.initiate_request(esp_idf_svc::http::Method::Get, update_url, &[])
        .context("Failed to initiate HTTP GET request")?;
        
    connection.initiate_response()
        .context("Failed to fetch HTTP response headers")?;
        
    let status = connection.status();
    if status != 200 {
        return Err(anyhow!("HTTP GET failed with status code {}", status));
    }
    
    let content_len = connection.content_len().unwrap_or(0);
    info!("OTA Binary size: {} bytes", content_len);
    
    // 2. Initialize ESP OTA
    let mut ota = EspOta::new().context("Failed to initialize ESP OTA")?;
    let mut ota_write = ota.initiate_update().context("Failed to initiate OTA partition update")?;
    
    // 3. Stream OTA data in chunks
    let mut buffer = [0u8; 1024]; // 1KB stream chunk
    let mut total_read = 0;
    
    loop {
        match connection.read(&mut buffer) {
            Ok(0) => break, // EOF
            Ok(n) => {
                ota_write.write(&buffer[..n])
                    .context("Failed writing chunk to OTA partition")?;
                total_read += n;
                if content_len > 0 {
                    let progress = (total_read as f32 / content_len as f32) * 100.0;
                    if total_read % (100 * 1024) == 0 || total_read == content_len as usize {
                        info!("OTA Progress: {:.1}% ({} / {}) bytes", progress, total_read, content_len);
                    }
                } else {
                    if total_read % (100 * 1024) == 0 {
                        info!("OTA Downloaded: {} bytes", total_read);
                    }
                }
            }
            Err(e) => {
                return Err(anyhow!("Error reading HTTP OTA stream: {:?}", e));
            }
        }
    }
    
    // 4. Finalize & Complete OTA
    info!("OTA stream complete. Writing update to boot configuration...");
    ota_write.complete().context("Failed to finalize OTA update")?;
    
    info!("OTA Update fully successful!");
    Ok(())
}
