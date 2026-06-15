use esp_idf_svc::ota::EspOta;
use esp_idf_svc::http::client::{EspHttpConnection, Configuration};
use embedded_svc::http::Headers;
use anyhow::{Result, Context, anyhow};
use log::{info};
use std::sync::Mutex;

use std::sync::Arc;

pub struct UpdateStatus {
    pub percentage: u8,
    pub size: usize,
    pub written: usize,
    pub status: &'static str,
}

pub static UPDATE_STATUS: Mutex<UpdateStatus> = Mutex::new(UpdateStatus {
    percentage: 0,
    size: 0,
    written: 0,
    status: "En attente",
});

pub fn perform_ota(update_url: &str, nvs: Arc<Mutex<common::nvs_storage::NvsStorage>>) -> Result<()> {
    match perform_ota_inner(update_url, nvs) {
        Ok(()) => {
            if let Ok(mut status) = UPDATE_STATUS.lock() {
                status.percentage = 100;
                status.status = "Mise à jour terminée. Redémarrage...";
            }
            Ok(())
        }
        Err(e) => {
            if let Ok(mut status) = UPDATE_STATUS.lock() {
                status.percentage = 0;
                status.status = "Erreur lors de la mise à jour";
            }
            Err(e)
        }
    }
}

pub fn get_formatted_time() -> String {
    use std::time::SystemTime;
    let now = SystemTime::now();
    let duration = now.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
    let total_secs = duration.as_secs();
    if total_secs < 86400 {
        return "1970-01-01T00:00:00Z".to_string();
    }
    let secs = total_secs % 60;
    let mins = (total_secs / 60) % 60;
    let hours = (total_secs / 3600) % 24;
    format!("2026-05-27T{:02}:{:02}:{:02}Z", hours, mins, secs)
}

fn perform_ota_inner(update_url: &str, nvs: Arc<Mutex<common::nvs_storage::NvsStorage>>) -> Result<()> {
    info!("Starting automatic OTA from URL: {}", update_url);
    common::led::set_led_color(common::led::BLUE, 25); // Bleu à 10%
    {

        let mut status = UPDATE_STATUS.lock().unwrap();
        status.percentage = 0;
        status.size = 0;
        status.written = 0;
        status.status = "Connexion au serveur de mise à jour...";
    }
    
    // 1. Initialize HTTP Connection
    let config = Configuration {
        buffer_size: Some(2048), // Keep memory footprint small
        crt_bundle_attach: Some(esp_idf_sys::esp_crt_bundle_attach),
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
    {
        let mut status = UPDATE_STATUS.lock().unwrap();
        status.size = content_len as usize;
    }
    
    // Set lastOtaDl when starting download
    let now_str = get_formatted_time();
    if let Ok(mut storage) = nvs.lock() {
        let _ = storage.set_str("lastOtaDl", &now_str);
    }
    
    // 2. Initialize ESP OTA
    let mut ota = EspOta::new().context("Failed to initialize ESP OTA")?;
    let mut ota_write = ota.initiate_update().context("Failed to initiate OTA partition update")?;
    
    // 3. Stream OTA data in chunks
    let mut buffer = [0u8; 2048]; // 2KB stream chunk
    let mut total_read = 0;
    
    info!("\x1b[35;1m[ÉTAPE 4] Téléchargement du binaire bloc par bloc\x1b[0m");
    info!("\x1b[36;1m  -> Source URL : {}\x1b[0m", update_url);
    info!("\x1b[36;1m  -> Taille totale attendue : {} octets\x1b[0m", content_len);
    
    {
        let mut status = UPDATE_STATUS.lock().unwrap();
        status.percentage = 0;
        status.status = "Téléchargement du firmware...";
    }
 
    loop {
        match connection.read(&mut buffer) {
            Ok(0) => break, // EOF
            Ok(n) => {
                ota_write.write(&buffer[..n])
                    .context("Failed writing chunk to OTA partition")?;
                total_read += n;
                let progress = if content_len > 0 {
                    ((total_read as f32 / content_len as f32) * 100.0) as u8
                } else {
                    0
                };
                {
                    let mut status = UPDATE_STATUS.lock().unwrap();
                    status.percentage = progress;
                    status.written = total_read;
                }
                if content_len > 0 {
                    print!("\r\x1b[36;1m  -> Progression : {}% ({} / {} octets)\x1b[0m", progress, total_read, content_len);
                } else {
                    print!("\r\x1b[36;1m  -> Progression : {} octets\x1b[0m", total_read);
                }
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
            Err(e) => {
                return Err(anyhow!("Error reading HTTP OTA stream: {:?}", e));
            }
        }
    }
    println!(); // print newline after progress bar completes
    
    // 4. Finalize & Complete OTA
    info!("\x1b[35;1m[ÉTAPE 5] Écriture en mémoire flash\x1b[0m");
    info!("\x1b[36;1m  -> Finalisation de la partition et écriture dans la table de boot (Total : {} octets)...\x1b[0m", total_read);
    {
        let mut status = UPDATE_STATUS.lock().unwrap();
        status.percentage = 100;
        status.written = total_read;
        status.status = "Écriture en mémoire flash...";
    }
    ota_write.complete().context("Failed to finalize OTA update")?;
    
    // Set lastOtaWrite when write completes successfully
    let now_str = get_formatted_time();
    if let Ok(mut storage) = nvs.lock() {
        let _ = storage.set_str("lastOtaWrite", &now_str);
    }
    
    info!("\x1b[35;1m[ÉTAPE 5] Flashage terminé avec succès !\x1b[0m");
    Ok(())
}
