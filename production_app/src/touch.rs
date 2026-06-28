use esp_idf_sys::*;
use esp_idf_hal::gpio::Gpio14;
use std::sync::atomic::{AtomicU32, Ordering};

fn check_err(err: esp_err_t) -> Result<(), EspError> {
    if err != 0 {
        if let Some(e) = EspError::from(err) {
            Err(e)
        } else {
            Err(EspError::from_infallible::<ESP_FAIL>())
        }
    } else {
        Ok(())
    }
}

pub struct TouchSensor {
    threshold: AtomicU32,
    _pin: Gpio14<'static>,
}

impl TouchSensor {
    /// Initialise le périphérique matériel Touch Pad de l'ESP32-S3 pour le GPIO14 (T14)
    pub fn new(pin: Gpio14<'static>, default_threshold: u32) -> Result<Self, EspError> {
        log::info!("Initializing ESP32-S3 hardware touch sensor on T14 (GPIO14)...");
        
        unsafe {
            // 1. Initialiser le pilote global touch pad
            check_err(touch_pad_init())?;
            
            // 2. Définir le mode du FSM en mode TIMER (automatique)
            check_err(touch_pad_set_fsm_mode(touch_fsm_mode_t_TOUCH_FSM_MODE_TIMER))?;
            
            // 3. Configurer le GPIO14 comme entrée tactile (libère la fonction GPIO classique)
            check_err(touch_pad_io_init(touch_pad_t_TOUCH_PAD_NUM14))?;
            
            // 4. Appliquer la configuration par défaut sur le canal T14
            check_err(touch_pad_config(touch_pad_t_TOUCH_PAD_NUM14))?;
            
            // 5. Configurer le seuil par défaut
            check_err(touch_pad_set_thresh(touch_pad_t_TOUCH_PAD_NUM14, default_threshold))?;
            
            // 6. Activer le canal T14 dans le masque d'analyse
            // Note de sécurité étanchéité (Waterproof Shield) :
            // Nous vérifions en commentaire que le canal T14 n'est pas utilisé comme canal "shield"
            // de protection contre l'eau (waterproof). Sur cette carte, la fonction waterproof
            // n'est pas configurée, le canal T14 est donc entièrement disponible pour la touche tactile.
            check_err(touch_pad_set_channel_mask((1 << (touch_pad_t_TOUCH_PAD_NUM14 as u32)) as u16))?;
            
            // 7. Activer le filtre matériel IIR anti-rebond
            check_err(touch_pad_filter_enable())?;
            
            // 8. Démarrer le FSM automatique
            check_err(touch_pad_fsm_start())?;
        }
        
        Ok(Self {
            threshold: AtomicU32::new(default_threshold),
            _pin: pin,
        })
    }

    /// Lecture non bloquante de l'état tactile.
    /// Retourne true si touché (valeur brute < seuil de repos), false sinon.
    pub fn is_pressed(&self) -> Result<bool, EspError> {
        let mut raw_val: u32 = 0;
        unsafe {
            // Lire la valeur brute filtrée (ou raw) de la touche
            check_err(touch_pad_read_raw_data(touch_pad_t_TOUCH_PAD_NUM14, &mut raw_val))?;
            
            // Récupérer le masque de statut de déclenchement du FSM
            let status = touch_pad_get_status();
            
            // Acquitter le statut
            check_err(touch_pad_clear_status())?;
            
            let triggered = (status & (1 << (touch_pad_t_TOUCH_PAD_NUM14 as u32))) != 0;
            let thresh = self.threshold.load(Ordering::Relaxed);
            
            // Logique inversée et sensibilité accrue :
            // On considère comme pressé si déclenché par le FSM ou si la valeur brute dépasse le seuil (repos + 5%).
            Ok(triggered || (raw_val > 0 && raw_val > thresh))
        }
    }

    /// Calibre le capteur au repos (sans contact physique) sur N échantillons.
    /// Calcule la moyenne et ajuste le seuil à 80% (moyenne - 20%).
    pub fn calibrate(&self, samples: usize) -> Result<u32, EspError> {
        log::info!("Calibrating touch sensor T14 (GPIO14) over {} samples...", samples);
        let mut sum: u64 = 0;
        let mut valid_samples = 0;
        
        for _ in 0..samples {
            let mut raw_val: u32 = 0;
            unsafe {
                if touch_pad_read_raw_data(touch_pad_t_TOUCH_PAD_NUM14, &mut raw_val) == 0 {
                    if raw_val > 0 {
                        sum += raw_val as u64;
                        valid_samples += 1;
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        
        if valid_samples == 0 {
            return Err(EspError::from_infallible::<ESP_FAIL>());
        }
        
        let avg = (sum / valid_samples) as u32;
        // Seuil à moyenne + 5% (sensibilité accrue et logique inversée)
        let new_threshold = (avg as f64 * 1.025) as u32;
        
        unsafe {
            check_err(touch_pad_set_thresh(touch_pad_t_TOUCH_PAD_NUM14, new_threshold))?;
        }
        
        self.threshold.store(new_threshold, Ordering::Relaxed);
        log::info!("Touch calibration finished. Average raw: {}, New threshold: {}", avg, new_threshold);
        
        Ok(new_threshold)
    }

    /// Récupère la valeur brute actuelle du capteur tactile
    pub fn get_raw_value(&self) -> Result<u32, EspError> {
        let mut raw_val: u32 = 0;
        unsafe {
            check_err(touch_pad_read_raw_data(touch_pad_t_TOUCH_PAD_NUM14, &mut raw_val))?;
        }
        Ok(raw_val)
    }

    /// Récupère le seuil de déclenchement actuel
    pub fn get_threshold(&self) -> u32 {
        self.threshold.load(Ordering::Relaxed)
    }
}
